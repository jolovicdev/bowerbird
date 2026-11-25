<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, describe, type LibrarySearch, type NotebookSummary } from '#lib/api.ts';
	import Logo from '#lib/components/Logo.svelte';
	import ThemeSwitch from '#lib/components/ThemeSwitch.svelte';
	import { ago } from '#lib/format.ts';

	let notebooks = $state<NotebookSummary[] | null>(null);
	let error = $state<string | null>(null);

	let query = $state('');
	let searchResults = $state<LibrarySearch | null>(null);
	let searching = $state(false);
	let searchError = $state<string | null>(null);

	let searchSeq = 0;
	$effect(() => {
		const q = query.trim();
		const seq = ++searchSeq;
		if (!q) {
			searchResults = null;
			searchError = null;
			searching = false;
			return;
		}
		searching = true;
		searchError = null;
		const timer = setTimeout(async () => {
			try {
				const res = await api.searchLibrary(q);
				if (seq === searchSeq) searchResults = res;
			} catch (e) {
				if (seq === searchSeq) searchError = describe(e);
			} finally {
				if (seq === searchSeq) searching = false;
			}
		}, 220);
		return () => clearTimeout(timer);
	});

	const searchCount = $derived(
		searchResults
			? searchResults.notebooks.length +
					searchResults.passages.length +
					searchResults.notes.length +
					searchResults.chats.length
			: 0
	);

	onMount(() => {
		api.notebooks()
			.then((list) => (notebooks = list))
			.catch((e) => (error = describe(e)));
	});

	const count = (n: number) => (n === 1 ? '1 source' : `${n} sources`);

	let naming = $state(false);
	let name = $state('');
	let creating = $state(false);
	let createError = $state<string | null>(null);
	let nameField = $state<HTMLInputElement>();

	async function startNaming() {
		naming = true;
		createError = null;
		await tick();
		nameField?.focus();
	}

	async function create(event: SubmitEvent) {
		event.preventDefault();
		creating = true;
		try {
			const notebook = await api.createNotebook(name.trim());
			await goto(`/n/${notebook.slug}?add`);
		} catch (e) {
			createError = describe(e);
			creating = false;
		}
	}
</script>

<svelte:head><title>Bowerbird</title></svelte:head>

<div class="page">
	<header class="masthead">
		<Logo />
		<span class="masthead-tools">
			<a class="settings" href="/settings">Settings</a>
			<ThemeSwitch />
		</span>
	</header>

	<main>
		<div class="title-row">
			<h1>Notebooks</h1>
			{#if !naming}<button class="primary" onclick={startNaming}>New notebook</button>{/if}
		</div>
		{#if naming}
			<form class="new" onsubmit={create}>
				<label class="visually-hidden" for="new-name">Notebook name</label>
				<input
					id="new-name"
					placeholder="Name the notebook"
					bind:value={name}
					bind:this={nameField}
					required
					aria-describedby={createError ? 'create-error' : undefined}
					onkeydown={(e) => e.key === 'Escape' && (naming = false)}
				/>
				<button class="primary" disabled={!name.trim() || creating}>{creating ? 'Creating…' : 'Create'}</button>
				<button type="button" class="quiet" onclick={() => (naming = false)}>Cancel</button>
			</form>
			{#if createError}<p class="problem create-error" id="create-error" role="alert">{createError}</p>{/if}
		{/if}

		{#if error}
			<p class="problem">{error}</p>
		{:else if notebooks?.length === 0}
			<div class="empty">
				<p>A notebook holds the sources you want to ask about: PDFs, web pages, or pasted text.</p>
				<p>Everything stays on this computer as plain files in your Bowerbird folder.</p>
			</div>
		{:else if notebooks}
			<div class="library-search">
				<label class="visually-hidden" for="library-search">Search across all notebooks</label>
				<input
					id="library-search"
					type="search"
					placeholder="Search across all notebooks…"
					bind:value={query}
				/>
			</div>
			{#if query.trim()}
				{#if searchError}
					<p class="problem" role="alert">{searchError}</p>
				{:else if searching && !searchResults}
					<p class="meta">Searching…</p>
				{:else if searchResults && searchCount === 0}
					<p class="empty">No matches for “{searchResults.query}”.</p>
				{:else if searchResults}
					<div class="search-results">
						{#if searchResults.notebooks.length}
							<h2>Notebooks</h2>
							<ul class="list">
								{#each searchResults.notebooks as nb (nb.slug)}
									<li>
										<a href="/n/{encodeURIComponent(nb.slug)}">
											<span class="entry">
												<span class="title">{nb.title}</span>
												<span class="leader" aria-hidden="true"></span>
												<span class="count">{count(nb.sources)}</span>
											</span>
										</a>
									</li>
								{/each}
							</ul>
						{/if}
						{#if searchResults.passages.length}
							<h2>Passages</h2>
							<ul class="hits">
								{#each searchResults.passages as hit (`${hit.notebook_slug}-${hit.label}`)}
									<li>
										<a
											href="/n/{encodeURIComponent(hit.notebook_slug)}?source={encodeURIComponent(hit.source_id)}&ord={hit.ord}&q={encodeURIComponent(searchResults.query)}"
										>
											<span class="hit-head">
												<span class="hit-title">{hit.source_title}</span>
												<span class="hit-where">
													in {hit.notebook_title}{hit.page_start ? `, p. ${hit.page_start}` : ''}
												</span>
											</span>
											<span class="snippet">{hit.snippet}</span>
										</a>
									</li>
								{/each}
							</ul>
						{/if}
						{#if searchResults.notes.length}
							<h2>Notes</h2>
							<ul class="hits">
								{#each searchResults.notes as hit (`${hit.notebook_slug}-${hit.id}`)}
									<li>
										<a href="/n/{encodeURIComponent(hit.notebook_slug)}?note={encodeURIComponent(hit.id)}">
											<span class="hit-head">
												<span class="hit-title">{hit.title || 'Untitled note'}</span>
												<span class="hit-where">in {hit.notebook_title}</span>
											</span>
											{#if hit.snippet}<span class="snippet">{hit.snippet}</span>{/if}
										</a>
									</li>
								{/each}
							</ul>
						{/if}
						{#if searchResults.chats.length}
							<h2>Conversations</h2>
							<ul class="hits">
								{#each searchResults.chats as hit (`${hit.notebook_slug}-${hit.chat_id}-${hit.turn}`)}
									<li>
										<a href="/n/{encodeURIComponent(hit.notebook_slug)}?c={encodeURIComponent(hit.chat_id)}">
											<span class="hit-head">
												<span class="hit-title">{hit.question}</span>
												<span class="hit-where">in {hit.notebook_title}</span>
											</span>
											{#if hit.snippet}<span class="snippet">{hit.snippet}</span>{/if}
										</a>
									</li>
								{/each}
							</ul>
						{/if}
					</div>
				{/if}
			{:else}
				<!-- Set like a contents page: title, dot leader, the count. -->
				<ul class="list">
					{#each notebooks as nb (nb.slug)}
						<li>
							<a href="/n/{encodeURIComponent(nb.slug)}">
								<span class="entry">
									<span class="title">{nb.title}</span>
									<span class="leader" aria-hidden="true"></span>
									<span class="count">{count(nb.sources.length)}</span>
								</span>
								<span class="meta">Updated {ago(nb.updated)}</span>
							</a>
						</li>
					{/each}
				</ul>
			{/if}
		{/if}
	</main>
</div>

<style>
	.page {
		width: min(100% - 2.5rem, 46rem);
		margin: 0 auto;
		padding: 0 0 6rem;
	}
	/* The running head, as on the notebook page. */
	.masthead {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		padding: 1rem 0 0.7rem;
		border-bottom: 1px solid var(--line);
	}
	.masthead-tools {
		display: flex;
		gap: 1.25rem;
		align-items: baseline;
	}
	.settings {
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
		text-decoration: none;
	}
	.settings:hover {
		color: var(--ink);
	}
	main {
		padding-top: 4rem;
	}
	.title-row {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 1rem;
		margin-bottom: 2.5rem;
	}
	h1 {
		margin: 0;
		font-size: var(--t-display);
		font-weight: 500;
		font-variation-settings: 'opsz' 72;
		line-height: 1.08;
		letter-spacing: -0.015em;
	}
	.primary {
		padding: calc(0.45rem - var(--cap-lift)) 1rem calc(0.45rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: none;
		color: var(--rubric);
		font-size: var(--t-s);
		font-weight: 600;
		cursor: pointer;
	}
	.primary:hover {
		background: var(--highlight);
	}
	.primary:disabled {
		border-color: var(--line);
		color: var(--ink-2);
		background: none;
		cursor: default;
	}
	.quiet {
		padding: 0.45rem 0.5rem;
		border: 0;
		background: none;
		color: var(--ink-2);
		font-style: italic;
		cursor: pointer;
	}
	.new {
		display: flex;
		gap: 0.75rem;
		margin-bottom: 2rem;
	}
	.new input {
		flex: 1;
		min-width: 0;
		padding: calc(0.45rem - var(--cap-lift)) 0.75rem calc(0.45rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--raised);
		box-shadow: inset 0 0 0 1px var(--edge);
		font-size: var(--t-m);
	}
	.new input::placeholder {
		font-style: italic;
	}
	.new input:focus {
		outline: 1px solid var(--ink);
		outline-offset: 0;
	}
	.list {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.list a {
		display: block;
		padding: 0.9rem 0;
		color: inherit;
		text-decoration: none;
	}
	.entry {
		display: flex;
		align-items: baseline;
		gap: 0.6rem;
	}
	.title {
		font-size: var(--t-xl);
		font-variation-settings: 'opsz' 36;
		line-height: 1.25;
	}
	.list a:hover .title {
		color: var(--rubric);
	}
	.leader {
		flex: 1;
		min-width: 2rem;
		border-bottom: 1px dotted var(--edge);
		transform: translateY(-0.3em);
	}
	.count {
		font-size: var(--t-s);
		font-style: italic;
		color: var(--ink-2);
		white-space: nowrap;
	}
	.meta {
		display: block;
		margin-top: 0.2rem;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.empty {
		max-width: 30rem;
		color: var(--ink-2);
		font-size: var(--t-m);
		font-style: italic;
	}
	.problem {
		color: var(--danger);
	}
	.create-error {
		margin: -1.25rem 0 2rem;
	}
	.library-search {
		margin-bottom: 1.75rem;
	}
	.library-search input {
		width: 100%;
		padding: calc(0.5rem - var(--cap-lift)) 0.75rem calc(0.5rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--raised);
		box-shadow: inset 0 0 0 1px var(--edge);
		font-size: var(--t-s);
	}
	.library-search input::placeholder {
		font-style: italic;
	}
	.library-search input:focus {
		outline: 1px solid var(--ink);
		outline-offset: 0;
	}
	.search-results h2 {
		margin: 1.5rem 0 0.5rem;
		font-size: var(--t-s);
		font-style: italic;
		font-weight: 400;
		color: var(--ink-2);
	}
	.search-results h2:first-child {
		margin-top: 0;
	}
	.hits {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.hits li + li {
		border-top: 1px solid var(--line);
	}
	.hits a {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		padding: 0.75rem 0;
		color: inherit;
		text-decoration: none;
	}
	.hits a:hover .hit-title {
		color: var(--rubric);
	}
	.hit-head {
		display: flex;
		flex-wrap: wrap;
		gap: 0.45rem;
		align-items: baseline;
	}
	.hit-title {
		font-weight: 600;
	}
	.hit-where {
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.snippet {
		font-size: var(--t-xs);
		color: var(--ink-2);
		line-height: 1.45;
	}
	@media (max-width: 719px) {
		main {
			padding-top: 2.5rem;
		}
		h1 {
			font-size: var(--t-xxl);
		}
		.title {
			font-size: var(--t-l);
		}
	}
</style>
