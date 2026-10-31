<script lang="ts">
	import type { ChatStyle, Goal, Length } from '#lib/api.ts';
	import Choice from './Choice.svelte';
	import Dialog from './Dialog.svelte';

	let {
		onsave
	}: {
		/** Resolves to an error message, or null once saved. */
		onsave: (style: ChatStyle) => Promise<string | null>;
	} = $props();

	const CUSTOM_MAX = 10_000;
	let dialog: Dialog;
	let goal = $state<Goal>('default');
	let custom = $state('');
	let length = $state<Length>('default');
	let problem = $state<string | null>(null);
	let saving = $state(false);

	export function show(style: ChatStyle) {
		goal = style.goal;
		custom = style.custom ?? '';
		length = style.length;
		problem = null;
		dialog.show();
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		saving = true;
		problem = await onsave({ goal, custom: custom.trim(), length });
		saving = false;
		if (!problem) dialog.close();
	}
</script>

<Dialog bind:this={dialog} title="Answer style">
	<form class="form" onsubmit={submit}>
		<p class="hint">How answers in this notebook are written. Every style cites the sources.</p>
		<Choice
			legend="Style"
			name="goal"
			options={[
				{ value: 'default', label: 'Default' },
				{ value: 'tutor', label: 'Tutor' },
				{ value: 'custom', label: 'Your own' }
			]}
			bind:value={goal}
		/>
		{#if goal === 'default'}
			<p class="hint">Direct answers to the question asked.</p>
		{:else if goal === 'tutor'}
			<p class="hint">
				Explains step by step, defines terms as they come up, and ends with a question that checks your understanding.
				Good for studying.
			</p>
		{:else}
			<div class="field">
				<label for="custom-style">Describe how to answer</label>
				<textarea
					id="custom-style"
					rows="5"
					maxlength={CUSTOM_MAX}
					bind:value={custom}
					placeholder="For example: Answer as a history teacher preparing me for an exam. Give dates and name the people involved."
				></textarea>
				<p class="hint">{custom.length.toLocaleString()} of {CUSTOM_MAX.toLocaleString()} characters</p>
			</div>
		{/if}
		<Choice
			legend="Length"
			name="length"
			options={[
				{ value: 'shorter', label: 'Shorter' },
				{ value: 'default', label: 'Default' },
				{ value: 'longer', label: 'Longer' }
			]}
			bind:value={length}
		/>
		{#if problem}<p class="problem" role="alert">{problem}</p>{/if}
		<button class="submit" disabled={saving || (goal === 'custom' && !custom.trim())}>{saving ? 'Saving…' : 'Save'}</button>
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
