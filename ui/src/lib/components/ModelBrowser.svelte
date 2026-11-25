<script lang="ts">
	import { onMount } from 'svelte';
	import { api, describe, type Provider } from '#lib/api.ts';

	let {
		providers,
		server,
		provider = $bindable(''),
		onpick
	}: {
		/** Providers in the order to list them. */
		providers: Provider[];
		/** The address of the user's own model server, when Settings has one. */
		server: string | null;
		/** The provider whose models are listed. The page holds it, so it is still chosen when the list opens again. */
		provider?: string;
		onpick: (model: string) => void;
	} = $props();

	/** Most models shown at once; the filter narrows a longer list. */
	const SHOWN_MAX = 120;

	let filter = $state('');
	let models = $state<string[]>([]);
	let loading = $state(false);
	let error = $state<string | null>(null);

	const options = $derived.by(() => {
		const list: { value: string; label: string }[] = [];
		// A server address takes the place of Ollama on this device, so only one of the two is listed.
		if (server) list.push({ value: 'server', label: `Your own server (${server})` });
		else list.push({ value: 'Ollama', label: 'Ollama (this device)' });
		for (const p of providers) {
			list.push({ value: p.name, label: p.has_key ? `${p.name} (key ready)` : p.name });
		}
		return list;
	});

	const shown = $derived.by(() => {
		const q = filter.trim().toLowerCase();
		if (!q) return models.slice(0, SHOWN_MAX);
		return models.filter((m) => m.toLowerCase().includes(q)).slice(0, SHOWN_MAX);
	});

	let request = 0;
	async function list(from: string) {
		// A slow list for a provider chosen earlier must not replace this one.
		const current = ++request;
		loading = true;
		error = null;
		models = [];
		try {
			const res = await api.models(from);
			if (current === request) models = res.models;
		} catch (e) {
			if (current === request) error = describe(e);
		} finally {
			if (current === request) loading = false;
		}
	}

	onMount(() => {
		provider ||= providers.find((p) => p.has_key)?.name ?? (server ? 'server' : 'OpenRouter');
		list(provider);
	});
</script>

<div class="picker">
	<div class="picker-bar">
		<label class="visually-hidden" for="browse-provider">Provider</label>
		<select
			id="browse-provider"
			bind:value={provider}
			onchange={() => list(provider)}
		>
			{#each options as opt (opt.value)}
				<option value={opt.value}>{opt.label}</option>
			{/each}
		</select>
		<label class="visually-hidden" for="browse-filter">Filter models</label>
		<input
			id="browse-filter"
			type="search"
			placeholder="Filter models (e.g. :free, flash, sonnet)…"
			bind:value={filter}
		/>
	</div>
	{#if loading}
		<p class="hint">Fetching available models…</p>
	{:else if error}
		<p class="problem">{error}</p>
	{:else if shown.length === 0}
		<p class="hint">No matching models found.</p>
	{:else}
		<p class="hint">
			Showing {shown.length} of {models.length} models. Click one to select it.
		</p>
		<ul class="model-list">
			{#each shown as item (item)}
				<li>
					<button type="button" class="model-pick" onclick={() => onpick(item)}>
						<code>{item}</code>
					</button>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.picker {
		margin-top: 0.9rem;
		padding: 0.9rem;
		border: 1px solid var(--line);
		border-radius: 6px;
		background: var(--raised);
	}
	.picker-bar {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
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
	input {
		flex: 1;
		min-width: 0;
		padding: calc(0.5rem - var(--cap-lift)) 0.65rem calc(0.5rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--raised);
		box-shadow: inset 0 0 0 1px var(--edge);
	}
	input:focus {
		outline: 1px solid var(--ink);
		outline-offset: 0;
	}
	.picker-bar select,
	.picker-bar input {
		background: var(--page);
	}
	.hint {
		margin: 0.5rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	code {
		font-family: var(--mono);
	}
	.problem {
		margin: 0.5rem 0 0;
		color: var(--danger);
	}
	.model-list {
		margin: 0.65rem 0 0;
		padding: 0;
		max-height: 15rem;
		overflow-y: auto;
		list-style: none;
		border-top: 1px solid var(--line);
	}
	.model-list li {
		border-bottom: 1px solid var(--line);
	}
	.model-pick {
		display: block;
		width: 100%;
		padding: 0.4rem 0.25rem;
		border: 0;
		background: none;
		color: var(--ink);
		font-size: var(--t-xs);
		text-align: left;
		cursor: pointer;
	}
	.model-pick:hover {
		color: var(--rubric);
	}
</style>
