<script lang="ts">
	import { tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { api, describe, type Notebook } from '#lib/api.ts';

	let {
		notebook,
		error,
		covered,
		onretry,
		oncontents,
		onstyle,
		onrenamed
	}: {
		notebook: Notebook | null;
		/** Why the notebook didn't open. */
		error: string | null;
		/** Something covers the page, so the heading can't be used. */
		covered: boolean;
		onretry: () => void;
		oncontents: () => void;
		onstyle: () => void;
		onrenamed: (next: Notebook) => void;
	} = $props();

	const STYLE_NAMES = { default: '', tutor: 'Tutor', custom: 'your own' } as const;

	let renaming = $state(false);
	let newTitle = $state('');
	let deleting = $state(false);
	let titleError = $state<string | null>(null);
	let titleVisible = $state(true);
	let heading = $state<HTMLElement>();
	let renameButton = $state<HTMLButtonElement>();
	let deleteButton = $state<HTMLButtonElement>();
	let cancelDelete = $state<HTMLButtonElement>();

	// On phones the header keeps the notebook's name once its title scrolls away.
	$effect(() => {
		if (!heading) return;
		const watcher = new IntersectionObserver(([entry]) => (titleVisible = entry.isIntersecting));
		watcher.observe(heading);
		return () => watcher.disconnect();
	});

	async function startRename() {
		if (!notebook) return;
		newTitle = notebook.title;
		titleError = null;
		renaming = true;
		await tick();
		const input = document.getElementById('notebook-title');
		input?.focus();
		// The title block scrolls under the sticky header once a conversation is read.
		input?.scrollIntoView({ block: 'center' });
	}

	async function stopRenaming() {
		renaming = false;
		await tick();
		renameButton?.focus();
	}

	async function startDelete() {
		deleting = true;
		titleError = null;
		await tick();
		cancelDelete?.focus();
		cancelDelete?.scrollIntoView({ block: 'center' });
	}

	async function stopDeleting() {
		deleting = false;
		await tick();
		deleteButton?.focus();
	}

	async function saveTitle(event: SubmitEvent) {
		event.preventDefault();
		if (!notebook) return;
		try {
			onrenamed(await api.renameNotebook(notebook.slug, newTitle.trim()));
			await stopRenaming();
		} catch (e) {
			titleError = describe(e);
		}
	}

	async function deleteNotebook() {
		if (!notebook) return;
		try {
			await api.deleteNotebook(notebook.slug);
			await goto('/');
		} catch (e) {
			titleError = describe(e);
		}
	}
</script>

<header class="head" inert={covered}>
	<a class="back" href="/">Notebooks</a>
	{#if notebook}
		<button class="action quiet contents-toggle" onclick={oncontents}>Contents</button>
		<span class="head-title" class:shown={!titleVisible} aria-hidden="true">{notebook.title}</span>
		{#if !renaming && !deleting}
			<span class="tools">
				<button
					class="action quiet"
					aria-label="Answer style{notebook.chat.goal !== 'default' ? `: ${STYLE_NAMES[notebook.chat.goal]}` : ''}"
					onclick={onstyle}
				>
					<span class="long">Answer style</span><span class="short">Style</span>{#if notebook.chat.goal !== 'default'}: {STYLE_NAMES[notebook.chat.goal]}{/if}
				</button>
				<button class="action quiet" bind:this={renameButton} onclick={startRename}>Rename</button>
				<button class="action quiet" bind:this={deleteButton} onclick={startDelete}>Delete</button>
			</span>
		{/if}
	{/if}
</header>

<div class="title-block" inert={covered}>
	{#if error}
		<p class="problem">
			{error}
			<button class="action" onclick={onretry}>Try again</button>
		</p>
	{:else if !notebook}
		<p class="lede">Opening the notebook…</p>
	{:else}
		{#if renaming}
			<form class="rename" onsubmit={saveTitle}>
				<label class="visually-hidden" for="notebook-title">Notebook name</label>
				<input
					id="notebook-title"
					bind:value={newTitle}
					required
					onkeydown={(e) => e.key === 'Escape' && stopRenaming()}
				/>
				<button class="action" disabled={!newTitle.trim()}>Save</button>
				<button type="button" class="action quiet" onclick={stopRenaming}>Cancel</button>
			</form>
		{:else}
			<h1 bind:this={heading}>{notebook.title}</h1>
		{/if}
		{#if deleting}
			<div class="confirm" role="alert">
				<p>
					Delete {notebook.title}? Its folder moves to .trash in your Bowerbird library, so you can still get it back
					from there.
				</p>
				<p>
					<button class="action danger" onclick={deleteNotebook}>Delete notebook</button>
					<button class="action quiet" bind:this={cancelDelete} onclick={stopDeleting}>Cancel</button>
				</p>
			</div>
		{/if}
		{#if titleError}<p class="problem" role="alert">{titleError}</p>{/if}
	{/if}
</div>

<style>
	/* The running head, as at the top of a printed page. */
	.head {
		grid-area: head;
		position: sticky;
		top: 0;
		z-index: 5;
		display: flex;
		gap: 1.5rem;
		align-items: baseline;
		padding: 1rem 0 0.7rem;
		border-bottom: 1px solid var(--line);
		background: var(--page);
		font-size: var(--t-xs);
		font-style: italic;
	}
	.back {
		padding: 0.3rem 0;
		color: var(--ink-2);
		text-decoration: none;
		white-space: nowrap;
	}
	.back::before {
		content: '‹ ';
		font-style: normal;
	}
	.back:hover {
		color: var(--ink);
	}
	.head-title {
		min-width: 0;
		overflow: hidden;
		color: var(--ink);
		text-overflow: ellipsis;
		white-space: nowrap;
		opacity: 0;
		transition: opacity 160ms var(--ease);
	}
	.head-title.shown {
		opacity: 1;
	}
	.tools {
		display: flex;
		gap: 1.25rem;
		margin-left: auto;
	}
	.title-block {
		grid-area: title;
		padding: 3.5rem 0 0.5rem;
	}
	h1 {
		margin: 0 0 2rem;
		font-size: var(--t-display);
		font-weight: 500;
		font-variation-settings: 'opsz' 72;
		line-height: 1.08;
		letter-spacing: -0.015em;
		text-wrap: balance;
	}
	.rename {
		display: flex;
		gap: 1rem;
		align-items: center;
		margin: 0 0 2rem;
	}
	.rename input {
		flex: 1;
		min-width: 0;
		padding: calc(0.25rem - var(--cap-lift)) 0.5rem calc(0.25rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--raised);
		box-shadow: inset 0 0 0 1px var(--edge);
		font-size: var(--t-xl);
	}
	.action {
		padding: 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: inherit;
		font-style: normal;
		font-weight: 600;
		cursor: pointer;
	}
	.action:disabled {
		color: var(--ink-2);
		font-weight: 400;
		cursor: default;
	}
	.action.quiet {
		color: var(--ink-2);
		font-style: italic;
		font-weight: 400;
	}
	.action.quiet:hover {
		color: var(--ink);
	}
	.action.danger {
		color: var(--danger);
	}
	.confirm {
		margin: -1.5rem 0 2rem;
		font-size: var(--t-s);
	}
	.confirm p {
		margin: 0 0 0.5rem;
	}
	.confirm .action + .action {
		margin-left: 1rem;
	}
	.lede {
		margin: 0 0 1.25rem;
		color: var(--ink-2);
		font-size: var(--t-m);
		font-style: italic;
	}
	.problem {
		color: var(--danger);
	}
	.problem .action {
		margin-left: 0.75rem;
	}
	.contents-toggle,
	.short {
		display: none;
	}
	.head .action {
		padding: 0.3rem 0;
		white-space: nowrap;
	}
	/* Fingers need more than the text's height to hit these. */
	@media (pointer: coarse) {
		.head .action,
		.back {
			padding: 0.65rem 0;
		}
	}

	/* Without room for the contents column, the contents open as a drawer
	   from the running head. */
	@media (max-width: 1239px) {
		.contents-toggle {
			display: inline;
		}
	}

	@media (max-width: 719px) {
		.head {
			margin-inline: -1rem;
			padding-inline: 1rem;
		}
		.title-block {
			padding-top: 2rem;
		}
		h1 {
			font-size: var(--t-xxl);
			margin-bottom: 1.5rem;
		}
		.contents-toggle,
		.long {
			display: none;
		}
		.short {
			display: inline;
		}
	}
</style>
