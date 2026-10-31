<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { describe } from '#lib/api.ts';

	let {
		slug,
		sourceId,
		page,
		quotes
	}: {
		slug: string;
		sourceId: string;
		page: number;
		/** Words to mark on the page. */
		quotes: string[];
	} = $props();

	let holder: HTMLElement;
	let status = $state<'loading' | 'ready' | 'missed'>('loading');
	let problem = $state<string | null>(null);

	onMount(async () => {
		try {
			const { renderPage } = await import('#lib/pdf.ts');
			const found = await renderPage(holder, slug, sourceId, page, quotes);
			status = quotes.length && !found ? 'missed' : 'ready';
			await tick();
			holder.querySelector('mark')?.scrollIntoView({ block: 'center' });
		} catch (e) {
			problem = e instanceof Error && !('status' in e) ? `Couldn't draw page ${page}: ${e.message}` : describe(e);
		}
	});
</script>

<figure class="page-view" aria-label="Page {page} as printed">
	<div class="page" bind:this={holder}></div>
	<figcaption>
		{#if problem}
			<span class="problem">{problem}</span>
		{:else if status === 'loading'}
			Drawing page {page}…
		{:else if status === 'missed'}
			Page {page}. The quoted words couldn't be placed on the printed page; the passage above shows them.
		{:else}
			Page {page}{quotes.length ? ', with the quoted words marked' : ''}.
		{/if}
	</figcaption>
</figure>

<style>
	.page-view {
		margin: 0 0 1.25rem;
	}
	.page {
		position: relative;
		width: 100%;
		min-height: 6rem;
		overflow: hidden;
		background: #fff;
		box-shadow: 0 0 0 1px var(--line);
	}
	.page :global(canvas) {
		display: block;
	}
	/* The rules of pdf.js's text layer that place each run of text over the
	   drawn page, transparent, so the marks land on the printed words. */
	.page :global(.textLayer) {
		--min-font-size: 1;
		--text-scale-factor: calc(var(--total-scale-factor) * var(--min-font-size));
		--min-font-size-inv: calc(1 / var(--min-font-size));
		position: absolute;
		inset: 0;
		overflow: clip;
		line-height: 1;
		letter-spacing: normal;
		word-spacing: normal;
		text-align: initial;
		text-size-adjust: none;
		forced-color-adjust: none;
		transform-origin: 0 0;
	}
	.page :global(.textLayer :is(span, br)) {
		position: absolute;
		color: transparent;
		white-space: pre;
		cursor: text;
		transform-origin: 0% 0%;
	}
	.page :global(.textLayer > :not(.markedContent)),
	.page :global(.textLayer .markedContent span:not(.markedContent)) {
		--font-height: 0;
		--scale-x: 1;
		--rotate: 0deg;
		font-size: calc(var(--text-scale-factor) * var(--font-height));
		transform: rotate(var(--rotate)) scaleX(var(--scale-x)) scale(var(--min-font-size-inv));
	}
	.page :global(.textLayer .markedContent) {
		display: contents;
	}
	.page :global(.textLayer ::selection) {
		background: color-mix(in srgb, #a3161f 25%, transparent);
	}
	.page :global(.textLayer mark) {
		position: static;
		background: color-mix(in srgb, #a3161f 28%, transparent);
		color: transparent;
	}
	figcaption {
		margin-top: 0.4rem;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.problem {
		color: var(--danger);
	}
</style>
