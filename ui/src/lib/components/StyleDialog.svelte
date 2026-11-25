<script lang="ts">
	import type { ChatStyle, Goal, Length, ModelProfile, Thinking } from '#lib/api.ts';
	import Choice from './Choice.svelte';
	import Dialog from './Dialog.svelte';

	let {
		profiles = [],
		defaultModel = '',
		onsave
	}: {
		profiles?: ModelProfile[];
		defaultModel?: string;
		/** Resolves to an error message, or null once saved. */
		onsave: (style: ChatStyle) => Promise<string | null>;
	} = $props();

	const CUSTOM_MAX = 10_000;
	let dialog: Dialog;
	let goal = $state<Goal>('default');
	let custom = $state('');
	let length = $state<Length>('default');
	let model = $state('');
	let thinking = $state<'' | Thinking>('');
	let problem = $state<string | null>(null);
	let saving = $state(false);

	export function show(style: ChatStyle) {
		goal = style.goal;
		custom = style.custom ?? '';
		length = style.length;
		model = style.model ?? '';
		thinking = style.thinking ?? '';
		problem = null;
		dialog.show();
	}

	function applyProfile(name: string) {
		const found = profiles.find((p) => p.name === name);
		if (!found) return;
		model = found.model;
		thinking = found.thinking;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		saving = true;
		const cleanModel = model.trim();
		problem = await onsave({
			goal,
			custom: custom.trim(),
			length,
			...(cleanModel ? { model: cleanModel } : {}),
			...(thinking ? { thinking } : {})
		});
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
		<div class="field">
			<label for="notebook-model">Model for this notebook (optional)</label>
			<input
				id="notebook-model"
				bind:value={model}
				placeholder={defaultModel ? `Default from Settings (${defaultModel})` : 'Default from Settings'}
				autocapitalize="none"
				autocorrect="off"
				spellcheck="false"
			/>
			{#if profiles.length > 0}
				<div class="profile-pills">
					{#each profiles as p (p.name)}
						<button type="button" class="pill" onclick={() => applyProfile(p.name)}>{p.name}</button>
					{/each}
					{#if model || thinking}
						<button
							type="button"
							class="pill quiet"
							onclick={() => {
								model = '';
								thinking = '';
							}}
						>
							Use Settings default
						</button>
					{/if}
				</div>
			{/if}
		</div>
		<div class="field">
			<label for="notebook-thinking">Thinking for this notebook</label>
			<select id="notebook-thinking" bind:value={thinking}>
				<option value="">Default from Settings</option>
				<option value="default">Model default</option>
				<option value="off">Off</option>
				<option value="low">Low</option>
				<option value="medium">Medium</option>
				<option value="high">High</option>
			</select>
		</div>
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
	select {
		padding: 0.45rem 0.65rem;
		border: 0;
		border-radius: 4px;
		background: var(--page);
		box-shadow: inset 0 0 0 1px var(--edge);
		color: var(--ink);
		font-size: var(--t-s);
	}
	.profile-pills {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		margin-top: 0.35rem;
	}
	.pill {
		padding: 0.2rem 0.55rem;
		border: 1px solid var(--edge);
		border-radius: 999px;
		background: none;
		color: var(--rubric);
		font-size: var(--t-xs);
		cursor: pointer;
	}
	.pill.quiet {
		color: var(--ink-2);
	}
	.submit:disabled {
		border-color: var(--line);
		background: transparent;
		color: var(--ink-2);
		cursor: default;
	}
</style>
