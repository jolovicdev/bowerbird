<script lang="ts">
	import type { Citation, Piece, Source } from '#lib/api.ts';
	import CitedText from './CitedText.svelte';
	import Sheet from './Sheet.svelte';

	let {
		title,
		pieces,
		citations,
		thinking = '',
		since = 0,
		error,
		sources,
		modal = false,
		onclose,
		onopen
	}: {
		title: string;
		pieces: Piece[];
		citations: Citation[];
		thinking?: string;
		since?: number;
		error: string | null;
		sources: Map<string, Source>;
		modal?: boolean;
		onclose: () => void;
		onopen: (citations: Citation[], el: HTMLElement) => void;
	} = $props();

	let now = $state(Date.now());
	$effect(() => {
		if (error || !since) return;
		now = Date.now();
		const timer = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(timer);
	});
	const elapsed = $derived(since ? Math.max(0, Math.round((now - since) / 1000)) : 0);

	let showThinking = $state(false);
	let thinkingBox = $state<HTMLElement>();
	let followThinking = true;
	const hasThinkingText = $derived(thinking.trim().length > 0);
	$effect(() => {
		if (!thinking) {
			followThinking = true;
			return;
		}
		if (thinkingBox && followThinking) thinkingBox.scrollTop = thinkingBox.scrollHeight;
	});
	function scrolledThinking() {
		if (!thinkingBox) return;
		followThinking = thinkingBox.scrollHeight - thinkingBox.scrollTop - thinkingBox.clientHeight < 24;
	}
</script>

<Sheet label="Writing {title}" mark="¶" {modal} closeLabel="Close the draft" {onclose}>
	{#snippet heading()}
		<h2>{title}</h2>
		<p class="meta" role="status">
			{#if error}
				<span class="problem">{error}</span>
			{:else if !pieces.length && thinking}
				<span class="dot" aria-hidden="true"></span>
				Thinking, {elapsed} s. It's saved to your notes even if you close this.
			{:else}
				<span class="dot" aria-hidden="true"></span>
				Writing from the sources{#if since}, {elapsed} s{/if}. It's saved to your notes even if you close this.
			{/if}
			{#if hasThinkingText}
				<button
					class="action"
					aria-expanded={showThinking}
					onclick={() => (showThinking = !showThinking)}
				>
					{showThinking ? 'Hide thinking' : 'Show thinking'}
				</button>
			{/if}
		</p>
		{#if hasThinkingText && showThinking}
			<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
			<div
				class="thinking"
				tabindex="0"
				role="region"
				aria-label="The model's thinking"
				bind:this={thinkingBox}
				onscroll={scrolledThinking}
			>
				{thinking}
			</div>
		{/if}
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
		flex-wrap: wrap;
		gap: 0.35rem 0.5rem;
		align-items: center;
		margin: 0.3rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.action {
		padding: 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: inherit;
		font-style: normal;
		font-weight: 600;
		cursor: pointer;
	}
	.thinking {
		max-height: 10rem;
		margin: 0.5rem 0 0;
		padding: 0.1rem 0 0.1rem 0.75rem;
		overflow-y: auto;
		border-left: 2px solid var(--line);
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
		line-height: 1.5;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
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
