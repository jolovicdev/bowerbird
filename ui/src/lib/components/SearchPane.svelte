<script lang="ts">
	import { tick } from 'svelte';
	import { api, describe, type ChatHit, type NoteHit, type NotebookSearch, type PassageHit } from '#lib/api.ts';
	import Sheet from './Sheet.svelte';

	let {
		slug,
		modal = false,
		onclose,
		onpassage,
		onnote,
		onchat
	}: {
		slug: string;
		modal?: boolean;
		onclose: () => void;
		onpassage: (hit: PassageHit, query: string) => void;
		onnote: (hit: NoteHit) => void;
		onchat: (hit: ChatHit) => void;
	} = $props();

	let query = $state('');
	let includeExcluded = $state(false);
	let filter = $state<'all' | 'passages' | 'notes' | 'chats'>('all');
	let results = $state<NotebookSearch | null>(null);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let field = $state<HTMLInputElement>();

	$effect(() => {
		tick().then(() => field?.focus());
	});

	let runId = 0;
	async function runSearch(q: string, allSources: boolean) {
		const id = ++runId;
		const trimmed = q.trim();
		if (!trimmed) {
			results = null;
			error = null;
			loading = false;
			return;
		}
		loading = true;
		error = null;
		try {
			const res = await api.searchNotebook(slug, trimmed, allSources);
			if (id !== runId) return;
			results = res;
		} catch (e) {
			if (id !== runId) return;
			error = describe(e);
		} finally {
			if (id === runId) loading = false;
		}
	}

	$effect(() => {
		const q = query;
		const allSources = includeExcluded;
		const timer = setTimeout(() => runSearch(q, allSources), 200);
		return () => clearTimeout(timer);
	});

	const total = $derived(
		results ? results.passages.length + results.notes.length + results.chats.length : 0
	);
</script>

<Sheet label="Search this notebook" mark="⌕" {modal} closeLabel="Close search" {onclose}>
	{#snippet heading()}
		<h2>Search notebook</h2>
		<p class="sub">Finds matching passages (keyword &amp; meaning), notes, and conversations without calling a model.</p>
	{/snippet}

	{#snippet top()}
		<form
			class="search-bar"
			onsubmit={(e) => {
				e.preventDefault();
				runSearch(query, includeExcluded);
			}}
		>
			<label class="visually-hidden" for="nb-search-input">Search query</label>
			<input
				id="nb-search-input"
				type="search"
				placeholder="Search passages, notes, and chats…"
				bind:value={query}
				bind:this={field}
			/>
			<label class="toggle">
				<input class="check quiet" type="checkbox" bind:checked={includeExcluded} />
				Include switched-off sources
			</label>
		</form>
		{#if results && total > 0}
			<div class="tabs" role="tablist" aria-label="Filter results">
				<button class:active={filter === 'all'} onclick={() => (filter = 'all')}>All ({total})</button>
				<button class:active={filter === 'passages'} onclick={() => (filter = 'passages')}>
					Passages ({results.passages.length})
				</button>
				<button class:active={filter === 'notes'} onclick={() => (filter = 'notes')}>
					Notes ({results.notes.length})
				</button>
				<button class:active={filter === 'chats'} onclick={() => (filter = 'chats')}>
					Chats ({results.chats.length})
				</button>
			</div>
		{/if}
	{/snippet}

	<div class="results">
		{#if error}
			<p class="problem" role="alert">{error}</p>
		{:else if loading && !results}
			<p class="status">Searching…</p>
		{:else if results && total === 0}
			<p class="status">No matches for “{results.query}”.</p>
		{:else if results}
			{#if (filter === 'all' || filter === 'passages') && results.passages.length}
				<section class="group">
					<h3>Passages</h3>
					<ul>
						{#each results.passages as hit (hit.label)}
							<li>
								<button class="hit" onclick={() => onpassage(hit, results!.query)}>
									<span class="hit-head">
										<span class="siglum">{hit.letter}</span>
										<span class="hit-title">{hit.source_title}</span>
										<span class="hit-where">
											{hit.page_start ? `p. ${hit.page_start}` : `§${hit.ord + 1}`}{#if hit.section}, {hit.section}{/if}
										</span>
									</span>
									<span class="snippet">{hit.snippet}</span>
								</button>
							</li>
						{/each}
					</ul>
				</section>
			{/if}

			{#if (filter === 'all' || filter === 'notes') && results.notes.length}
				<section class="group">
					<h3>Notes</h3>
					<ul>
						{#each results.notes as hit (hit.id)}
							<li>
								<button class="hit" onclick={() => onnote(hit)}>
									<span class="hit-title">{hit.title || 'Untitled note'}</span>
									{#if hit.snippet}<span class="snippet">{hit.snippet}</span>{/if}
								</button>
							</li>
						{/each}
					</ul>
				</section>
			{/if}

			{#if (filter === 'all' || filter === 'chats') && results.chats.length}
				<section class="group">
					<h3>Conversations</h3>
					<ul>
						{#each results.chats as hit (`${hit.chat_id}-${hit.turn}`)}
							<li>
								<button class="hit" onclick={() => onchat(hit)}>
									<span class="hit-title">{hit.question}</span>
									<span class="hit-where">Turn {hit.turn}</span>
									{#if hit.snippet}<span class="snippet">{hit.snippet}</span>{/if}
								</button>
							</li>
						{/each}
					</ul>
				</section>
			{/if}
		{:else}
			<p class="status">Type a word, phrase, or concept to search this notebook instantly.</p>
		{/if}
	</div>
</Sheet>

<style>
	h2 {
		margin: 0;
		font-size: var(--t-l);
		font-style: italic;
		font-weight: 400;
		font-variation-settings: 'opsz' 36;
		line-height: 1.3;
	}
	.sub {
		margin: 0.3rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.search-bar {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
		padding: 0.9rem 1.5rem 0.75rem;
		border-bottom: 1px solid var(--line);
	}
	.search-bar input[type='search'] {
		width: 100%;
		padding: calc(0.45rem - var(--cap-lift)) 0.65rem calc(0.45rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--page);
		box-shadow: inset 0 0 0 1px var(--edge);
		font-size: var(--t-s);
	}
	.search-bar input[type='search']:focus {
		outline: 1px solid var(--ink);
		outline-offset: 0;
	}
	.toggle {
		display: flex;
		gap: 0.4rem;
		align-items: center;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
		cursor: pointer;
	}
	.tabs {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.9rem;
		padding: 0.5rem 1.5rem;
		border-bottom: 1px solid var(--line);
	}
	.tabs button {
		padding: 0.2rem 0;
		border: 0;
		background: none;
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
		cursor: pointer;
	}
	.tabs button.active {
		color: var(--rubric);
		font-style: normal;
		font-weight: 600;
	}
	.results {
		font-size: var(--t-s);
	}
	.status {
		color: var(--ink-2);
		font-style: italic;
	}
	.problem {
		color: var(--danger);
	}
	.group + .group {
		margin-top: 1.5rem;
	}
	h3 {
		margin: 0 0 0.5rem;
		font-size: var(--t-xs);
		font-style: italic;
		font-weight: 400;
		color: var(--ink-2);
	}
	ul {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	li + li {
		border-top: 1px solid var(--line);
	}
	.hit {
		display: flex;
		flex-direction: column;
		gap: 0.2rem;
		width: 100%;
		padding: 0.65rem 0;
		border: 0;
		background: none;
		color: inherit;
		text-align: left;
		cursor: pointer;
	}
	.hit:hover .hit-title {
		color: var(--rubric);
	}
	.hit-head {
		display: flex;
		flex-wrap: wrap;
		gap: 0.35rem;
		align-items: baseline;
	}
	.siglum {
		color: var(--rubric);
		font-style: italic;
		font-weight: 600;
	}
	.hit-title {
		font-weight: 600;
		line-height: 1.3;
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
		display: -webkit-box;
		overflow: hidden;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 3;
		line-clamp: 3;
	}
</style>
