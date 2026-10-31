<script lang="ts">
	import type { Citation, GuideState, Source } from '#lib/api.ts';
	import CitedText from './CitedText.svelte';

	let {
		state,
		writing,
		error,
		sources,
		active,
		onopen,
		onask
	}: {
		state: GuideState | null;
		/** A new overview is being written. */
		writing: boolean;
		error: string | null;
		sources: Map<string, Source>;
		active: Citation[] | null;
		onopen: (citations: Citation[], el: HTMLElement) => void;
		onask: (question: string) => void;
	} = $props();

	const guide = $derived(state?.guide ?? null);
</script>

<section class="guide" aria-label="Overview of the sources" aria-busy={writing}>
	{#if guide}
		<CitedText pieces={guide.pieces} citations={guide.citations} {sources} {active} {onopen} apparatus={false} />
		{#if guide.questions.length}
			<h2>Questions to start with</h2>
			<ul class="starters">
				{#each guide.questions as question (question)}
					<li><button onclick={() => onask(question)}>{question}</button></li>
				{/each}
			</ul>
		{/if}
		<p class="meta">
			{#if writing}
				<span class="dot" aria-hidden="true"></span> Updating the overview for the sources that are switched on…
			{:else}
				Overview by {guide.model}. Ask anything else below.
			{/if}
		</p>
	{:else if writing}
		<p class="meta"><span class="dot" aria-hidden="true"></span> Reading the sources to write an overview…</p>
	{:else}
		<p class="lede">
			Ask anything about these sources. Answers cite the passages they come from, and every quote is checked against
			the source.
		</p>
	{/if}
	{#if error}
		<p class="meta problem">
			Couldn't write an overview. {error}
			{#if error.includes('Settings')}<a href="/settings">Open Settings</a>{/if}
		</p>
	{/if}
</section>

<style>
	.guide {
		margin-bottom: 2rem;
	}
	h2 {
		margin: 1.75rem 0 0.5rem;
		font-size: var(--t-s);
		font-style: italic;
		font-weight: 400;
		color: var(--ink-2);
	}
	.starters {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.starters button {
		display: block;
		width: 100%;
		padding: 0.45rem 0 0.45rem 1.1rem;
		border: 0;
		background: none;
		color: var(--ink);
		font-size: var(--t-m);
		font-style: italic;
		line-height: 1.4;
		text-align: left;
		text-indent: -1.1rem;
		cursor: pointer;
	}
	.starters button::before {
		content: '›';
		display: inline-block;
		width: 1.1rem;
		color: var(--rubric);
		font-style: normal;
		text-indent: 0;
	}
	.starters button:hover {
		color: var(--rubric);
	}
	.lede {
		margin: 0;
		color: var(--ink-2);
		font-size: var(--t-m);
		font-style: italic;
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.5rem;
		align-items: center;
		margin: 1rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.problem {
		color: var(--danger);
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
</style>
