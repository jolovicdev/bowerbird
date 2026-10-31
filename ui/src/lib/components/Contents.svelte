<script lang="ts">
	import type { ChatSummary, NoteSummary, Progress, Source, StudyKind, StudySummary } from '#lib/api.ts';
	import { embeddingNote, readingNote, sourceMeta, transcribingNote } from '#lib/format.ts';
	import { closeOnBack, popLayer, pushLayer } from '#lib/layers.svelte.ts';

	let {
		sources,
		embedding,
		active,
		ready,
		inert = false,
		pending = [],
		fresh = new Set<string>(),
		chats,
		chat,
		notes,
		studies,
		writing = [],
		reports = [],
		open = $bindable(false),
		onadd,
		onopen,
		onexclude,
		onchat,
		onnewchat,
		onnote,
		onnewnote,
		onnewreport,
		onstudy,
		onnewstudy
	}: {
		sources: Source[];
		/** Embedding of new passages for search by meaning, while under way. */
		embedding?: Progress;
		/** Id of the source, note, or study set open in the sheet. */
		active: string | null;
		/** The notebook loaded; until then the list stays empty rather than claiming 0 sources. */
		ready: boolean;
		/** A sheet covers the page, so the list can't be used. */
		inert?: boolean;
		/** Names of sources being added right now. */
		pending?: string[];
		/** Sources that just arrived. */
		fresh?: Set<string>;
		chats: ChatSummary[];
		/** The conversation on the page; null for one not started yet. */
		chat: string | null;
		notes: NoteSummary[];
		studies: StudySummary[];
		/** Study sets being written right now. */
		writing?: StudyKind[];
		/** Names of reports being written right now. */
		reports?: string[];
		/** Below the three-column width, the contents open over the page. */
		open?: boolean;
		onadd: () => void;
		onopen: (source: Source) => void;
		onexclude: (sources: Source[], excluded: boolean) => void;
		onchat: (id: string) => void;
		onnewchat: () => void;
		onnote: (id: string) => void;
		onnewnote: () => void;
		onnewreport: () => void;
		onstudy: (set: StudySummary) => void;
		onnewstudy: (kind: StudyKind) => void;
	} = $props();

	const off = $derived(sources.filter((s) => s.excluded));
	const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
	function studyMeta(s: StudySummary): string {
		switch (s.kind) {
			case 'flashcards':
				return `${count(s.count, 'card', 'cards')}${s.due ? `, ${s.due} due` : ''}`;
			case 'quiz':
				return `${count(s.count, 'question', 'questions')}${s.best !== null ? `, best score ${s.best} of ${s.count}` : ''}`;
			case 'table':
				return `Table, ${count(s.count, 'row', 'rows')}`;
			case 'slides':
				return count(s.count, 'slide', 'slides');
			case 'mindmap':
				return 'Mind map';
			case 'infographic':
				return 'Infographic';
			case 'audio':
				return s.seconds ? `Audio, ${Math.max(1, Math.round(s.seconds / 60))} min` : 'Audio, recording…';
		}
	}
	const WRITING = {
		flashcards: 'Writing flashcards…',
		quiz: 'Writing a quiz…',
		mindmap: 'Drawing a mind map…',
		table: 'Building a table…',
		slides: 'Making slides…',
		infographic: 'Laying out an infographic…',
		audio: 'Writing an audio overview…'
	};

	let panel = $state<HTMLElement>();
	let toggle = $state<HTMLButtonElement>();
	let returnTo: HTMLElement | null = null;

	// Opened over the page, the contents take focus and a history entry, and
	// give focus back on closing.
	$effect(() => {
		if (open) {
			returnTo = document.activeElement instanceof HTMLElement ? document.activeElement : null;
			toggle?.focus();
			pushLayer('contents');
		} else if (returnTo) {
			const target = returnTo;
			returnTo = null;
			if (!panel?.contains(document.activeElement) && document.activeElement !== document.body) return;
			target.focus();
		}
	});

	closeOnBack('contents', () => open, () => (open = false));
	$effect(() => {
		if (!open) popLayer('contents');
	});

	/** Runs an action from the list, closing the contents first, so a sheet the
	 * action opens takes its history entry after theirs is gone. */
	const pick =
		<A extends unknown[]>(action: (...args: A) => void) =>
		async (...args: A) => {
			open = false;
			await popLayer('contents');
			action(...args);
		};
</script>

<svelte:window
	onkeydown={(e) => {
		if (open && e.key === 'Escape' && !e.defaultPrevented) {
			e.preventDefault();
			open = false;
		}
	}}
/>

{#if open}<button class="scrim" aria-label="Close contents" tabindex="-1" onclick={() => (open = false)}></button>{/if}
<aside class="contents" class:open aria-label="Contents" {inert} bind:this={panel}>
	{#if ready}
		<section class="part sources-part">
			{#if sources.length || pending.length}<h2>Sources</h2>{/if}
			<ol>
				{#each sources as source (source.id)}
					<li class:active={source.id === active} class:fresh={fresh.has(source.id)} class:off={source.excluded}>
						<button
							class="source"
							aria-label="{source.title}, source {source.letter}{source.excluded ? ', switched off' : ''}"
							aria-current={source.id === active ? 'true' : undefined}
							onclick={() => pick(onopen)(source)}
						>
							<span class="siglum">{source.letter}</span>
							<span class="info">
								<span class="title">{source.title}</span>
								{#if source.error}
									<span class="error">{source.error}</span>
								{:else if source.transcribing}
									<span class="meta">{transcribingNote(source.transcribing)}</span>
								{:else if source.reading}
									<span class="meta">{readingNote(source.reading)}</span>
								{:else}
									<span class="meta">{sourceMeta(source.kind, source.pages)}</span>
								{/if}
							</span>
						</button>
						<label class="switch" title="Use in answers and study sets">
							<input
								class="check quiet"
								type="checkbox"
								checked={!source.excluded}
								aria-label="Use {source.title} in answers"
								onchange={(e) => onexclude([source], !e.currentTarget.checked)}
							/>
						</label>
					</li>
				{/each}
				{#each pending as name, i (i)}
					<li class="pending">
						<span class="siglum" aria-hidden="true"></span>
						<span class="info">
							<span class="title">{name}</span>
							<span class="meta">Adding…</span>
							<span class="progress" aria-hidden="true"></span>
						</span>
					</li>
				{/each}
			</ol>
			{#if embedding}
				<p class="index-note" role="status">{embeddingNote(embedding)}</p>
			{/if}
			{#if off.length}
				<p class="off-note">
					{off.length === sources.length ? 'All' : off.length} switched off.
					<button class="link" onclick={() => onexclude(off, false)}>Switch all on</button>
				</p>
			{/if}
			<button class="add" onclick={pick(onadd)} aria-label="Add sources"><span aria-hidden="true">+</span> Add a source</button>
		</section>

		{#if sources.length}
			<section class="part">
				<h2>Study</h2>
				{#if studies.length || writing.length}
					<ul>
						{#each studies as set (set.id)}
							<li class:active={set.id === active}>
								<button class="item" aria-current={set.id === active ? 'true' : undefined} onclick={() => pick(onstudy)(set)}>
									<span class="title">{set.title}</span>
									<span class="meta">{studyMeta(set)}</span>
								</button>
							</li>
						{/each}
						{#each writing as kind, i (i)}
							<li class="pending">
								<span class="title">{WRITING[kind]}</span>
								<span class="progress" aria-hidden="true"></span>
							</li>
						{/each}
					</ul>
				{/if}
				<p class="adds">
					<button class="add" onclick={pick(() => onnewstudy('flashcards'))}><span aria-hidden="true">+</span> Flashcards</button>
					<button class="add" onclick={pick(() => onnewstudy('quiz'))}><span aria-hidden="true">+</span> Quiz</button>
					<button class="add" onclick={pick(() => onnewstudy('mindmap'))}><span aria-hidden="true">+</span> Mind map</button>
					<button class="add" onclick={pick(() => onnewstudy('table'))}><span aria-hidden="true">+</span> Table</button>
					<button class="add" onclick={pick(() => onnewstudy('slides'))}><span aria-hidden="true">+</span> Slides</button>
					<button class="add" onclick={pick(() => onnewstudy('infographic'))}
						><span aria-hidden="true">+</span> Infographic</button
					>
					<button class="add" onclick={pick(() => onnewstudy('audio'))}
						><span aria-hidden="true">+</span> Audio overview</button
					>
				</p>
			</section>

			<section class="part">
				<h2>Notes</h2>
				{#if notes.length || reports.length}
					<ul>
						{#each notes as note (note.id)}
							<li class:active={note.id === active}>
								<button class="item" aria-current={note.id === active ? 'true' : undefined} onclick={() => pick(onnote)(note.id)}>
									<span class="title">{note.title}</span>
								</button>
							</li>
						{/each}
						{#each reports as name, i (i)}
							<li class="pending">
								<span class="title">Writing a {name}…</span>
								<span class="progress" aria-hidden="true"></span>
							</li>
						{/each}
					</ul>
				{/if}
				<p class="adds">
					<button class="add" onclick={pick(onnewnote)}><span aria-hidden="true">+</span> New note</button>
					<button class="add" onclick={pick(onnewreport)}><span aria-hidden="true">+</span> Report</button>
				</p>
			</section>

			{#if chats.length}
				<section class="part">
					<h2>Conversations</h2>
					<ul>
						{#each chats as summary (summary.id)}
							<li class:active={summary.id === chat}>
								<button
									class="item"
									aria-current={summary.id === chat ? 'page' : undefined}
									onclick={() => pick(onchat)(summary.id)}
								>
									<span class="title">{summary.title}</span>
									<span class="meta">{count(summary.turns, 'question', 'questions')}</span>
								</button>
							</li>
						{/each}
					</ul>
					{#if chat !== null}
						<button class="add" onclick={pick(onnewchat)}><span aria-hidden="true">+</span> New conversation</button>
					{/if}
				</section>
			{/if}
		{/if}
		<button class="toggle" bind:this={toggle} aria-expanded={open} onclick={() => (open = !open)}>
			{open ? 'Close' : 'Contents'}
		</button>
	{/if}
</aside>

<style>
	.contents {
		font-size: var(--t-s);
	}
	.scrim,
	.toggle {
		display: none;
	}
	.part + .part {
		margin-top: 2.25rem;
	}
	h2 {
		margin: 0 0 0.75rem;
		font-size: var(--t-s);
		font-style: italic;
		font-weight: 400;
		color: var(--ink-2);
	}
	ol,
	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.sources-part li {
		display: flex;
		gap: 0.5rem;
		align-items: flex-start;
	}
	.sources-part li + li {
		margin-top: 0.15rem;
	}
	.source {
		display: grid;
		flex: 1;
		grid-template-columns: 1.75rem minmax(0, 1fr);
		min-width: 0;
		padding: 0.4rem 0;
		border: 0;
		background: none;
		text-align: left;
		cursor: pointer;
	}
	/* The box is small; the label around it is the target, 24px and larger
	   for fingers, without moving the box. */
	.switch {
		display: grid;
		place-items: center;
		margin: 0.15rem -0.3rem 0;
		padding: 0.3rem;
		cursor: pointer;
	}
	@media (pointer: coarse) {
		.switch {
			margin: -0.35rem -0.85rem 0;
			padding: 0.85rem;
		}
	}
	/* On is the usual state, so it stays quiet; a switched-off source shows
	   an empty box and grey ink. */
	.check.quiet:checked {
		background: none;
		box-shadow: inset 0 0 0 1px var(--edge);
	}
	.check.quiet:checked::after {
		border-color: var(--ink-2);
	}
	.siglum {
		color: var(--rubric);
		font-size: var(--t-l);
		font-style: italic;
		font-weight: 600;
		line-height: 1.1;
	}
	li.off .siglum,
	li.off .title {
		color: var(--ink-2);
	}
	li.off .siglum {
		font-weight: 400;
	}
	.pending {
		display: grid;
		grid-template-columns: 1.75rem minmax(0, 1fr);
		padding: 0.4rem 0;
	}
	.part:not(.sources-part) .pending {
		display: block;
	}
	.info {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}
	.title {
		display: -webkit-box;
		overflow: hidden;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		line-height: 1.3;
		overflow-wrap: anywhere;
	}
	.item {
		display: flex;
		flex-direction: column;
		width: 100%;
		padding: 0.35rem 0;
		border: 0;
		background: none;
		text-align: left;
		cursor: pointer;
	}
	.source:hover .title,
	.item:hover .title,
	li.active .title {
		color: var(--rubric);
	}
	li.active .title {
		text-decoration: underline;
		text-decoration-thickness: 1px;
		text-underline-offset: 0.2em;
	}
	.meta,
	.error,
	.index-note,
	.off-note {
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.error {
		color: var(--danger);
	}
	.index-note,
	.off-note {
		margin: 0.5rem 0 0;
	}
	.link {
		padding: 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: inherit;
		font-style: normal;
		font-weight: 600;
		cursor: pointer;
	}
	/* Work in progress draws a hairline across, once per pass. */
	.progress {
		display: block;
		height: 1px;
		margin-top: 0.35rem;
		background: linear-gradient(var(--rubric), var(--rubric)) no-repeat left / 35% 100%, var(--line);
		animation: draw 1.6s var(--ease) infinite;
	}
	@keyframes draw {
		from {
			background-size: 0 100%, auto;
		}
		to {
			background-size: 100% 100%, auto;
		}
	}
	li.fresh .title {
		animation: arrive 900ms var(--ease);
	}
	@keyframes arrive {
		from {
			color: var(--rubric);
		}
	}
	.adds {
		display: flex;
		flex-wrap: wrap;
		gap: 0 1.25rem;
		margin: 0;
	}
	.add {
		margin-top: 0.6rem;
		padding: 0.25rem 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: var(--t-s);
		font-style: italic;
		cursor: pointer;
	}
	.adds .add {
		margin-top: 0.15rem;
	}
	.sources-part .add span {
		display: inline-block;
		width: 1.75rem;
		font-style: normal;
	}

	/* Without room for a contents column, the contents open as a drawer from
	   the running head. */
	@media (min-width: 720px) and (max-width: 1239px) {
		.contents:not(.open) {
			display: none;
		}
		.contents.open {
			position: fixed;
			inset: 0 auto 0 0;
			z-index: 11;
			width: min(22rem, 86vw);
			padding: 1.5rem 1.5rem 2rem;
			overflow-y: auto;
			border-right: 1px solid var(--line);
			background: var(--raised);
			animation: from-left 180ms var(--ease);
		}
		@keyframes from-left {
			from {
				transform: translateX(-1.5rem);
				opacity: 0;
			}
		}
		.scrim {
			display: block;
			position: fixed;
			inset: 0;
			z-index: 10;
			border: 0;
			background: var(--scrim);
		}
		.open .toggle {
			display: block;
			position: absolute;
			top: 1.1rem;
			right: 1.25rem;
			padding: 0.25rem;
			border: 0;
			background: none;
			color: var(--ink-2);
			font-size: var(--t-xs);
			font-style: italic;
			cursor: pointer;
		}
	}

	/* Phones: the sigla in a row along the bottom, in thumb reach. The rest of
	   the contents opens as a sheet from the same strip. */
	@media (max-width: 719px) {
		.contents:not(.open) {
			position: fixed;
			inset: auto 0 0;
			z-index: 4;
			display: flex;
			align-items: center;
			gap: 0.25rem;
			height: var(--strip, 3.4rem);
			padding: 0 0.75rem env(safe-area-inset-bottom);
			border-top: 1px solid var(--line);
			background: var(--page);
		}
		.contents:not(.open) .part:not(.sources-part),
		.contents:not(.open) h2,
		.contents:not(.open) .info,
		.contents:not(.open) .switch,
		.contents:not(.open) .off-note {
			display: none;
		}
		.contents:not(.open) .sources-part {
			display: flex;
			flex: 1;
			align-items: center;
			min-width: 0;
			overflow-x: auto;
		}
		.contents:not(.open) ol {
			display: flex;
			gap: 0.25rem;
		}
		.contents:not(.open) .sources-part li + li {
			margin-top: 0;
		}
		.contents:not(.open) .source,
		.contents:not(.open) .pending {
			display: grid;
			place-items: center;
			grid-template-columns: none;
			width: 2.75rem;
			height: 2.75rem;
			padding: 0;
		}
		.contents:not(.open) li.active .source {
			box-shadow: inset 0 -2px 0 var(--rubric);
		}
		.contents:not(.open) .pending .siglum::before {
			content: '…';
		}
		.contents:not(.open) .add {
			flex: none;
			margin: 0 0 0 auto;
			padding: 0.5rem 0.25rem;
			white-space: nowrap;
		}
		.contents:not(.open) .add span {
			width: auto;
			margin-right: 0.25rem;
		}
		.toggle {
			display: block;
			flex: none;
			padding: 0.5rem 0.25rem 0.5rem 0.75rem;
			border: 0;
			background: none;
			color: var(--ink);
			font-size: var(--t-s);
			font-style: italic;
			cursor: pointer;
		}
		.open .toggle {
			position: absolute;
			top: 1rem;
			right: 1rem;
		}
		.scrim {
			display: block;
			position: fixed;
			inset: 0;
			z-index: 10;
			border: 0;
			background: var(--scrim);
		}
		.contents.open {
			position: fixed;
			inset: auto 0 0;
			z-index: 11;
			max-height: 86dvh;
			padding: 1.25rem 1.25rem calc(1.5rem + env(safe-area-inset-bottom));
			overflow-y: auto;
			border-radius: 14px 14px 0 0;
			background: var(--raised);
			box-shadow: 0 -1px 0 var(--line);
			animation: rise 180ms var(--ease);
		}
		@keyframes rise {
			from {
				transform: translateY(2rem);
				opacity: 0;
			}
		}
	}
</style>
