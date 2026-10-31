<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api, describe, type AudioFormat, type Citation, type Source, type StudySet } from '#lib/api.ts';
	import AudioOverview from './AudioOverview.svelte';
	import { ago } from '#lib/format.ts';
	import { clip } from './Mark.svelte';
	import Sheet from './Sheet.svelte';

	let {
		slug,
		id,
		sources,
		modal = false,
		onclose,
		onopen,
		onask,
		onchange,
		ondelete
	}: {
		slug: string;
		id: string;
		sources: Map<string, Source>;
		modal?: boolean;
		onclose: () => void;
		/** Opens the passage a card or question came from. */
		onopen: (citations: Citation[], el: HTMLElement) => void;
		/** Asks a question in the conversation. */
		onask: (question: string) => void;
		/** Progress was saved. */
		onchange: (set: StudySet) => void;
		ondelete: () => void;
	} = $props();

	let set = $state<StudySet | null>(null);
	let problem = $state<string | null>(null);
	let confirming = $state(false);
	let wide = $state(false);

	onMount(async () => {
		try {
			set = await api.study(slug, id);
			if (set.kind === 'flashcards') startCards(set.due > 0 ? 'due' : 'none');
			else if (set.kind === 'quiz') answers = (set.questions ?? []).map(() => null);
			// Start on the first control, not the sheet's close button, so the
			// keys the hint names act on the cards.
			await tick();
			(reveal ?? document.querySelector<HTMLButtonElement>('.pane .choice, .pane .end .primary'))?.focus();
		} catch (e) {
			problem = describe(e);
		}
	});

	const DIFFICULTY = { easy: 'Easy', medium: 'Medium', hard: 'Hard' } as const;
	const FORMAT: Record<AudioFormat, string> = {
		'deep-dive': 'Deep dive',
		brief: 'Brief',
		critique: 'Critique',
		debate: 'Debate'
	};
	const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

	// Flashcards: a queue of card positions. Missed cards go to the back of
	// the queue and come round again before the session ends.
	let reveal = $state<HTMLButtonElement>();
	let gotIt = $state<HTMLButtonElement>();
	let nextButton = $state<HTMLButtonElement>();
	let queue = $state<number[]>([]);
	let shown = $state(false);
	let reviewed = $state(0);
	let knewCount = $state(0);
	let session = $state<'due' | 'all' | 'none'>('none');
	const card = $derived(set?.cards && queue.length ? set.cards[queue[0]] : null);

	function startCards(mode: 'due' | 'all' | 'none') {
		if (!set?.cards) return;
		const now = Date.now() / 1000;
		session = mode;
		reviewed = 0;
		knewCount = 0;
		shown = false;
		queue = mode === 'none' ? [] : set.cards.flatMap((c, i) => (mode === 'all' || c.due <= now ? [i] : []));
	}

	// Each step replaces the button that was pressed, so focus moves to the
	// next control instead of falling back to the page.
	async function showAnswer() {
		shown = true;
		await tick();
		gotIt?.focus();
	}

	async function grade(knew: boolean) {
		if (!set || !queue.length) return;
		const position = queue[0];
		queue = knew ? queue.slice(1) : [...queue.slice(1), position];
		shown = false;
		reviewed += 1;
		if (knew) knewCount += 1;
		tick().then(() => (reveal ?? document.querySelector<HTMLButtonElement>('.pane .end .primary'))?.focus());
		try {
			set = await api.review(slug, id, position, knew);
			onchange(set);
		} catch (e) {
			problem = describe(e);
		}
	}

	const nextDue = $derived.by(() => {
		const upcoming = (set?.cards ?? []).map((c) => c.due).filter((due) => due > Date.now() / 1000);
		return upcoming.length ? Math.min(...upcoming) : null;
	});

	function exportCsv() {
		const table = set?.table;
		if (!set || !table) return;
		const field = (s: string) => (/[",\n]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s);
		const source = (c: Citation | null) => (c?.source_id ? (sources.get(c.source_id)?.title ?? '') : '');
		const lines = [
			[...table.columns, 'Source', 'Quote'].map(field).join(','),
			...table.rows.map((r) => [...r.cells, source(r.citation), r.citation?.quote ?? ''].map(field).join(','))
		];
		const link = document.createElement('a');
		link.href = URL.createObjectURL(new Blob([`${lines.join('\r\n')}\r\n`], { type: 'text/csv' }));
		link.download = `${set.title}.csv`;
		link.click();
		URL.revokeObjectURL(link.href);
	}

	function exportForAnki() {
		if (!set?.cards) return;
		// Anki reads one note per line with fields split by tabs; HTML line breaks keep multi-line text.
		const field = (s: string) => s.replace(/\t/g, ' ').replace(/\r?\n/g, '<br>');
		const lines = set.cards.map((c) => {
			const source = c.citation?.source_id ? sources.get(c.citation.source_id)?.title : undefined;
			return [field(c.front), field(c.back), field(source ?? '')].join('\t');
		});
		const blob = new Blob([`#separator:tab\n#html:true\n${lines.join('\n')}\n`], { type: 'text/plain' });
		const link = document.createElement('a');
		link.href = URL.createObjectURL(blob);
		link.download = `${set.title}.txt`;
		link.click();
		URL.revokeObjectURL(link.href);
	}

	// Quiz: one question at a time, then the score.
	let answers = $state<(number | null)[]>([]);
	let at = $state(0);
	let hint = $state(false);
	let finished = $state(false);
	const question = $derived(set?.questions?.[at] ?? null);
	const picked = $derived(answers[at] ?? null);
	const score = $derived(set?.questions?.filter((q, i) => answers[i] === q.answer).length ?? 0);
	const best = $derived(Math.max(0, ...(set?.attempts ?? []).map((a) => a.score)));

	async function pick(choice: number) {
		if (picked !== null) return;
		answers[at] = choice;
		await tick();
		nextButton?.focus();
	}

	async function next() {
		hint = false;
		if (!set?.questions) return;
		if (at + 1 < set.questions.length) {
			at += 1;
			await tick();
			document.querySelector<HTMLButtonElement>('.choice')?.focus();
			return;
		}
		finished = true;
		try {
			set = await api.recordAttempt(slug, id, answers);
			onchange(set);
		} catch (e) {
			problem = describe(e);
		}
	}

	function retake() {
		answers = (set?.questions ?? []).map(() => null);
		at = 0;
		hint = false;
		finished = false;
	}

	async function remove() {
		try {
			await api.deleteStudy(slug, id);
			ondelete();
		} catch (e) {
			problem = describe(e);
		}
	}

	function keys(e: KeyboardEvent) {
		const target = e.target as HTMLElement;
		if (target.closest('input, textarea, [contenteditable]') || e.metaKey || e.ctrlKey || e.altKey || !set) return;
		if (set.kind === 'flashcards' && card) {
			if (!shown && (e.key === ' ' || e.key === 'Enter') && !target.closest('button')) {
				e.preventDefault();
				showAnswer();
			} else if (shown && e.key === '1') grade(false);
			else if (shown && e.key === '2') grade(true);
		} else if (set.kind === 'quiz' && question && !finished) {
			const n = Number(e.key);
			if (picked === null && n >= 1 && n <= question.choices.length) pick(n - 1);
		}
	}

	const where = (c: Citation | null) => {
		if (!c) return '';
		const source = c.source_id ? sources.get(c.source_id) : undefined;
		return [source?.title ?? 'a removed source', c.page ? `p. ${c.page}` : ''].filter(Boolean).join(', ');
	};
</script>

<svelte:window onkeydown={keys} />

{#snippet cited(citation: Citation | null)}
	{#if citation}
		<button class="cited" onclick={(e) => onopen([citation], e.currentTarget)}>
			<span class="siglum">{citation.letter}{#if !citation.verified}†{/if}</span>
			{where(citation)}: “{clip(citation.quote, 90)}”
			{#if !citation.verified}<span class="doubt">Not found in the source.</span>{/if}
		</button>
	{/if}
{/snippet}

<Sheet
	label={set?.title ?? 'Study set'}
	{modal}
	wide={wide && set?.kind === 'table'}
	closeLabel={set?.kind === 'quiz'
		? 'Close quiz'
		: set?.kind === 'table'
			? 'Close table'
			: set?.kind === 'audio'
				? 'Close audio overview'
				: 'Close flashcards'}
	{onclose}
>
	{#snippet heading()}
		<h2>{set?.title ?? 'Opening…'}</h2>
		{#if set}
			<p class="meta">
				{#if set.kind === 'table'}
					{count(set.table?.rows.length ?? 0, 'row', 'rows')}{#if set.topic}, on {set.topic}{/if}
				{:else if set.kind === 'audio' && set.audio}
					{FORMAT[set.audio.format]}{#if set.audio.seconds}, {Math.max(1, Math.round(set.audio.seconds / 60))} min{/if}{#if set.topic},
						on {set.topic}{/if}
				{:else}
					{set.kind === 'flashcards' ? count(set.cards?.length ?? 0, 'card', 'cards') : count(set.questions?.length ?? 0, 'question', 'questions')},
					{DIFFICULTY[set.difficulty].toLowerCase()}{#if set.topic}, on {set.topic}{/if}
				{/if}
			</p>
			{#if !confirming}
				<p class="actions">
					{#if set.kind === 'flashcards'}<button class="action" onclick={exportForAnki}>Export for Anki</button>{/if}
					{#if set.kind === 'table'}
						<button class="action" onclick={exportCsv}>Export as CSV</button>
						<button class="action wide-toggle" onclick={() => (wide = !wide)}>{wide ? 'Narrower' : 'Wider'}</button>
					{/if}
					<button class="action" onclick={() => (confirming = true)}>Delete</button>
				</p>
			{/if}
		{/if}
	{/snippet}
	{#snippet top()}
		{#if confirming}
			<div class="confirm" role="alert">
				<p>Delete {set?.title}? Its progress goes with it.</p>
				<p class="actions">
					<button class="action" onclick={remove}>Delete</button>
					<button class="action quiet" onclick={() => (confirming = false)}>Cancel</button>
				</p>
			</div>
		{/if}
		{#if problem}<p class="problem" role="alert">{problem}</p>{/if}
	{/snippet}

	{#if set?.kind === 'audio' && set.audio}
		<AudioOverview
			{slug}
			{id}
			{set}
			{onopen}
			onupdate={(next) => {
				const finished = !set?.audio?.seconds && next.audio?.seconds;
				set = next;
				if (finished) onchange(next);
			}}
		/>
	{:else if set?.kind === 'table' && set.table}
		<div class="table-wrap">
			<table>
				<thead>
					<tr>
						{#each set.table.columns as column, i (i)}<th scope="col">{column}</th>{/each}
						<th scope="col"><span class="visually-hidden">Source</span></th>
					</tr>
				</thead>
				<tbody>
					{#each set.table.rows as row, i (i)}
						<tr>
							{#each row.cells as value, j (j)}<td>{value}</td>{/each}
							<td class="cite-cell">
								{#if row.citation}
									{@const c = row.citation}
									<button
										class="siglum-button"
										class:unverified={!c.verified}
										aria-label="Source {c.letter}{c.verified ? '' : ', quote not found'}: {c.quote}"
										title={c.quote}
										onclick={(e) => onopen([c], e.currentTarget)}>{c.letter}{#if !c.verified}†{/if}</button
									>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
	{:else if set?.kind === 'flashcards' && set.cards}
		{#if card}
			<p class="progress">{reviewed} done, {queue.length} to go</p>
			<p class="front">{card.front}</p>
			{#if shown}
				<p class="back">{card.back}</p>
				{@render cited(card.citation)}
				<div class="grade">
					<button class="secondary" onclick={() => grade(false)}>Again <kbd>1</kbd></button>
					<button class="primary" bind:this={gotIt} onclick={() => grade(true)}>Got it <kbd>2</kbd></button>
				</div>
				<button class="link" onclick={() => onask(`Explain this: ${card.front}`)}>Ask about this card</button>
			{:else}
				<button class="primary reveal" bind:this={reveal} onclick={showAnswer}>Show answer</button>
				<p class="hint">
					Say the answer to yourself first.{' '}<span class="keys">Space shows it; then 1 for again, 2 for got it.</span>
				</p>
			{/if}
		{:else}
			<div class="end">
				{#if session === 'none'}
					<p>Nothing is due right now.</p>
				{:else}
					<p>
						Done. {knewCount} known, {reviewed - knewCount} to practise again; you went through
						{count(reviewed, 'card', 'cards')}.
					</p>
				{/if}
				{#if nextDue}<p class="hint">The next card comes back {ago(nextDue)}.</p>{/if}
				<button class="primary" onclick={() => startCards('all')}>Go through all {set.cards.length} cards</button>
			</div>
		{/if}
	{:else if set?.kind === 'quiz' && set.questions}
		{#if finished}
			<div class="end">
				<p class="score">{score} of {set.questions.length} right.</p>
				{#if set.attempts && set.attempts.length > 1}<p class="hint">Your best is {best} of {set.questions.length}.</p>{/if}
				<ol class="review">
					{#each set.questions as q, i (i)}
						<li class:wrong={answers[i] !== q.answer}>
							<p class="review-q">{q.question}</p>
							<p class="review-a">
								{#if answers[i] === q.answer}Right: {q.choices[q.answer]}{:else}
									The answer is {q.choices[q.answer]}{#if answers[i] !== null}; you picked {q.choices[answers[i]!]}{/if}.
								{/if}
							</p>
							{#if answers[i] !== q.answer}{@render cited(q.citation)}{/if}
						</li>
					{/each}
				</ol>
				<button class="primary" onclick={retake}>Take it again</button>
			</div>
		{:else if question}
			<p class="progress">Question {at + 1} of {set.questions.length}</p>
			<p class="front">{question.question}</p>
			<ol class="choices">
				{#each question.choices as choice, i (i)}
					<li>
						<button
							class="choice"
							class:right={picked !== null && i === question.answer}
							class:wrong={picked === i && i !== question.answer}
							aria-pressed={picked === i}
							disabled={picked !== null}
							onclick={() => pick(i)}
						>
							<span class="n">{i + 1}</span>{choice}
						</button>
					</li>
				{/each}
			</ol>
			{#if picked === null}
				{#if hint}
					<p class="hint">{question.hint}</p>
				{:else if question.hint}
					<button class="link" onclick={() => (hint = true)}>Show a hint</button>
				{/if}
			{:else}
				<p class="verdict" role="status">{picked === question.answer ? 'Right.' : 'Not quite.'} {question.explanation}</p>
				{@render cited(question.citation)}
				<button class="primary" bind:this={nextButton} onclick={next}>
					{at + 1 < set.questions.length ? 'Next question' : 'See your score'}
				</button>
			{/if}
		{/if}
	{/if}
</Sheet>

<style>
	h2 {
		margin: 0;
		font-size: var(--t-l);
		font-style: italic;
		font-weight: 400;
		font-variation-settings: 'opsz' 36;
		line-height: 1.3;
	}
	.meta,
	.progress,
	.hint {
		margin: 0.3rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.actions {
		display: flex;
		gap: 1rem;
		margin: 0.5rem 0 0;
	}
	.action,
	.link {
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: var(--t-xs);
		font-weight: 600;
		cursor: pointer;
	}
	.action.quiet {
		color: var(--ink-2);
		font-weight: 400;
	}
	.link {
		display: block;
		margin-top: 1rem;
		font-style: italic;
		font-weight: 400;
	}
	.confirm {
		padding: 1rem 1.5rem;
		border-bottom: 1px solid var(--line);
		font-size: var(--t-s);
	}
	.confirm p {
		margin: 0;
	}
	.problem {
		margin: 0;
		padding: 0.75rem 1.5rem 0;
		font-style: italic;
		color: var(--danger);
	}
	.progress {
		margin: 0 0 1rem;
	}
	.front {
		margin: 0 0 1.5rem;
		font-size: var(--t-xl);
		font-style: italic;
		font-variation-settings: 'opsz' 48;
		line-height: 1.3;
		text-wrap: balance;
	}
	.back {
		margin: 0 0 1rem;
		padding-top: 1rem;
		border-top: 1px solid var(--line);
		font-size: var(--t-m);
		line-height: 1.55;
	}
	.cited {
		display: block;
		margin: 0 0 1.25rem;
		padding: 0 0 0 0.6rem;
		border: 0;
		border-left: 1px solid var(--line);
		background: none;
		color: var(--ink-2);
		font-size: var(--t-xs);
		line-height: 1.45;
		text-align: left;
		cursor: pointer;
	}
	.cited:hover {
		border-left-color: var(--rubric);
		color: var(--ink);
	}
	.siglum {
		margin-right: 0.35em;
		color: var(--rubric);
		font-style: italic;
		font-weight: 600;
	}
	.doubt {
		display: block;
		font-style: italic;
	}
	.grade {
		display: flex;
		gap: 0.75rem;
	}
	.primary,
	.secondary {
		padding: calc(0.5rem - var(--cap-lift)) 1.1rem calc(0.5rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		font-size: var(--t-s);
		font-weight: 600;
		cursor: pointer;
	}
	.secondary {
		background: transparent;
		color: var(--rubric);
	}
	kbd {
		margin-left: 0.4rem;
		font: inherit;
		font-weight: 400;
		opacity: 0.7;
	}
	.end p {
		margin: 0 0 1rem;
	}
	.score {
		font-size: var(--t-xl);
		font-style: italic;
	}
	.choices,
	.review {
		margin: 0 0 1.25rem;
		padding: 0;
		list-style: none;
	}
	.choices li + li {
		margin-top: 0.5rem;
	}
	.choice {
		display: flex;
		gap: 0.75rem;
		width: 100%;
		padding: calc(0.6rem - var(--cap-lift)) 0.75rem calc(0.6rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--page);
		box-shadow: inset 0 0 0 1px var(--line);
		font-size: var(--t-s);
		line-height: 1.4;
		text-align: left;
		cursor: pointer;
	}
	.choice:not(:disabled):hover {
		box-shadow: inset 0 0 0 1px var(--ink);
	}
	.choice:disabled {
		cursor: default;
	}
	.n {
		flex: none;
		color: var(--ink-2);
		font-style: italic;
	}
	.choice.right {
		box-shadow: inset 2px 0 0 var(--rubric), inset 0 0 0 1px var(--ink);
		font-weight: 600;
	}
	.choice.right .n::after {
		content: ' ✓';
		color: var(--rubric);
	}
	.choice.wrong {
		color: var(--ink-2);
		text-decoration: line-through;
		text-decoration-color: var(--rubric);
	}
	.verdict {
		margin: 0 0 0.75rem;
	}
	.review li {
		padding: 0.6rem 0;
		border-top: 1px solid var(--line);
	}
	.review p {
		margin: 0;
	}
	.review-q {
		font-style: italic;
	}
	.review-a {
		font-size: var(--t-xs);
		color: var(--ink-2);
	}
	.review li.wrong .review-a {
		color: var(--rubric);
	}
	.table-wrap {
		margin: 0 -0.5rem;
		overflow-x: auto;
	}
	table {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--t-s);
		line-height: 1.4;
	}
	th {
		padding: 0.4rem 0.5rem;
		border-bottom: 1px solid var(--edge);
		font-size: var(--t-xs);
		font-style: italic;
		font-weight: 400;
		color: var(--ink-2);
		text-align: left;
		white-space: nowrap;
	}
	td {
		padding: 0.5rem;
		border-bottom: 1px solid var(--line);
		vertical-align: top;
	}
	td:first-child {
		font-weight: 600;
	}
	.cite-cell {
		width: 1.5rem;
		padding-inline: 0.25rem;
	}
	.siglum-button {
		min-width: 1.5rem;
		min-height: 1.5rem;
		padding: 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-style: italic;
		font-weight: 600;
		cursor: pointer;
	}
	.siglum-button.unverified {
		color: var(--ink-2);
	}
	@media (max-width: 959px) {
		.wide-toggle {
			display: none;
		}
	}
	/* Touch screens have no keys to press. */
	@media (pointer: coarse) {
		kbd,
		.keys {
			display: none;
		}
	}
</style>
