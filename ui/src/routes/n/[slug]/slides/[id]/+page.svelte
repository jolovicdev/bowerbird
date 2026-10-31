<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { api, describe, type Citation, type Source, type StudySet } from '#lib/api.ts';
	import { clip } from '#lib/components/Mark.svelte';

	const slug = $derived(page.params.slug ?? '');
	const id = $derived(page.params.id ?? '');

	let deck = $state<StudySet | null>(null);
	let sources = $state(new Map<string, Source>());
	let notebookTitle = $state('');
	let error = $state<string | null>(null);
	let at = $state(0);
	let notes = $state(false);
	let stage = $state<HTMLElement>();
	let exportProblem = $state<string | null>(null);

	onMount(async () => {
		try {
			const [loaded, nb] = await Promise.all([api.study(slug, id), api.notebook(slug)]);
			deck = loaded;
			notebookTitle = nb.title;
			sources = new Map(nb.sources.map((s) => [s.id, s]));
		} catch (e) {
			error = describe(e);
		}
	});

	const slides = $derived(deck?.slides ?? []);
	const slide = $derived(slides[at]);
	const go = (n: number) => (at = Math.max(0, Math.min(slides.length - 1, n)));

	function keys(e: KeyboardEvent) {
		if ((e.target as HTMLElement).closest('input, textarea') || e.metaKey || e.ctrlKey || e.altKey) return;
		if (['ArrowRight', 'PageDown', ' '].includes(e.key)) {
			e.preventDefault();
			go(at + 1);
		} else if (['ArrowLeft', 'PageUp'].includes(e.key)) {
			e.preventDefault();
			go(at - 1);
		} else if (e.key === 'Home') go(0);
		else if (e.key === 'End') go(slides.length - 1);
	}

	async function exportPptx() {
		if (!deck) return;
		exportProblem = null;
		try {
			const link = document.createElement('a');
			link.href = URL.createObjectURL(await api.slidesPptx(slug, id));
			link.download = `${deck.title}.pptx`;
			link.click();
			URL.revokeObjectURL(link.href);
		} catch (e) {
			exportProblem = describe(e);
		}
	}

	const where = (c: Citation) => {
		const source = c.source_id ? sources.get(c.source_id) : undefined;
		return [source?.title ?? 'a removed source', c.page ? `p. ${c.page}` : ''].filter(Boolean).join(', ');
	};
</script>

<svelte:window onkeydown={keys} />
<svelte:head><title>{deck ? `${deck.title} · Bowerbird` : 'Bowerbird'}</title></svelte:head>

{#snippet face(s: (typeof slides)[number], number: number)}
	<div class="slide" class:opening={number === 1}>
		<p class="folio">{number} / {slides.length}</p>
		<h2>{s.title}</h2>
		{#if s.points.length}
			<ul>
				{#each s.points as point, i (i)}<li>{point}</li>{/each}
			</ul>
		{/if}
		{#if s.citations.length}
			<ol class="apparatus">
				{#each s.citations as c, i (i)}
					<li>
						<span class="siglum">{c.letter}{#if !c.verified}†{/if}</span>
						{where(c)}: “{clip(c.quote, 80)}”
					</li>
				{/each}
			</ol>
		{/if}
	</div>
{/snippet}

<div class="page">
	<nav class="head">
		<a class="back" href="/n/{encodeURIComponent(slug)}">{notebookTitle || 'Notebook'}</a>
		{#if deck}
			<span class="tools">
				<button class="action" onclick={() => (notes = !notes)}>{notes ? 'Hide notes' : 'Speaker notes'}</button>
				<button class="action" onclick={() => stage?.requestFullscreen()}>Present</button>
				<button class="action" onclick={() => print()}>Print or save as PDF</button>
				<button class="action" onclick={exportPptx}>Download PowerPoint</button>
			</span>
		{/if}
	</nav>
	{#if exportProblem}
		<p class="problem" role="alert">{exportProblem}</p>
	{/if}

	{#if error}
		<p class="problem">{error}</p>
	{:else if !deck}
		<p class="lede">Opening the slides…</p>
	{:else if slide}
		<h1>{deck.title}</h1>
		<div class="stage" bind:this={stage} role="region" aria-label="Slide {at + 1} of {slides.length}" aria-live="polite">
			{@render face(slide, at + 1)}
		</div>
		<div class="controls">
			<button class="action" disabled={at === 0} onclick={() => go(at - 1)}>Previous</button>
			<span class="count">Slide {at + 1} of {slides.length}</span>
			<button class="action" disabled={at === slides.length - 1} onclick={() => go(at + 1)}>Next</button>
		</div>
		{#if notes && slide.notes}
			<section class="notes" aria-label="Speaker notes"><p>{slide.notes}</p></section>
		{/if}
		<p class="hint">Arrow keys move between slides.</p>
		<!-- Every slide, one per printed page. -->
		<div class="printed" aria-hidden="true">
			{#each slides as s, i (i)}{@render face(s, i + 1)}{/each}
		</div>
	{/if}
</div>

<style>
	.page {
		width: min(100% - 2.5rem, 64rem);
		margin: 0 auto;
		padding-bottom: 5rem;
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		gap: 0 1.5rem;
		align-items: baseline;
		padding: 1rem 0 0.7rem;
		border-bottom: 1px solid var(--line);
		font-size: var(--t-xs);
		font-style: italic;
	}
	.back {
		margin-right: auto;
		padding: 0.3rem 0;
		color: var(--ink-2);
		text-decoration: none;
		white-space: nowrap;
	}
	.back::before {
		content: '‹ ';
		font-style: normal;
	}
	.tools {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 1.25rem;
	}
	.action {
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: inherit;
		font-style: normal;
		font-weight: 600;
		cursor: pointer;
	}
	.action:disabled {
		color: var(--ink-2);
		font-weight: 400;
		cursor: default;
	}
	h1 {
		margin: 2.5rem 0 1.5rem;
		font-size: var(--t-xl);
		font-style: italic;
		font-weight: 400;
		color: var(--ink-2);
	}
	.lede,
	.hint {
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
	}
	.problem {
		margin-top: 3rem;
		color: var(--danger);
	}
	/* The slide is a sheet of paper in both themes, as it would be projected. */
	.stage {
		--paper: #fafaf8;
		--paper-ink: #1f1f1d;
		--paper-ink-2: #5f5e59;
		--paper-rubric: #a3161f;
		container-type: inline-size;
		background: var(--paper);
		box-shadow: 0 0 0 1px var(--line);
	}
	.stage:fullscreen {
		display: grid;
		place-items: center;
		box-shadow: none;
	}
	.slide {
		position: relative;
		display: flex;
		flex-direction: column;
		width: 100%;
		aspect-ratio: 16 / 9;
		padding: 6cqi 7cqi 4cqi;
		color: var(--paper-ink);
		font-size: 2.4cqi;
	}
	.stage:fullscreen .slide {
		width: min(100vw, 177.78vh);
	}
	.folio {
		position: absolute;
		top: 3cqi;
		right: 4cqi;
		margin: 0;
		font-size: 1.4cqi;
		font-style: italic;
		color: var(--paper-rubric);
	}
	h2 {
		margin: 0 0 3cqi;
		font-size: 4.6cqi;
		font-style: italic;
		font-weight: 500;
		font-variation-settings: 'opsz' 72;
		line-height: 1.12;
		text-wrap: balance;
	}
	.opening h2 {
		margin-top: 8cqi;
		font-size: 6.2cqi;
	}
	ul {
		margin: 0;
		padding-left: 1.2em;
		line-height: 1.4;
	}
	li + li {
		margin-top: 0.6em;
	}
	li::marker {
		color: var(--paper-rubric);
	}
	.apparatus {
		margin: auto 0 0;
		padding: 1.4cqi 0 0;
		border-top: 1px solid #d3d4ce;
		list-style: none;
		font-size: 1.35cqi;
		line-height: 1.4;
		color: var(--paper-ink-2);
	}
	.apparatus li + li {
		margin-top: 0.2em;
	}
	.siglum {
		margin-right: 0.35em;
		color: var(--paper-rubric);
		font-style: italic;
		font-weight: 600;
	}
	/* At phone width a 16:9 frame would shrink the text below 9 px, so the
	   slide becomes a card at reading sizes. Present on a wide screen and
	   print keep the frame. */
	@container (width < 36rem) {
		.slide {
			aspect-ratio: auto;
			min-height: 56.25cqi;
			padding: 2.5rem 1.25rem 1.25rem;
			font-size: var(--t-m);
		}
		.folio {
			top: 1rem;
			right: 1.25rem;
			font-size: var(--t-xs);
		}
		h2,
		.opening h2 {
			margin: 0 0 1rem;
			font-size: var(--t-xl);
		}
		.apparatus {
			margin-top: 1.5rem;
			padding-top: 0.6rem;
			font-size: var(--t-xs);
		}
	}
	.controls {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		padding: 0.9rem 0 0;
		font-size: var(--t-s);
	}
	.count {
		font-style: italic;
		color: var(--ink-2);
	}
	.notes {
		margin-top: 1.25rem;
		padding: 0.9rem 1rem;
		border-left: 2px solid var(--rubric);
		background: var(--raised);
		font-size: var(--t-m);
	}
	.notes p {
		margin: 0;
	}
	.printed {
		display: none;
	}
	@media print {
		@page {
			size: landscape;
			margin: 0;
		}
		:global(body) {
			background: #fff;
		}
		.head,
		h1,
		.stage,
		.controls,
		.notes,
		.hint {
			display: none !important;
		}
		.page {
			width: auto;
			padding: 0;
		}
		.printed {
			--paper-ink: #1f1f1d;
			--paper-ink-2: #5f5e59;
			--paper-rubric: #a3161f;
			display: block;
			container-type: inline-size;
		}
		.printed .slide {
			break-after: page;
		}
	}
</style>
