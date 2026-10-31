<script lang="ts">
	let {
		title,
		enabled,
		onfiles
	}: {
		/** The notebook files go into. */
		title: string;
		enabled: boolean;
		onfiles: (files: File[]) => void;
	} = $props();

	let dropping = $state(false);

	const carriesFiles = (e: DragEvent) => e.dataTransfer?.types.includes('Files') ?? false;
</script>

<svelte:window
	ondragover={(e) => {
		if (!carriesFiles(e) || !enabled) return;
		e.preventDefault();
		dropping = true;
	}}
	ondragleave={(e) => {
		if (e.relatedTarget === null) dropping = false;
	}}
	ondrop={(e) => {
		if (!carriesFiles(e) || !enabled) return;
		e.preventDefault();
		dropping = false;
		const files = [...(e.dataTransfer?.files ?? [])];
		if (files.length) onfiles(files);
	}}
/>

{#if dropping}
	<div class="dropzone" aria-hidden="true"><p>Drop to add to {title}</p></div>
{/if}

<style>
	.dropzone {
		position: fixed;
		inset: 0.75rem;
		z-index: 20;
		display: grid;
		place-items: center;
		border-radius: 6px;
		background: color-mix(in srgb, var(--page) 90%, transparent);
		box-shadow: inset 0 0 0 1px var(--rubric);
		pointer-events: none;
	}
	.dropzone p {
		font-size: var(--t-xl);
		font-style: italic;
	}
</style>
