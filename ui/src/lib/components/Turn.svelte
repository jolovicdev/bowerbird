<script lang="ts">
	import type { Citation, Reading, Source, Turn } from '#lib/api.ts';
	import { seconds } from '#lib/format.ts';
	import CitedText from './CitedText.svelte';
	import FigureImage from './FigureImage.svelte';

	let {
		slug,
		turn,
		number,
		sources,
		active,
		live = false,
		fill = false,
		error = null,
		stopped = false,
		reading = null,
		since = 0,
		onopen,
		onretry,
		onsave,
		onopennote
	}: {
		slug: string;
		turn: Turn;
		/** Position in the conversation, shown as its section number. */
		number: number;
		sources: Map<string, Source>;
		active: Citation[] | null;
		/** The answer is still streaming in. */
		live?: boolean;
		/** Asked just now: take a screen's height, so the question can sit at
		 * the top while its answer arrives below it. */
		fill?: boolean;
		error?: string | null;
		/** The reader stopped the answer. */
		stopped?: boolean;
		/** Passages the live answer is written from, once the server says. */
		reading?: Reading | null;
		/** When the live question was sent, in milliseconds since the epoch. */
		since?: number;
		onopen: (citations: Citation[], el: HTMLElement) => void;
		onretry?: () => void;
		/** Saves the answer as a note. Resolves to an error message, or null when it worked. */
		onsave?: () => Promise<string | null>;
		/** Opens the note this answer was saved to. */
		onopennote?: () => void;
	} = $props();

	let saved = $state(false);
	let saveError = $state<string | null>(null);
	async function save() {
		if (!onsave) return;
		saveError = await onsave();
		saved = saveError === null;
	}

	const missing = $derived(turn.citations.filter((c) => !c.verified).length);
	const total = $derived(turn.citations.length);
	const writing = $derived(live && !error && !stopped);

	let now = $state(Date.now());
	$effect(() => {
		if (!writing) return;
		now = Date.now();
		const timer = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(timer);
	});
	const elapsed = $derived(Math.max(0, Math.round((now - since) / 1000)));

	const list = (letters: string[]) =>
		letters.length < 3 ? letters.join(' and ') : `${letters.slice(0, -1).join(', ')} and ${letters.at(-1)}`;

	let article: HTMLElement;
	let plates = $state<HTMLElement>();
	let tail = $state<HTMLElement>();
	// Pictures arrive with the finished answer and can land under the composer,
	// below text the reader followed to its end. Bring the end up then; a reader
	// who scrolled back up stays where they are.
	$effect(() => {
		if (fill && plates && tail && plates.getBoundingClientRect().top < innerHeight) {
			tail.scrollIntoView({ block: 'nearest' });
		}
	});

	let nextUnverified = 0;
	function showUnverified() {
		const marks = article.querySelectorAll<HTMLElement>('.mark.unverified');
		if (!marks.length) return;
		const mark = marks[nextUnverified++ % marks.length];
		mark.scrollIntoView({ block: 'center' });
		mark.focus();
	}
</script>

<article class="turn" class:fill aria-busy={writing} bind:this={article}>
	<h2 class="question"><span class="number">§ {number}</span> {turn.question}</h2>
	<CitedText pieces={turn.pieces} citations={turn.citations} {sources} {active} {onopen} />
	{#if turn.figures?.length}
		<div class="plates" bind:this={plates}>
			{#each turn.figures as figure (figure.file)}
				{@const citation = turn.citations[figure.citation ?? 0]}
				{#if citation?.source_id}
					<button
						class="plate"
						aria-label="{figure.caption || 'Picture'}, from source {citation.letter}"
						onclick={(e) => onopen([citation], e.currentTarget)}
					>
						<FigureImage {slug} sourceId={citation.source_id} {figure} />
						<span class="caption"><span class="siglum">{citation.letter}</span> {figure.caption}</span>
					</button>
				{/if}
			{/each}
		</div>
	{/if}
	{#if error}
		<p class="meta problem" role="alert">
			{error}
			{#if error.includes('Settings')}<a class="action" href="/settings">Open Settings</a>{/if}
			{#if onretry}<button class="action" onclick={onretry}>Try again</button>{/if}
		</p>
	{:else if stopped}
		<p class="meta">
			Stopped.
			{#if onretry}<button class="action" onclick={onretry}>Try again</button>{/if}
		</p>
	{:else if live}
		<p class="meta live">
			<span class="dot" aria-hidden="true"></span>
			{#if turn.pieces.length}
				Writing, {elapsed} s
			{:else if reading}
				Reading {reading.count === 1 ? '1 passage' : `${reading.count} passages`} from {list(reading.sources)}, {elapsed} s
			{:else}
				Finding passages, {elapsed} s
			{/if}
		</p>
	{:else}
		<p class="meta tail" bind:this={tail}>
			{turn.model}{#if turn.deep}, Deep search{/if}, {seconds(turn.timings.total_ms)} s.
			{#if total === 0}
				No citations.
			{:else if missing === 0}
				{total === 1 ? 'Its quote was' : `All ${total} quotes were`} found in the sources.
			{:else}
				<button class="action problem" onclick={showUnverified}>
					{missing} of {total} quotes weren't found in the sources.
				</button>
			{/if}
			{#if onsave && saved && onopennote}
				<button class="action" onclick={onopennote}>Saved; open the note</button>
			{:else if onsave}
				<button class="action" onclick={save}>Save to note</button>
			{/if}
		</p>
		{#if saveError}<p class="meta problem" role="alert">{saveError}</p>{/if}
	{/if}
</article>

<style>
	.turn {
		position: relative;
	}
	/* Pictures from the cited passages, all one height and never cropped. */
	.plates {
		display: flex;
		flex-wrap: wrap;
		gap: 1rem 1.25rem;
		margin: 1.25rem 0 0;
	}
	.plate {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		width: min-content;
		padding: 0;
		border: 0;
		background: none;
		color: inherit;
		text-align: left;
		cursor: pointer;
	}
	.plate :global(img) {
		height: 8rem;
		width: auto;
		max-width: none;
	}
	.caption {
		display: -webkit-box;
		overflow: hidden;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
		line-height: 1.35;
	}
	.plate:hover .caption {
		color: var(--ink);
	}
	.caption .siglum {
		margin-right: 0.15em;
		color: var(--rubric);
		font-weight: 600;
	}
	/* Phones: one row to swipe through. */
	@media (max-width: 719px) {
		.plates {
			flex-wrap: nowrap;
			overflow-x: auto;
			padding-bottom: 0.25rem;
		}
		.plate :global(img) {
			height: 6.5rem;
		}
	}
	.turn.fill {
		min-height: calc(100dvh - var(--reserved, 11rem));
	}
	.turn + :global(.turn) {
		margin-top: 3rem;
		padding-top: 2.5rem;
		border-top: 1px solid var(--line);
	}
	.question {
		position: relative;
		margin: 0 0 1rem;
		font-size: var(--t-xl);
		font-style: italic;
		font-weight: 400;
		font-variation-settings: 'opsz' 48;
		line-height: 1.3;
		text-wrap: balance;
	}
	/* Section numbers hang in the gutter, like verse numbers in a margin. */
	.number {
		position: absolute;
		top: 0.45em;
		right: calc(100% + 1rem);
		color: var(--rubric);
		font-size: var(--t-xs);
		font-style: normal;
		white-space: nowrap;
	}
	.meta {
		margin: 0.75rem 0 0;
		font-size: var(--t-xs);
		color: var(--ink-2);
	}
	.problem {
		color: var(--danger);
	}
	.action {
		margin-left: 0.6rem;
		padding: 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: inherit;
		font-style: normal;
		font-weight: 600;
		text-decoration: none;
		cursor: pointer;
	}
	.action:disabled {
		color: var(--ink-2);
		font-weight: 400;
		cursor: default;
	}
	@media (pointer: coarse) {
		.action {
			padding: 0.55rem 0;
		}
	}
	.question {
		scroll-margin-top: 4.5rem;
	}
	/* Clears the composer docked at the bottom of the screen. */
	.tail {
		scroll-margin-bottom: 7.5rem;
	}
	.action.problem {
		margin-left: 0;
		font-style: italic;
		font-weight: inherit;
		text-decoration: underline;
		text-underline-offset: 0.2em;
	}
	.live {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	.dot {
		width: 5px;
		height: 5px;
		border-radius: 50%;
		background: var(--rubric);
		animation: pulse 1.2s ease-in-out infinite;
	}
	@keyframes pulse {
		50% {
			opacity: 0.25;
		}
	}

	@media (max-width: 719px) {
		.number {
			position: static;
			margin-right: 0.35em;
		}
		.tail {
			scroll-margin-bottom: calc(7rem + var(--strip, 0px));
		}
	}
</style>
