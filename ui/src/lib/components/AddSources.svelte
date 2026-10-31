<script lang="ts">
	import { UPLOAD_MB, type Found, type Research } from '#lib/api.ts';

	let {
		onfiles,
		onurl,
		ontext,
		onfind
	}: {
		onfiles: (files: File[]) => void;
		/** Resolve to an error message, or null once the source is added. */
		onurl: (url: string) => Promise<string | null>;
		ontext: (title: string, text: string) => Promise<string | null>;
		/** Searches the web for pages about a question; resolves to an error message on failure. */
		onfind: (question: string) => Promise<Research | string>;
	} = $props();

	import { closeOnBack, popLayer, pushLayer } from '#lib/layers.svelte.ts';

	let dialog: HTMLDialogElement;
	let url = $state('');
	let title = $state('');
	let text = $state('');
	let over = $state(false);
	let busy = $state<'url' | 'text' | null>(null);
	let urlError = $state<string | null>(null);
	let textError = $state<string | null>(null);

	export function show() {
		dialog.showModal();
		pushLayer('dialog');
	}

	closeOnBack('dialog', () => dialog?.open, () => dialog.close());

	let question = $state('');
	let finding = $state(false);
	let found = $state<Found[] | null>(null);
	let chosen = $state(new Set<string>());
	let findError = $state<string | null>(null);
	let progress = $state<{ done: number; total: number } | null>(null);
	let failures = $state<string[]>([]);

	function reset() {
		url = '';
		title = '';
		text = '';
		urlError = null;
		textError = null;
		found = null;
		findError = null;
		failures = [];
	}

	async function find(event: SubmitEvent) {
		event.preventDefault();
		finding = true;
		findError = null;
		found = null;
		failures = [];
		const result = await onfind(question.trim());
		finding = false;
		if (typeof result === 'string') {
			findError = result;
			return;
		}
		found = result.results;
		// The best few new pages start ticked; the reader adjusts from there.
		chosen = new Set(result.results.filter((r) => !r.already).slice(0, 4).map((r) => r.url));
	}

	function toggle(link: string) {
		const next = new Set(chosen);
		if (!next.delete(link)) next.add(link);
		chosen = next;
	}

	async function addChosen() {
		const links = [...chosen];
		failures = [];
		progress = { done: 0, total: links.length };
		for (const link of links) {
			const problem = await onurl(link);
			if (problem) failures.push(`${new URL(link).hostname}: ${problem}`);
			progress = { done: progress.done + 1, total: links.length };
		}
		progress = null;
		if (!failures.length) dialog.close();
	}

	const host = (link: string) => new URL(link).hostname.replace(/^www\./, '');

	function pick(files: FileList | null | undefined) {
		if (!files?.length) return;
		onfiles([...files]);
		dialog.close();
	}

	// Links and pasted text stay in the dialog until they're added, so a
	// failure keeps what was typed next to the reason.
	async function addUrl(event: SubmitEvent) {
		event.preventDefault();
		busy = 'url';
		urlError = await onurl(url.trim());
		busy = null;
		if (!urlError) dialog.close();
	}

	async function addText(event: SubmitEvent) {
		event.preventDefault();
		busy = 'text';
		textError = await ontext(title.trim(), text);
		busy = null;
		if (!textError) dialog.close();
	}
</script>

<dialog
	bind:this={dialog}
	aria-labelledby="add-title"
	onclose={() => {
		popLayer('dialog');
		reset();
	}}
>
	<header>
		<h2 id="add-title">Add sources</h2>
		<button class="close" onclick={() => dialog.close()} aria-label="Close">
			<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" /></svg>
		</button>
	</header>

	<label
		class="drop"
		class:over
		ondragover={(e) => {
			e.preventDefault();
			e.stopPropagation();
			over = true;
		}}
		ondragleave={() => (over = false)}
		ondrop={(e) => {
			e.preventDefault();
			e.stopPropagation();
			over = false;
			pick(e.dataTransfer?.files);
		}}
	>
		<input
			class="visually-hidden"
			type="file"
			multiple
			accept=".pdf,.docx,.epub,.md,.markdown,.txt,.mp3,.m4a,.aac,.wav,.ogg,.oga,.opus,.flac,.webm"
			onchange={(e) => pick(e.currentTarget.files)}
		/>
		<span class="drop-main"><span class="link">Choose files</span> or drop them here</span>
		<span class="hint">PDF, Word, EPUB, Markdown, plain text, or a recording, up to {UPLOAD_MB} MB each</span>
	</label>

	<form class="field" onsubmit={addUrl}>
		<label for="add-url">Web page, PDF, or YouTube link</label>
		<div class="inline">
			<input
				id="add-url"
				type="url"
				placeholder="https://"
				bind:value={url}
				required
				aria-invalid={urlError ? 'true' : undefined}
				aria-describedby={urlError ? 'add-url-error' : undefined}
			/>
			<button disabled={!url.trim() || busy !== null}>{busy === 'url' ? 'Adding…' : 'Add link'}</button>
		</div>
		{#if urlError}<p class="problem" id="add-url-error" role="alert">{urlError}</p>{/if}
	</form>

	<details>
		<summary>Paste text</summary>
		<form class="field" onsubmit={addText}>
			<label for="add-title-field">Title</label>
			<input id="add-title-field" placeholder="Pasted text" bind:value={title} />
			<label for="add-text">Text</label>
			<textarea id="add-text" rows="6" bind:value={text} required></textarea>
			{#if textError}<p class="problem" role="alert">{textError}</p>{/if}
			<button disabled={!text.trim() || busy !== null}>{busy === 'text' ? 'Adding…' : 'Add text'}</button>
		</form>
	</details>

	<details>
		<summary>Find sources on the web</summary>
		<form class="field" onsubmit={find}>
			<label for="find-question">What do you want to read about?</label>
			<div class="inline">
				<input
					id="find-question"
					bind:value={question}
					placeholder="How did the Peace of Westphalia change diplomacy?"
					required
				/>
				<button disabled={!question.trim() || finding}>{finding ? 'Searching…' : 'Search'}</button>
			</div>
			{#if finding}<p class="hint" role="status">Writing searches and reading the results. This takes about half a minute.</p>{/if}
			{#if findError}<p class="problem" role="alert">{findError}</p>{/if}
		</form>
		{#if found}
			{#if found.length === 0}
				<p class="hint">Nothing found. Try wording the question differently.</p>
			{:else}
				<ul class="found">
					{#each found as page (page.url)}
						<li>
							<label class:already={page.already}>
								<input
									class="check"
									type="checkbox"
									checked={chosen.has(page.url)}
									disabled={page.already !== null || progress !== null}
									onchange={() => toggle(page.url)}
								/>
								<span class="found-text">
									<span class="found-title">{page.title}</span>
									<span class="hint">
										{host(page.url)}{#if page.already}, already source {page.already}{/if}
									</span>
									{#if page.snippet}<span class="snippet">{page.snippet}</span>{/if}
								</span>
							</label>
						</li>
					{/each}
				</ul>
				<div class="field found-actions">
					<button disabled={chosen.size === 0 || progress !== null} onclick={addChosen}>
						{progress
							? `Adding ${progress.done + 1} of ${progress.total}…`
							: chosen.size === 1
								? 'Add 1 source'
								: `Add ${chosen.size} sources`}
					</button>
					{#each failures as failure (failure)}<p class="problem" role="alert">{failure}</p>{/each}
				</div>
			{/if}
		{/if}
	</details>
</dialog>

<style>
	dialog {
		width: min(100% - 2rem, 32rem);
		max-height: 90dvh;
		overflow-y: auto;
		padding: 0;
		border: 0;
		border-radius: 6px;
		background: var(--raised);
		color: var(--ink);
		box-shadow: 0 0 0 1px var(--line);
	}
	dialog::backdrop {
		background: var(--scrim);
	}
	header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 1.25rem 1rem 0.5rem 1.5rem;
	}
	h2 {
		margin: 0;
		font-size: var(--t-xl);
		font-style: italic;
		font-weight: 400;
		font-variation-settings: 'opsz' 48;
	}
	.close {
		display: grid;
		place-items: center;
		width: 2.25rem;
		height: 2.25rem;
		border: 0;
		border-radius: 6px;
		background: transparent;
		color: var(--ink-2);
		cursor: pointer;
	}
	.close svg {
		width: 1rem;
		height: 1rem;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.75;
		stroke-linecap: round;
	}
	.drop {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		align-items: center;
		margin: 0.75rem 1.5rem 1.25rem;
		padding: 2rem 1rem;
		border: 1px dashed var(--edge);
		border-radius: 4px;
		text-align: center;
		cursor: pointer;
	}
	.drop.over,
	.drop:hover {
		background: var(--page);
	}
	.drop:has(:focus-visible) {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}
	.link {
		color: var(--rubric);
		font-weight: 600;
	}
	.hint {
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.field {
		display: flex;
		flex-direction: column;
		gap: 0.4rem;
		padding: 0 1.5rem 1.25rem;
	}
	.field label {
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.inline {
		display: flex;
		gap: 0.5rem;
	}
	input:not([type='file'], [type='checkbox']),
	textarea {
		flex: 1;
		min-width: 0;
		padding: 0.5rem 0.65rem;
		border: 0;
		border-radius: 4px;
		background: var(--page);
		box-shadow: inset 0 0 0 1px var(--edge);
		font-size: var(--t-s);
	}
	input:not([type='file'], [type='checkbox']) {
		padding-block: calc(0.5rem - var(--cap-lift)) calc(0.5rem + var(--cap-lift));
	}
	input:focus,
	textarea:focus {
		outline: 1px solid var(--ink);
		outline-offset: 0;
	}
	textarea {
		resize: vertical;
	}
	.field button {
		align-self: flex-start;
		padding: calc(0.45rem - var(--cap-lift)) 1rem calc(0.45rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		font-weight: 600;
		cursor: pointer;
	}
	.field button:disabled {
		border-color: var(--line);
		background: transparent;
		color: var(--ink-2);
		cursor: default;
	}
	details {
		padding: 0 1.5rem 1.5rem;
	}
	details .field {
		padding: 0.75rem 0 0;
	}
	summary {
		display: flex;
		gap: 0.4rem;
		align-items: center;
		color: var(--rubric);
		font-style: italic;
		list-style: none;
		cursor: pointer;
	}
	summary::-webkit-details-marker {
		display: none;
	}
	summary::before {
		content: '';
		width: 0.4rem;
		height: 0.4rem;
		border: solid currentColor;
		border-width: 0 2px 2px 0;
		transform: rotate(-45deg);
		transition: transform 160ms var(--ease);
	}
	details[open] summary::before {
		transform: rotate(45deg);
	}
	.problem {
		margin: 0;
		font-size: var(--t-xs);
		color: var(--danger);
	}
	.found {
		margin: 1rem 0 0;
		padding: 0;
		list-style: none;
	}
	.found li + li {
		border-top: 1px solid var(--line);
	}
	.found label {
		display: flex;
		gap: 0.75rem;
		align-items: flex-start;
		padding: 0.6rem 0;
		cursor: pointer;
	}
	.found label.already {
		color: var(--ink-2);
		cursor: default;
	}
	.found .check {
		margin-top: 0.3rem;
	}
	.found-text {
		display: flex;
		flex-direction: column;
		min-width: 0;
		gap: 0.1rem;
	}
	.found-title {
		font-weight: 600;
		line-height: 1.3;
	}
	.snippet {
		display: -webkit-box;
		overflow: hidden;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		font-size: var(--t-xs);
		color: var(--ink-2);
	}
	.found-actions {
		padding: 0.75rem 0 0;
	}

	/* Phones: a bottom sheet, like the passage sheet. */
	@media (max-width: 719px) {
		dialog {
			width: 100%;
			max-width: none;
			max-height: 90dvh;
			margin: auto 0 0;
			border-radius: 14px 14px 0 0;
			padding-bottom: env(safe-area-inset-bottom);
		}
	}
</style>
