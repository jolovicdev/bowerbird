<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { api, describe, type Provider, type SearchBackend, type Settings } from '#lib/api.ts';
	import Choice from '#lib/components/Choice.svelte';

	let settings = $state<Settings | null>(null);
	let error = $state<string | null>(null);

	let model = $state('');
	let modelSaved = $state(false);
	let modelError = $state<string | null>(null);

	let backend = $state<SearchBackend>('duckduckgo');
	let searxng = $state('');
	let searchSaved = $state(false);
	let searchError = $state<string | null>(null);

	let editing = $state<string | null>(null);
	let key = $state('');
	let keyError = $state<string | null>(null);

	onMount(async () => {
		try {
			settings = await api.settings();
			model = settings.chat_model;
			backend = settings.search.backend;
			searxng = settings.search.searxng_url ?? '';
		} catch (e) {
			error = describe(e);
		}
	});

	// Providers with a key first, so the ones in use are at hand.
	const providers = $derived(
		[...(settings?.providers ?? [])].sort((a, b) => Number(b.has_key) - Number(a.has_key) || a.name.localeCompare(b.name))
	);

	const status = (p: Provider) => (p.saved ? 'Key saved' : p.has_key ? 'Key from the environment' : 'No key');

	async function saveModel(event: SubmitEvent) {
		event.preventDefault();
		modelSaved = false;
		modelError = null;
		try {
			settings = await api.saveSettings({ chat_model: model.trim() });
			model = settings.chat_model;
			modelSaved = true;
		} catch (e) {
			modelError = describe(e);
		}
	}

	const SEARCH_ABOUT: Record<SearchBackend, string> = {
		duckduckgo: 'Needs no setup. DuckDuckGo sometimes refuses automated searches; Bowerbird says so when it does.',
		searxng: 'A search engine you run yourself, free and private. Turn on JSON results in its settings.yml.',
		brave: 'Brave Search API, with your key. Its free plan covers a few thousand searches a month.',
		tavily: 'Tavily, a search API made for research tools, with your key.',
		exa: 'Exa, a search API that is good at finding papers and long articles, with your key.'
	};
	const KEY_FOR: Partial<Record<SearchBackend, string>> = {
		brave: 'BRAVE_SEARCH_API_KEY',
		tavily: 'TAVILY_API_KEY',
		exa: 'EXA_API_KEY'
	};
	const searchKey = $derived(settings?.search_keys.find((k) => k.key_env === KEY_FOR[backend]));

	async function saveSearch(event: SubmitEvent) {
		event.preventDefault();
		searchSaved = false;
		searchError = null;
		try {
			settings = await api.saveSettings({ search: { backend, searxng_url: searxng.trim() || undefined } });
			searchSaved = true;
		} catch (e) {
			searchError = describe(e);
		}
	}

	async function edit(env: string) {
		editing = env;
		key = '';
		keyError = null;
		await tick();
		document.getElementById(`key-${env}`)?.focus();
	}

	async function saveKey(event: SubmitEvent, env: string) {
		event.preventDefault();
		try {
			settings = await api.saveSettings({ keys: { [env]: key.trim() } });
			editing = null;
			key = '';
		} catch (e) {
			keyError = describe(e);
		}
	}

	async function removeKey(env: string) {
		try {
			settings = await api.saveSettings({ keys: { [env]: '' } });
		} catch (e) {
			error = describe(e);
		}
	}
</script>

{#snippet keyRow(provider: Provider)}
	<li>
		<div class="row">
			<span class="name">{provider.name}</span>
			<span class="status" class:ready={provider.has_key}>{status(provider)}</span>
			{#if editing !== provider.key_env}
				<span class="row-actions">
					<button class="action" onclick={() => edit(provider.key_env)}>
						{provider.has_key ? 'Replace key' : 'Add key'}
					</button>
					{#if provider.saved}
						<button class="action quiet" onclick={() => removeKey(provider.key_env)}>Remove</button>
					{/if}
				</span>
			{/if}
		</div>
		{#if editing === provider.key_env}
			<form class="inline key" onsubmit={(e) => saveKey(e, provider.key_env)}>
				<label class="visually-hidden" for="key-{provider.key_env}">{provider.name} API key</label>
				<input
					id="key-{provider.key_env}"
					type="password"
					autocomplete="off"
					placeholder={provider.key_env}
					bind:value={key}
					required
					onkeydown={(e) => e.key === 'Escape' && (editing = null)}
				/>
				<button class="primary" disabled={!key.trim()}>Save key</button>
				<button type="button" class="action quiet" onclick={() => (editing = null)}>Cancel</button>
			</form>
			{#if keyError}<p class="problem" role="alert">{keyError}</p>{/if}
		{/if}
	</li>
{/snippet}

<svelte:head><title>Settings · Bowerbird</title></svelte:head>

<div class="page">
	<nav><a href="/">Notebooks</a></nav>
	<h1>Settings</h1>

	{#if error}
		<p class="problem" role="alert">{error}</p>
	{/if}
	{#if settings}
		<section aria-labelledby="model-heading">
			<h2 id="model-heading">Model</h2>
			<form class="model" onsubmit={saveModel}>
				<label for="chat-model">Model for answers</label>
				<div class="inline">
					<input
						id="chat-model"
						bind:value={model}
						placeholder={settings.default_model}
						aria-describedby="model-hint"
						oninput={() => (modelSaved = false)}
					/>
					<button class="primary" disabled={model.trim() === settings.chat_model}>Save</button>
				</div>
				<p class="hint" id="model-hint">
					Use the name the provider gives the model. Bowerbird picks the provider from the name; to choose one
					yourself, put it in front, as in <code>groq::model-name</code>. Leave it empty for {settings.default_model}.
				</p>
				{#if modelSaved}<p class="hint" role="status">Saved. New questions use {settings.chat_model}.</p>{/if}
				{#if modelError}<p class="problem" role="alert">{modelError}</p>{/if}
			</form>
		</section>

		<section aria-labelledby="search-heading">
			<h2 id="search-heading">Web search</h2>
			<p class="hint">Used by Find sources on the web, in Add sources.</p>
			<form class="model" onsubmit={saveSearch}>
				<Choice
					legend="Search with"
					name="search-backend"
					options={[
						{ value: 'duckduckgo', label: 'DuckDuckGo' },
						{ value: 'searxng', label: 'SearXNG' },
						{ value: 'brave', label: 'Brave' },
						{ value: 'tavily', label: 'Tavily' },
						{ value: 'exa', label: 'Exa' }
					]}
					bind:value={backend}
				/>
				<p class="hint">{SEARCH_ABOUT[backend]}</p>
				{#if backend === 'searxng'}
					<label for="searxng-url">SearXNG address</label>
					<input id="searxng-url" type="url" bind:value={searxng} placeholder="http://localhost:8888" />
				{/if}
				<p><button class="primary">Save</button></p>
				{#if searchSaved}<p class="hint" role="status">Saved.</p>{/if}
				{#if searchError}<p class="problem" role="alert">{searchError}</p>{/if}
			</form>
			{#if searchKey}
				<ul class="providers">{@render keyRow(searchKey)}</ul>
			{/if}
		</section>

		<section aria-labelledby="keys-heading">
			<h2 id="keys-heading">API keys</h2>
			<p class="hint">
				Keys stay on this computer, in a settings file only your user account can read. A key set as an environment
				variable works too; a key saved here takes its place.
			</p>
			<ul class="providers">
				{#each providers as provider (provider.key_env)}{@render keyRow(provider)}{/each}
			</ul>
		</section>
	{:else if !error}
		<p class="hint">Loading settings…</p>
	{/if}
</div>

<style>
	.page {
		width: min(100% - 2.5rem, 46rem);
		margin: 0 auto;
		padding: 0 0 6rem;
	}
	nav {
		margin-bottom: 4rem;
		padding: 1rem 0 0.7rem;
		border-bottom: 1px solid var(--line);
		font-size: var(--t-xs);
		font-style: italic;
	}
	nav a {
		color: var(--ink-2);
		text-decoration: none;
	}
	nav a::before {
		content: '‹ ';
	}
	nav a:hover {
		color: var(--ink);
	}
	h1 {
		margin: 0 0 3rem;
		font-size: var(--t-display);
		font-weight: 500;
		font-variation-settings: 'opsz' 72;
		line-height: 1.08;
		letter-spacing: -0.015em;
	}
	h2 {
		margin: 0 0 0.75rem;
		font-size: var(--t-xl);
		font-style: italic;
		font-weight: 400;
		font-variation-settings: 'opsz' 48;
	}
	section + section {
		margin-top: 3rem;
	}
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
	.hint + form {
		margin-top: 1rem;
	}
	code {
		font-family: var(--mono);
	}
	.problem {
		margin: 0.5rem 0 0;
		color: var(--danger);
	}
	.providers {
		margin: 1.25rem 0 0;
		padding: 0;
		list-style: none;
		border-bottom: 1px solid var(--line);
	}
	.providers li {
		padding: 0.75rem 0;
		border-top: 1px solid var(--line);
	}
	.row {
		display: flex;
		gap: 1rem;
		align-items: baseline;
	}
	.name {
		font-weight: 600;
	}
	.status {
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.status.ready {
		color: var(--ink);
	}
	.row-actions {
		display: flex;
		gap: 1rem;
		margin-left: auto;
	}
	.action {
		padding: 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: var(--t-xs);
		font-weight: 600;
		cursor: pointer;
	}
	.action.quiet {
		color: var(--ink-2);
		font-weight: 400;
	}
	.key {
		margin-top: 0.6rem;
	}
</style>
