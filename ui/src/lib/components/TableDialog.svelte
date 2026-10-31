<script lang="ts">
	import Dialog from './Dialog.svelte';

	let {
		oncreate
	}: {
		oncreate: (describe: string, topic: string) => void;
	} = $props();

	let dialog: Dialog;
	let describe = $state('');
	let topic = $state('');
	let field = $state<HTMLTextAreaElement>();

	export function show() {
		describe = '';
		topic = '';
		dialog.show();
		field?.focus();
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		oncreate(describe.trim(), topic.trim());
		dialog.close();
	}
</script>

<Dialog bind:this={dialog} title="New table">
	<form class="form" onsubmit={submit}>
		<div class="field">
			<label for="table-describe">What should the table collect?</label>
			<textarea
				id="table-describe"
				rows="3"
				bind:this={field}
				bind:value={describe}
				placeholder="For example: battles, with date, place, commanders, and outcome"
			></textarea>
		</div>
		<div class="field">
			<label for="table-topic">Topic, if you want to focus on one</label>
			<input id="table-topic" bind:value={topic} placeholder="Leave empty to cover every source" />
		</div>
		<p class="hint">Each row links to the passage it comes from. Cells stay empty where the sources don't say.</p>
		<button class="submit" disabled={!describe.trim()}>Build the table</button>
	</form>
</Dialog>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
	}
	.submit:disabled {
		border-color: var(--line);
		background: transparent;
		color: var(--ink-2);
		cursor: default;
	}
</style>
