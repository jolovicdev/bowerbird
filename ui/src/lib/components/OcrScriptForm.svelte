<script lang="ts">
	import { untrack } from 'svelte';
	import { api, describe, type OcrScript, type Settings } from '#lib/api.ts';

	let { settings, onsaved }: { settings: Settings; onsaved: (settings: Settings) => void } = $props();

	const SCRIPTS: { value: OcrScript; label: string }[] = [
		{ value: 'latin', label: 'Latin (English & European)' },
		{ value: 'cyrillic', label: 'Cyrillic (Russian, Ukrainian, Serbian, Bulgarian)' },
		{ value: 'multilingual', label: 'Chinese, Japanese & English (Multilingual)' },
		{ value: 'korean', label: 'Korean (Hangul & English)' },
		{ value: 'arabic', label: 'Arabic' },
		{ value: 'devanagari', label: 'Devanagari (Hindi, Marathi, Nepali)' },
		{ value: 'greek', label: 'Greek' },
		{ value: 'thai', label: 'Thai' },
		{ value: 'tamil', label: 'Tamil' },
		{ value: 'telugu', label: 'Telugu' }
	];

	// The choice starts from what is saved and is the user's from then on.
	let script = $state<OcrScript>(untrack(() => settings.ocr_script));
	let saved = $state(false);
	let error = $state<string | null>(null);

	async function save(event: SubmitEvent) {
		event.preventDefault();
		saved = false;
		error = null;
		try {
			const next = await api.saveSettings({ ocr_script: script });
			script = next.ocr_script;
			saved = true;
			onsaved(next);
		} catch (e) {
			error = describe(e);
		}
	}
</script>

<form class="model" onsubmit={save}>
	<label for="ocr-script">Script for scanned pages</label>
	<div class="inline">
		<select
			id="ocr-script"
			bind:value={script}
			onchange={() => (saved = false)}
		>
			{#each SCRIPTS as opt (opt.value)}
				<option value={opt.value}>{opt.label}</option>
			{/each}
		</select>
		<button class="primary" disabled={script === settings.ocr_script}>Save</button>
	</div>
	<p class="hint">
		Each script's recognizer downloads once (8 to 16 MB) when it first reads a scanned PDF. After switching scripts,
		open any scanned PDF and choose <strong>Re-read scan</strong>.
	</p>
	{#if saved}<p class="hint">Saved OCR script.</p>{/if}
	{#if error}<p class="problem" role="alert">{error}</p>{/if}
</form>

<style>
	label {
		display: block;
		margin-bottom: 0.4rem;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.inline {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	.primary {
		padding: calc(0.45rem - var(--cap-lift)) 1rem calc(0.45rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		font-weight: 600;
		cursor: pointer;
	}
	.primary:disabled {
		border-color: var(--line);
		background: transparent;
		color: var(--ink-2);
		cursor: default;
	}
	.hint {
		margin: 0.5rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.problem {
		margin: 0.5rem 0 0;
		color: var(--danger);
	}
	select {
		flex: 1;
		min-width: 0;
		padding: 0.5rem 0.65rem;
		border: 0;
		border-radius: 4px;
		background: var(--raised);
		box-shadow: inset 0 0 0 1px var(--edge);
		color: var(--ink);
	}
</style>
