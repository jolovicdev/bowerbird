<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		label,
		mark = '',
		modal = false,
		wide = false,
		closeLabel = 'Close',
		onclose,
		onback,
		backLabel = '',
		body = $bindable(),
		heading,
		top,
		children
	}: {
		/** Names the sheet for screen readers. */
		label: string;
		/** Large rubric glyph before the heading: a siglum, or ¶ for a note. */
		mark?: string;
		/** On narrow screens the sheet covers the page. */
		modal?: boolean;
		/** Spread over the text too, for content that needs room, like a table. */
		wide?: boolean;
		closeLabel?: string;
		onclose: () => void;
		/** Returns to what the sheet showed before, such as the note a citation was opened from. */
		onback?: () => void;
		backLabel?: string;
		body?: HTMLElement;
		heading: Snippet;
		/** Between the header and the scrolling body: confirmations, errors. */
		top?: Snippet;
		children: Snippet;
	} = $props();

	let closeButton: HTMLButtonElement;
	$effect(() => {
		closeButton.focus();
	});
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && !e.defaultPrevented && onclose()} />

<button class="scrim" aria-label="Close" tabindex="-1" onclick={onclose}></button>
<aside class="pane" class:wide role={modal ? 'dialog' : undefined} aria-modal={modal ? 'true' : undefined} aria-label={label}>
	{#if onback}
		<button class="back" onclick={onback}>{backLabel}</button>
	{/if}
	<header>
		{#if mark}<span class="mark" aria-hidden="true">{mark}</span>{/if}
		<div class="heading">{@render heading()}</div>
		<button class="close" bind:this={closeButton} onclick={onclose} aria-label={closeLabel}>
			<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg>
		</button>
	</header>
	{@render top?.()}
	<div class="body" bind:this={body}>{@render children()}</div>
</aside>

<style>
	.scrim {
		display: none;
	}
	/* A sheet laid over the margin, beside the text column, so the text
	   being read stays where it is. */
	.pane {
		position: fixed;
		top: 0;
		right: 0;
		bottom: 0;
		z-index: 11;
		display: flex;
		flex-direction: column;
		width: var(--pane-width, 30rem);
		background: var(--raised);
		border-left: 1px solid var(--line);
		animation: slide-in 180ms var(--ease);
	}
	.pane.wide {
		width: min(64rem, 100vw - 4rem);
		box-shadow: -1px 0 0 var(--line);
	}
	@keyframes slide-in {
		from {
			transform: translateX(1.5rem);
			opacity: 0;
		}
	}
	.back {
		align-self: flex-start;
		margin: 1rem 1.5rem 0;
		padding: 0;
		border: 0;
		background: none;
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
		cursor: pointer;
	}
	.back::before {
		content: '‹ ';
		font-style: normal;
	}
	.back:hover {
		color: var(--ink);
	}
	.back + header {
		padding-top: 0.75rem;
	}
	header {
		display: flex;
		gap: 0.9rem;
		align-items: flex-start;
		padding: 1.5rem 1.25rem 1.1rem 1.5rem;
		border-bottom: 1px solid var(--line);
	}
	.mark {
		flex: none;
		min-width: 1.5rem;
		color: var(--rubric);
		font-size: var(--t-xxl);
		font-style: italic;
		font-weight: 600;
		line-height: 1;
	}
	.heading {
		flex: 1;
		min-width: 0;
	}
	.close {
		display: grid;
		flex: none;
		place-items: center;
		width: 2.25rem;
		height: 2.25rem;
		border: 0;
		border-radius: 4px;
		background: transparent;
		color: var(--ink-2);
		cursor: pointer;
	}
	.close:hover {
		background: var(--page);
		color: var(--ink);
	}
	.close svg {
		width: 1rem;
		height: 1rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.5;
		stroke-linecap: round;
	}
	.body {
		flex: 1;
		padding: 1.25rem 1.5rem 2rem;
		overflow-y: auto;
		font-size: var(--t-m);
		line-height: 1.6;
	}

	/* Narrow screens: the sheet covers the text, so the page behind is dimmed. */
	@media (max-width: 959px) {
		.scrim {
			display: block;
			position: fixed;
			inset: 0;
			z-index: 10;
			border: 0;
			background: var(--scrim);
		}
		.pane {
			width: min(30rem, 92vw);
		}
	}

	/* Phones: a bottom sheet. */
	@media (max-width: 719px) {
		.pane {
			top: auto;
			left: 0;
			width: auto;
			max-height: 86dvh;
			border-left: 0;
			border-radius: 14px 14px 0 0;
			box-shadow: 0 -1px 0 var(--line);
			animation-name: rise;
		}
		@keyframes rise {
			from {
				transform: translateY(2rem);
				opacity: 0;
			}
		}
		.back {
			margin-inline: 1.25rem;
		}
		header {
			padding: 1.25rem 1rem 0.85rem 1.25rem;
		}
		.body {
			padding: 1rem 1.25rem calc(1.5rem + env(safe-area-inset-bottom));
		}
	}
</style>
