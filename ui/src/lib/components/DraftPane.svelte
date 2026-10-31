<script lang="ts">
	import type { Citation, Piece, Source } from '#lib/api.ts';
	import CitedText from './CitedText.svelte';
	import Sheet from './Sheet.svelte';

	let {
		title,
		pieces,
		citations,
		error,
		sources,
		modal = false,
		onclose,
		onopen
	}: {
		title: string;
		pieces: Piece[];
		citations: Citation[];
		error: string | null;
		sources: Map<string, Source>;
		modal?: boolean;
		onclose: () => void;
		onopen: (citations: Citation[], el: HTMLElement) => void;
	} = $props();
</script>

<Sheet label="Writing {title}" mark="¶" {modal} closeLabel="Close the draft" {onclose}>
	{#snippet heading()}
		<h2>{title}</h2>
		<p class="meta" role="status">
			{#if error}
				<span class="problem">{error}</span>
			{:else}
				<span class="dot" aria-hidden="true"></span> Writing from the sources. It's saved to your notes even if you close
				this.
			{/if}
		</p>
	{/snippet}
	<div aria-busy={!error}>
		<CitedText {pieces} {citations} {sources} active={null} {onopen} apparatus={false} />
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
	}
	.meta {
		display: flex;
		gap: 0.5rem;
		align-items: center;
		margin: 0.3rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.problem {
		color: var(--danger);
	}
	.dot {
		flex: none;
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
