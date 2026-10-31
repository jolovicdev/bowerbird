<script lang="ts" generics="T extends string">
	let {
		legend,
		name,
		options,
		value = $bindable()
	}: {
		legend: string;
		name: string;
		options: { value: T; label: string }[];
		value: T;
	} = $props();
</script>

<fieldset>
	<legend>{legend}</legend>
	<span class="options">
		{#each options as option (option.value)}
			<label class:on={value === option.value}>
				<input class="visually-hidden" type="radio" {name} value={option.value} bind:group={value} />
				{option.label}
			</label>
		{/each}
	</span>
</fieldset>

<style>
	fieldset {
		margin: 0;
		padding: 0;
		border: 0;
	}
	legend {
		margin-bottom: 0.4rem;
		padding: 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.options {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem 1.1rem;
	}
	label {
		font-size: var(--t-s);
		color: var(--ink-2);
		cursor: pointer;
	}
	label.on {
		color: var(--ink);
		text-decoration: underline;
		text-decoration-color: var(--rubric);
		text-decoration-thickness: 1.5px;
		text-underline-offset: 0.3em;
	}
	label:has(:focus-visible) {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
</style>
