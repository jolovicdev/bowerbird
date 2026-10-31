<script lang="ts">
	import { applyTheme, loadTheme, type ThemeChoice } from '#lib/theme.ts';

	const options: { value: ThemeChoice; label: string }[] = [
		{ value: 'system', label: 'Auto' },
		{ value: 'light', label: 'Light' },
		{ value: 'dark', label: 'Dark' }
	];
	let choice = $state(loadTheme());
</script>

<fieldset>
	<legend class="visually-hidden">Theme</legend>
	{#each options as option (option.value)}
		<label class:on={choice === option.value}>
			<input
				class="visually-hidden"
				type="radio"
				name="theme"
				value={option.value}
				bind:group={choice}
				onchange={() => applyTheme(option.value)}
			/>
			{option.label}
		</label>
	{/each}
</fieldset>

<style>
	fieldset {
		display: inline-flex;
		gap: 0.75rem;
		margin: 0;
		padding: 0;
		border: 0;
	}
	label {
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
		cursor: pointer;
	}
	label.on {
		color: var(--ink);
		text-decoration: underline;
		text-decoration-color: var(--rubric);
		text-underline-offset: 0.25em;
	}
	label:has(:focus-visible) {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
</style>
