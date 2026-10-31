<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { api, describe, type Citation, type Note, type Notebook } from '#lib/api.ts';
	import type { Editor, format } from '#lib/editor.ts';
	import Sheet from './Sheet.svelte';

	let {
		slug,
		id,
		modal = false,
		onclose,
		onopen,
		onchange,
		ondelete,
		onsource
	}: {
		slug: string;
		id: string;
		modal?: boolean;
		onclose: () => void;
		/** Opens a cited passage; the sheet offers the way back to the note. */
		onopen: (citations: Citation[], el: HTMLElement) => void;
		/** The note was saved; its title may have changed. */
		onchange: (note: Note) => void;
		ondelete: () => void;
		/** The note was added as a source. */
		onsource: (notebook: Notebook) => void;
	} = $props();

	let note = $state<Note | null>(null);
	let title = $state('');
	let problem = $state<string | null>(null);
	let status = $state<'saved' | 'saving' | 'unsaved'>('saved');
	let confirming = $state(false);
	let adding = $state(false);
	let place = $state<HTMLElement>();
	let editor: Editor | null = null;
	let tools = $state<typeof format | null>(null);
	let timer: ReturnType<typeof setTimeout> | undefined;
	let destroyed = false;

	onMount(async () => {
		try {
			const [loaded, module] = await Promise.all([api.note(slug, id), import('#lib/editor.ts')]);
			if (destroyed || !place) return;
			const { createEditor } = module;
			tools = module.format;
			note = loaded;
			title = loaded.title;
			editor = createEditor(place, loaded.body, loaded.citations, {
				label: 'Note',
				change: schedule,
				open: (citations, el) => onopen(citations, el)
			});
			if (!loaded.body.trim()) editor.view.focus();
		} catch (e) {
			problem = describe(e);
		}
	});

	function schedule() {
		status = 'unsaved';
		clearTimeout(timer);
		timer = setTimeout(save, 700);
	}

	async function save() {
		clearTimeout(timer);
		if (!editor || status === 'saved') return;
		status = 'saving';
		try {
			const saved = await api.saveNote(slug, id, title, editor.markdown());
			if (status === 'saving') status = 'saved';
			if (!destroyed) {
				note = saved;
				editor.check(saved.citations);
			}
			onchange(saved);
		} catch (e) {
			status = 'unsaved';
			problem = describe(e);
		}
	}

	// Leaving with edits not yet sent saves them on the way out.
	onDestroy(() => {
		destroyed = true;
		save();
		editor?.destroy();
	});

	async function remove() {
		try {
			clearTimeout(timer);
			status = 'saved';
			await api.deleteNote(slug, id);
			ondelete();
		} catch (e) {
			problem = describe(e);
		}
	}

	async function addAsSource() {
		adding = true;
		problem = null;
		try {
			await save();
			onsource(await api.noteToSource(slug, id));
		} catch (e) {
			problem = describe(e);
		} finally {
			adding = false;
		}
	}

	const statusText = $derived(status === 'saved' ? 'Saved' : status === 'saving' ? 'Saving…' : 'Edited');
</script>

<Sheet label={title || 'Note'} mark="¶" {modal} closeLabel="Close note" {onclose}>
	{#snippet heading()}
		<label class="visually-hidden" for="note-title">Note title</label>
		<textarea
			id="note-title"
			class="title"
			rows="1"
			bind:value={title}
			placeholder="Untitled note"
			oninput={schedule}
			onkeydown={(e) => {
				if (e.key === 'Enter') {
					e.preventDefault();
					editor?.view.focus();
				}
			}}
		></textarea>
		<p class="meta">
			<span role="status">{note ? statusText : 'Opening…'}</span>
		</p>
		{#if note && !confirming}
			<p class="actions">
				<button class="action" onclick={addAsSource} disabled={adding}>{adding ? 'Adding…' : 'Add as source'}</button>
				<button class="action" onclick={() => (confirming = true)}>Delete</button>
			</p>
		{/if}
	{/snippet}
	{#snippet top()}
		{#if confirming}
			<div class="confirm" role="alert">
				<p>Delete this note? This can't be undone.</p>
				<p class="actions">
					<button class="action" onclick={remove}>Delete note</button>
					<button class="action quiet" onclick={() => (confirming = false)}>Cancel</button>
				</p>
			</div>
		{/if}
		{#if problem}<p class="problem" role="alert">{problem}</p>{/if}
		{#if note && tools}
			<div class="toolbar" role="toolbar" aria-label="Formatting">
				{#each Object.entries(tools) as [name, item] (name)}
					<button class="tool tool-{name}" onmousedown={(e) => e.preventDefault()} onclick={() => editor?.run(item.command)}>
						{item.label}
					</button>
				{/each}
			</div>
		{/if}
	{/snippet}
	<div class="editor" bind:this={place}></div>
	{#if note && !note.body.trim()}
		<p class="hint">
			Write here. Start a line with # for a heading, - for a list, or &gt; for a quote. Saved answers keep their
			citations.
		</p>
	{/if}
</Sheet>

<style>
	.title {
		display: block;
		width: 100%;
		field-sizing: content;
		resize: none;
		padding: 0;
		border: 0;
		outline: 0;
		background: none;
		font-size: var(--t-l);
		font-style: italic;
		font-variation-settings: 'opsz' 36;
		line-height: 1.3;
	}
	.title::placeholder {
		color: var(--ink-2);
	}
	.meta {
		margin: 0.3rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.actions {
		display: flex;
		gap: 1rem;
		margin: 0.25rem 0 0;
	}
	.action {
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: var(--t-xs);
		font-weight: 600;
		cursor: pointer;
	}
	.action:disabled {
		color: var(--ink-2);
		font-weight: 400;
	}
	.action.quiet {
		color: var(--ink-2);
		font-weight: 400;
	}
	.confirm {
		padding: 1rem 1.5rem;
		border-bottom: 1px solid var(--line);
		font-size: var(--t-s);
	}
	.confirm p {
		margin: 0;
	}
	.problem {
		margin: 0;
		padding: 0.75rem 1.5rem 0;
		font-style: italic;
		color: var(--danger);
	}
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 1rem;
		padding: 0.6rem 1.5rem;
		border-bottom: 1px solid var(--line);
	}
	.tool {
		min-width: 1.5rem;
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--ink-2);
		font-size: var(--t-xs);
		cursor: pointer;
	}
	.tool:hover {
		color: var(--ink);
	}
	.tool-bold {
		font-weight: 700;
	}
	.tool-italic {
		font-style: italic;
	}
	.editor :global(.prose) {
		min-height: 12rem;
		outline: 0;
		font-size: var(--t-m);
		line-height: 1.62;
		white-space: pre-wrap;
		overflow-wrap: break-word;
	}
	.editor :global(.prose p) {
		margin: 0 0 0.9em;
	}
	.editor :global(.prose h1),
	.editor :global(.prose h2),
	.editor :global(.prose h3) {
		margin: 1.2em 0 0.5em;
		font-size: var(--t-l);
		font-style: italic;
		font-weight: 600;
		line-height: 1.3;
	}
	.editor :global(.prose blockquote) {
		margin: 0 0 0.9em;
		padding-left: 0.9rem;
		border-left: 1px solid var(--rubric);
		color: var(--ink-2);
	}
	.editor :global(.prose ul),
	.editor :global(.prose ol) {
		margin: 0 0 0.9em;
		padding-left: 1.3rem;
	}
	.editor :global(.prose code) {
		font-family: var(--mono);
		font-size: 0.9em;
	}
	.editor :global(.prose sup.ref) {
		font-size: 0.72em;
		line-height: 0;
		vertical-align: super;
	}
	.editor :global(.prose .mark) {
		padding: 0 0.12em;
		border: 0;
		border-radius: 2px;
		background: none;
		color: var(--rubric);
		font: italic 600 1em/1 var(--serif);
		cursor: pointer;
	}
	.editor :global(.prose .mark:hover) {
		background: var(--highlight);
	}
	.editor :global(.prose sup.ref.repeat) {
		display: none;
	}
	.editor :global(.prose .unverified .mark) {
		color: var(--ink-2);
	}
	.editor :global(.prose .unverified .mark::after) {
		content: '†';
	}
	.hint {
		margin: 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
</style>
