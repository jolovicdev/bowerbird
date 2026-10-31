<script lang="ts">
	import type { Snippet } from 'svelte';
	import { closeOnBack, popLayer, pushLayer } from '#lib/layers.svelte.ts';

	let {
		title,
		onclose,
		children
	}: {
		title: string;
		onclose?: () => void;
		children: Snippet;
	} = $props();

	let dialog: HTMLDialogElement;
	const id = `dialog-${Math.random().toString(36).slice(2)}`;

	export function show() {
		dialog.showModal();
		pushLayer('dialog');
	}

	closeOnBack('dialog', () => dialog?.open, () => dialog.close());

	export function close() {
		dialog.close();
	}
</script>

<dialog
	bind:this={dialog}
	aria-labelledby={id}
	onclose={() => {
		popLayer('dialog');
		onclose?.();
	}}
>
	<header>
		<h2 {id}>{title}</h2>
		<button class="close" onclick={() => dialog.close()} aria-label="Close">
			<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg>
		</button>
	</header>
	<div class="content">{@render children()}</div>
</dialog>

<style>
	dialog {
		width: min(100% - 2rem, 32rem);
		padding: 0;
		border: 0;
		border-radius: 6px;
		background: var(--raised);
		color: var(--ink);
		box-shadow: 0 0 0 1px var(--line);
	}
	dialog::backdrop {
		background: var(--scrim);
	}
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 1.25rem 1rem 0.5rem 1.5rem;
	}
	h2 {
		margin: 0;
		font-size: var(--t-xl);
		font-style: italic;
		font-weight: 400;
		font-variation-settings: 'opsz' 48;
	}
	.close {
		display: grid;
		place-items: center;
		width: 2.25rem;
		height: 2.25rem;
		border: 0;
		border-radius: 6px;
		background: transparent;
		color: var(--ink-2);
		cursor: pointer;
	}
	.close svg {
		width: 1rem;
		height: 1rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.75;
		stroke-linecap: round;
	}
	.content {
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
		padding: 0.5rem 1.5rem 1.5rem;
	}
	/* Fields of the forms dialogs hold. */
	.content :global(.field) {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
	}
	.content :global(.field > label),
	.content :global(.hint) {
		margin: 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.content :global(input:not([type='radio'])),
	.content :global(textarea) {
		min-width: 0;
		padding: 0.5rem 0.65rem;
		border: 0;
		border-radius: 4px;
		background: var(--page);
		box-shadow: inset 0 0 0 1px var(--edge);
		font-size: var(--t-s);
	}
	.content :global(input:focus),
	.content :global(textarea:focus) {
		outline: 1px solid var(--ink);
		outline-offset: 0;
	}
	.content :global(textarea) {
		resize: vertical;
	}
	.content :global(.submit) {
		align-self: flex-start;
		padding: calc(0.5rem - var(--cap-lift)) 1.1rem calc(0.5rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		font-weight: 600;
		cursor: pointer;
	}
	.content :global(.problem) {
		margin: 0;
		font-size: var(--t-xs);
		color: var(--danger);
	}

	/* Phones: a bottom sheet, like the passage sheet. */
	@media (max-width: 719px) {
		dialog {
			width: 100%;
			max-width: none;
			max-height: 90dvh;
			margin: auto 0 0;
			border-radius: 14px 14px 0 0;
			padding-bottom: env(safe-area-inset-bottom);
		}
	}
</style>
