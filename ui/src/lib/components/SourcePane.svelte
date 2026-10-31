<script lang="ts" module>
	import type { Citation, Passage } from '#lib/api.ts';

	/** One cited passage and the citations that point into it. */
	export type Entry = {
		label: string;
		sourceId: string;
		citations: Citation[];
		passage: Passage | null;
		error: string | null;
	};
</script>

<script lang="ts">
	import { tick } from 'svelte';
	import { api, describe, type Figure, type Heading, type Source } from '#lib/api.ts';
	import { clock, readingNote, sourceMeta, transcribingNote } from '#lib/format.ts';
	import { markdownBlocks, type Run, type Span } from '#lib/passage.ts';
	import { quoteRange } from '#lib/quote.ts';
	import FigureImage from './FigureImage.svelte';
	import PageView from './PageView.svelte';
	import Sheet from './Sheet.svelte';

	let {
		slug,
		source,
		entries = null,
		modal = false,
		onclose,
		onback,
		backLabel = '',
		onrename,
		onremove,
		onexclude
	}: {
		slug: string;
		source: Source;
		/** Cited passages to show. Without them the pane shows the source itself. */
		entries?: Entry[] | null;
		/** On phones the pane is a bottom sheet over the page. */
		modal?: boolean;
		onclose: () => void;
		onback?: () => void;
		backLabel?: string;
		/** Each resolves to an error message, or null when it worked. */
		onrename: (title: string) => Promise<string | null>;
		onremove: () => Promise<string | null>;
		onexclude: (excluded: boolean) => void;
	} = $props();

	// PDF text keeps the page's hard line breaks, and PDFium marks line-break
	// hyphens as U+0002. Join both for reading, keeping string length so quote
	// offsets still line up.
	const readable = (text: string) =>
		text
			.replace(/[-\u0002]\n/g, '​​')
			.replace(/\u0002/g, '​')
			.replace(/(?<!\n)\n(?!\n)/g, ' ');

	/** Where the verified quotes sit in the passage, overlaps merged. */
	function hitRanges(text: string, citations: Citation[]): Span[] {
		const ranges = citations
			.filter((c) => c.verified)
			.map((c) => quoteRange(text, c.quote))
			.filter((r): r is Span => r !== null)
			.sort((a, b) => a[0] - b[0]);
		const merged: Span[] = [];
		for (const [start, end] of ranges) {
			const last = merged.at(-1);
			if (last && start <= last[1]) last[1] = Math.max(last[1], end);
			else merged.push([start, end]);
		}
		return merged;
	}

	function segments(text: string, citations: Citation[]): { text: string; hit: boolean }[] {
		const merged = hitRanges(text, citations);
		const shown = readable(text);
		const out: { text: string; hit: boolean }[] = [];
		let at = 0;
		for (const [start, end] of merged) {
			if (start > at) out.push({ text: shown.slice(at, start), hit: false });
			out.push({ text: shown.slice(start, end), hit: true });
			at = end;
		}
		if (at < shown.length) out.push({ text: shown.slice(at), hit: false });
		return out;
	}

	function where(passage: Passage | null): string {
		if (!passage) return '';
		const { page_start: a, page_end: b } = passage;
		const pages = a && b && a !== b ? `Pages ${a}–${b}` : a ? `Page ${a}` : '';
		return [pages, passage.section].filter(Boolean).join(', ');
	}

	const single = $derived(entries?.length === 1);
	/** The passage whose printed page is shown, by label; 'source' while reading the source. */
	let pageFor = $state<string | null>(null);
	const isPdf = $derived(source.kind === 'pdf');
	// Sources stored as Markdown; PDF and plain text are shown as written.
	const isMarkdown = $derived(['markdown', 'web', 'docx', 'epub', 'youtube', 'audio'].includes(source.kind));
	const isAudio = $derived(source.kind === 'audio');

	// A recording plays above its transcript. Keyed on the id, so the
	// notebook refreshing while a transcript is written doesn't restart it.
	let audioUrl = $state<string | null>(null);
	let audioProblem = $state<string | null>(null);
	let player = $state<HTMLAudioElement>();
	$effect(() => {
		if (!isAudio) return;
		let url: string | null = null;
		let gone = false;
		audioProblem = null;
		api.sourceFile(slug, sourceId)
			.then((bytes) => {
				if (gone) return;
				url = URL.createObjectURL(new Blob([bytes]));
				audioUrl = url;
			})
			.catch((e) => {
				if (!gone) audioProblem = describe(e);
			});
		return () => {
			gone = true;
			if (url) URL.revokeObjectURL(url);
			audioUrl = null;
		};
	});

	const CLOCK = /\[(\d+):(\d\d)(?::(\d\d))?\]/g;
	/** The second a passage of a transcript starts at: its paragraph's time. */
	function startOf(passage: Passage): number | null {
		const at = (m: RegExpMatchArray) =>
			m[3] ? +m[1] * 3600 + +m[2] * 60 + +m[3] : +m[1] * 60 + +m[2];
		const inText = [...passage.text.matchAll(CLOCK)];
		if (inText[0] && !passage.text.slice(0, inText[0].index).trim()) return at(inText[0]);
		const before = [...(passage.before ?? '').matchAll(CLOCK)].at(-1);
		return before ? at(before) : inText[0] ? at(inText[0]) : null;
	}

	function play(start: number) {
		if (!player) return;
		player.currentTime = start;
		player.play();
	}

	let body = $state<HTMLElement>();

	// Cited passages: bring the first highlight into view once it loads.
	const loaded = $derived(entries?.filter((e) => e.passage).length ?? 0);
	$effect(() => {
		if (!loaded) return;
		tick().then(() => body?.querySelector('mark')?.scrollIntoView({ block: 'center' }));
	});

	// The source itself: its contents and one passage at a time.
	let outline = $state<Heading[] | null>(null);
	let contentsOpen = $state(false);
	let ord = $state(0);
	let passage = $state<Passage | null>(null);
	let problem = $state<string | null>(null);

	async function read(n: number) {
		const id = source.id;
		ord = n;
		passage = null;
		problem = null;
		try {
			const next = await api.passage(slug, id, n);
			if (source.id !== id) return;
			passage = next;
			if (body) body.scrollTop = 0;
		} catch (e) {
			problem = describe(e);
		}
	}

	// Keyed on id and size, not the source object, so a rename keeps the place.
	const sourceId = $derived(source.id);
	const chunks = $derived(source.chunks);
	$effect(() => {
		if (entries || chunks === 0) return;
		const id = sourceId;
		outline = null;
		api.outline(slug, id)
			.then((headings) => {
				if (source.id !== id) return;
				outline = headings;
				contentsOpen = headings.length <= 12;
			})
			.catch(() => (outline = []));
		read(0);
	});

	let renaming = $state(false);
	let newTitle = $state('');
	let confirming = $state(false);
	let actionError = $state<string | null>(null);
	let renameField = $state<HTMLInputElement>();

	async function startRename() {
		newTitle = source.title;
		renaming = true;
		actionError = null;
		await tick();
		renameField?.select();
	}

	async function saveRename(event: SubmitEvent) {
		event.preventDefault();
		actionError = await onrename(newTitle.trim());
		if (!actionError) renaming = false;
	}

	async function remove() {
		actionError = await onremove();
	}
</script>

{#snippet styled(run: Run)}{#if run.code}<code>{run.text}</code>{:else if run.strong && run.em}<strong
			><em>{run.text}</em></strong
		>{:else if run.strong}<strong>{run.text}</strong>{:else if run.em}<em>{run.text}</em>{:else}{run.text}{/if}{/snippet}

{#snippet inline(runs: Run[])}{#each runs as run, i (i)}{#if run.hit}<mark>{@render styled(run)}</mark
		>{:else}{@render styled(run)}{/if}{/each}{/snippet}

{#snippet markdown(text: string, hits: Span[], figures: Figure[])}
	<div class="passage md">
		{#each markdownBlocks(text, hits, figures) as block, i (i)}
			{#if block.kind === 'figure'}
				<figure class="md-figure" style:max-width="{figures[block.index].width * 2}px">
					<FigureImage {slug} sourceId={source.id} figure={figures[block.index]} />
				</figure>
			{:else if block.kind === 'heading'}
				<svelte:element this={`h${Math.min(block.level + 2, 6)}`} class="md-heading">{@render inline(block.runs)}</svelte:element>
			{:else if block.kind === 'item'}
				<p class="md-item" style:--depth={block.depth}>
					<span class="marker" aria-hidden="true">{block.marker}</span>{@render inline(block.runs)}
				</p>
			{:else if block.kind === 'quote'}
				<blockquote>{@render inline(block.runs)}</blockquote>
			{:else if block.kind === 'code'}
				<pre>{@render inline(block.runs)}</pre>
			{:else if block.kind === 'table'}
				<div class="md-table">
					<table>
						{#if block.rows.length}
							<thead><tr>{#each block.rows[0] as cell, c (c)}<th>{@render inline(cell)}</th>{/each}</tr></thead>
						{/if}
						<tbody>
							{#each block.rows.slice(1) as row, r (r)}
								<tr>{#each row as cell, c (c)}<td>{@render inline(cell)}</td>{/each}</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{:else}
				<p>{@render inline(block.runs)}</p>
			{/if}
		{/each}
	</div>
{/snippet}

<Sheet
	label={entries ? `Cited passages from ${source.title}` : source.title}
	mark={source.letter}
	{modal}
	closeLabel={entries ? 'Close passage' : 'Close source'}
	{onclose}
	{onback}
	{backLabel}
	bind:body
>
	{#snippet heading()}
		{#if renaming}
			<form class="rename" onsubmit={saveRename}>
				<label class="visually-hidden" for="source-title">Source name</label>
				<input
					id="source-title"
					bind:value={newTitle}
					bind:this={renameField}
					required
					onkeydown={(e) => {
						if (e.key === 'Escape') {
							e.preventDefault();
							renaming = false;
						}
					}}
				/>
				<button class="action" disabled={!newTitle.trim()}>Save</button>
				<button type="button" class="action quiet" onclick={() => (renaming = false)}>Cancel</button>
			</form>
		{:else}
			<h2>{source.title}</h2>
		{/if}
		{#if entries}
			<p class="where">{single ? where(entries[0].passage) : `${entries.length} passages`}</p>
		{:else}
			<p class="where">
				{sourceMeta(source.kind, source.pages)}{#if source.url},
					<a href={source.url} target="_blank" rel="noopener noreferrer">open the page</a>{/if}
			</p>
			<label class="use">
				<input class="check" type="checkbox" checked={!source.excluded} onchange={(e) => onexclude(!e.currentTarget.checked)} />
				Use in answers and study sets
			</label>
			{#if !renaming && !confirming}
				<p class="actions">
					<button class="action" onclick={startRename}>Rename</button>
					<button class="action" onclick={() => ((confirming = true), (actionError = null))}>Remove</button>
				</p>
			{/if}
		{/if}
	{/snippet}
	{#snippet top()}
		{#if confirming}
			<div class="confirm" role="alert">
				<p>Remove {source.title} from this notebook? Answers that cite it keep their text.</p>
				<p class="actions">
					<button class="action danger" onclick={remove}>Remove source</button>
					<button class="action quiet" onclick={() => (confirming = false)}>Cancel</button>
				</p>
			</div>
		{/if}
		{#if actionError}<p class="problem padded" role="alert">{actionError}</p>{/if}
		{#if audioProblem}<p class="problem padded" role="alert">{audioProblem}</p>{/if}
		{#if isAudio && audioUrl}
			<audio class="recording" bind:this={player} src={audioUrl} controls preload="metadata"></audio>
		{/if}
	{/snippet}

	<div class="text">
		{#if entries}
			{#each entries as entry (entry.label)}
				<section class="entry">
					{#if !single && entry.passage}<p class="entry-where">{where(entry.passage)}</p>{/if}
					{#each entry.citations.filter((c) => !c.verified) as citation (citation.quote)}
						<p class="problem">
							The answer quoted words that aren't in this passage: “{citation.quote}”. Treat that sentence as
							unsupported.
						</p>
					{/each}
					{#if entry.error}
						<p class="problem">{entry.error}</p>
					{:else if !entry.passage}
						<p class="status">Loading the passage…</p>
					{:else}
						{#if isMarkdown}
							{#if single && entry.passage.before}{@render markdown(entry.passage.before, [], [])}{/if}
							{@render markdown(entry.passage.text, hitRanges(entry.passage.text, entry.citations), entry.passage.figures)}
							{#if single && entry.passage.after}{@render markdown(entry.passage.after, [], [])}{/if}
						{:else}
							{#if single && entry.passage.before}<p>{readable(entry.passage.before)}</p>{/if}
							<p class="passage">
								{#each segments(entry.passage.text, entry.citations) as part, i (i)}{#if part.hit}<mark>{part.text}</mark
										>{:else}{part.text}{/if}{/each}
							</p>
							{#if single && entry.passage.after}<p>{readable(entry.passage.after)}</p>{/if}
						{/if}
						{@const start = isAudio && audioUrl ? startOf(entry.passage) : null}
						{#if start !== null}
							<button class="action" onclick={() => play(start)}>Play from {clock(start)}</button>
						{/if}
						{@const pageNumber = entry.citations[0].page ?? entry.passage.page_start}
						{#if isPdf && pageNumber}
							<button class="action" onclick={() => (pageFor = pageFor === entry.label ? null : entry.label)}>
								{pageFor === entry.label ? 'Hide the page' : `See page ${pageNumber} as printed`}
							</button>
							{#if pageFor === entry.label}
								<PageView
									{slug}
									sourceId={source.id}
									page={pageNumber}
									quotes={entry.citations.filter((c) => c.verified).map((c) => c.quote)}
								/>
							{/if}
						{/if}
					{/if}
				</section>
			{/each}
		{:else if source.transcribing}
			<p class="status" role="status">{transcribingNote(source.transcribing)}. The transcript appears here when it's done.</p>
		{:else if source.reading}
			<p class="status" role="status">{readingNote(source.reading)}. The text appears here when it's done.</p>
		{:else if source.error}
			<p class="problem">Bowerbird couldn't read this source. {source.error}</p>
		{:else if source.chunks === 0}
			<p class="status">This source has no text Bowerbird can read.</p>
		{:else}
			{#if outline && outline.length > 1}
				<details class="contents" bind:open={contentsOpen}>
					<summary>Contents</summary>
					<ol>
						{#each outline as heading, i (i)}
							<li style:--depth={heading.depth}>
								<button
									class="heading-link"
									onclick={() => {
										contentsOpen = false;
										read(heading.ord);
									}}>{heading.title}</button
								>
							</li>
						{/each}
					</ol>
				</details>
			{/if}
			{#if problem}
				<p class="problem">{problem}</p>
			{:else if !passage}
				<p class="status">Loading the passage…</p>
			{:else}
				<p class="entry-where">{where(passage)}</p>
				{#if isMarkdown}
					{@render markdown(passage.text, [], passage.figures)}
				{:else}
					<p class="passage">{readable(passage.text)}</p>
				{/if}
				{@const start = isAudio && audioUrl ? startOf(passage) : null}
				{#if start !== null}
					<button class="action" onclick={() => play(start)}>Play from {clock(start)}</button>
				{/if}
				{#if isPdf && passage.page_start}
					<button class="action" onclick={() => (pageFor = pageFor === 'source' ? null : 'source')}>
						{pageFor === 'source' ? 'Hide the page' : `See page ${passage.page_start} as printed`}
					</button>
					{#if pageFor === 'source'}
						{#key passage.page_start}
							<PageView {slug} sourceId={source.id} page={passage.page_start} quotes={[]} />
						{/key}
					{/if}
				{/if}
			{/if}
			<nav class="pager" aria-label="Passages">
				<button class="action" disabled={ord === 0} onclick={() => read(ord - 1)}>Previous</button>
				<span>Passage {ord + 1} of {source.chunks}</span>
				<button class="action" disabled={ord + 1 >= source.chunks} onclick={() => read(ord + 1)}>Next</button>
			</nav>
		{/if}
	</div>
</Sheet>

<style>
	h2 {
		margin: 0;
		font-size: var(--t-l);
		font-style: italic;
		font-weight: 400;
		font-variation-settings: 'opsz' 36;
		line-height: 1.3;
		overflow-wrap: anywhere;
	}
	.where,
	.entry-where {
		margin: 0.3rem 0 0;
		font-size: var(--t-xs);
		color: var(--ink-2);
	}
	.actions {
		display: flex;
		gap: 1rem;
		margin: 0.5rem 0 0;
	}
	.action {
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: var(--t-xs);
		font-weight: 600;
		cursor: pointer;
	}
	.action:disabled {
		color: var(--ink-2);
		font-weight: 400;
		cursor: default;
	}
	.action.quiet {
		color: var(--ink-2);
		font-weight: 400;
	}
	.action.danger {
		color: var(--danger);
	}
	.text > .action,
	.entry > .action {
		display: block;
		margin: -0.5rem 0 1rem;
	}
	.rename {
		display: flex;
		gap: 0.75rem;
		align-items: center;
	}
	.rename input {
		flex: 1;
		min-width: 0;
		padding: calc(0.3rem - var(--cap-lift)) 0.5rem calc(0.3rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--page);
		box-shadow: inset 0 0 0 1px var(--edge);
		font-size: var(--t-m);
		font-style: italic;
	}
	.confirm {
		padding: 1rem 1.5rem;
		border-bottom: 1px solid var(--line);
		font-size: var(--t-s);
	}
	.confirm p {
		margin: 0;
	}
	.padded {
		margin: 0;
		padding: 0.75rem 1.5rem 0;
	}
	.recording {
		display: block;
		width: calc(100% - 3rem);
		margin: 0.75rem 1.5rem;
	}

	.text p {
		margin: 0 0 1em;
	}
	.use {
		display: flex;
		gap: 0.45rem;
		align-items: center;
		margin: 0.5rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
		cursor: pointer;
	}
	.use:has(:checked) {
		color: var(--ink);
	}
	.entry + .entry {
		padding-top: 1rem;
		border-top: 1px solid var(--line);
	}
	.entry-where {
		margin: 0 0 0.5rem;
	}
	.status {
		color: var(--ink-2);
	}
	/* Saved at up to twice the size the page showed, so up to that width. */
	.md-figure {
		margin: 0.75rem 0 0.5rem;
	}
	.md-figure :global(img) {
		width: 100%;
	}
	.md-heading {
		margin: 1.25em 0 0.5em;
		font-size: var(--t-m);
		font-weight: 600;
		line-height: 1.3;
	}
	h3.md-heading {
		font-size: var(--t-l);
	}
	.md-heading:first-child {
		margin-top: 0;
	}
	.md-item {
		position: relative;
		padding-left: calc(1.3em + var(--depth) * 1.3em);
	}
	.text .md-item {
		margin-bottom: 0.4em;
	}
	.marker {
		position: absolute;
		left: calc(var(--depth) * 1.3em);
		color: var(--rubric);
	}
	blockquote {
		margin: 0 0 1em;
		padding-left: 1em;
		border-left: 2px solid var(--line);
		color: var(--ink-2);
	}
	pre {
		margin: 0 0 1em;
		white-space: pre-wrap;
	}
	pre,
	code {
		font-family: ui-monospace, monospace;
		font-size: 0.85em;
	}
	.md-table {
		margin: 0 0 1em;
		overflow-x: auto;
	}
	table {
		border-collapse: collapse;
		font-size: var(--t-xs);
	}
	th,
	td {
		padding: 0.35em 0.75em 0.35em 0;
		border-bottom: 1px solid var(--line);
		text-align: left;
		vertical-align: top;
	}
	th {
		font-weight: 600;
	}
	mark {
		padding: 0.1em 0;
		background: var(--highlight);
		box-shadow: inset 0 -1px 0 var(--rubric);
		color: inherit;
	}
	.problem {
		font-style: italic;
		color: var(--danger);
	}
	.contents {
		margin-bottom: 1.25rem;
		padding-bottom: 1rem;
		border-bottom: 1px solid var(--line);
	}
	.contents summary {
		color: var(--rubric);
		font-size: var(--t-xs);
		font-style: italic;
		cursor: pointer;
	}
	.contents ol {
		margin: 0.5rem 0 0;
		padding: 0;
		list-style: none;
	}
	.contents li {
		padding-left: calc((var(--depth) - 1) * 0.9rem);
	}
	.heading-link {
		padding: 0.15rem 0;
		border: 0;
		background: none;
		color: var(--ink);
		font-size: var(--t-xs);
		line-height: 1.4;
		text-align: left;
		cursor: pointer;
	}
	.heading-link:hover {
		color: var(--rubric);
	}
	.pager {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		padding-top: 0.75rem;
		border-top: 1px solid var(--line);
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}

	@media (max-width: 719px) {
		h2 {
			display: -webkit-box;
			overflow: hidden;
			-webkit-box-orient: vertical;
			-webkit-line-clamp: 2;
			line-clamp: 2;
			font-size: var(--t-m);
		}
	}
</style>
