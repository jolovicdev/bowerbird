<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import { replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import {
		api,
		describe,
		type ChatStyle,
		type GuideState,
		type ModelProfile,
		type Notebook,
		type NoteSummary,
		type Source
	} from '#lib/api.ts';
	import { Additions } from '#lib/additions.svelte.ts';
	import { Conversation, scroller } from '#lib/conversation.svelte.ts';
	import { downloadBlob } from '#lib/drop.ts';
	import { Notices, type Notice } from '#lib/notices.svelte.ts';
	import { ReportDrafts } from '#lib/reports.svelte.ts';
	import { SheetState } from '#lib/sheet.svelte.ts';
	import { StudyRuns } from '#lib/studio.svelte.ts';
	import AddSources from '#lib/components/AddSources.svelte';
	import Composer from '#lib/components/Composer.svelte';
	import Contents from '#lib/components/Contents.svelte';
	import DropZone from '#lib/components/DropZone.svelte';
	import Guide from '#lib/components/Guide.svelte';
	import DraftPane from '#lib/components/DraftPane.svelte';
	import NotebookHeading from '#lib/components/NotebookHeading.svelte';
	import NotePane from '#lib/components/NotePane.svelte';
	import ReportDialog from '#lib/components/ReportDialog.svelte';
	import SearchPane from '#lib/components/SearchPane.svelte';
	import SourcePane from '#lib/components/SourcePane.svelte';
	import StudyDialog from '#lib/components/StudyDialog.svelte';
	import StudyPane from '#lib/components/StudyPane.svelte';
	import StyleDialog from '#lib/components/StyleDialog.svelte';
	import TableDialog from '#lib/components/TableDialog.svelte';
	import TurnView from '#lib/components/Turn.svelte';

	const slug = $derived(page.params.slug ?? '');
	// Below this width there is no margin, so passages open in a sheet over the text.
	const narrow = new MediaQuery('max-width: 959px');
	// From this width the contents are a column; below it they open over the page.
	const wide = new MediaQuery('min-width: 1240px');

	let notebook = $state<Notebook | null>(null);
	let error = $state<string | null>(null);
	let notes = $state<NoteSummary[]>([]);
	let profiles = $state<ModelProfile[]>([]);
	let defaultModel = $state('');
	/** Name of the saved profile picked for the next question, or '' for the notebook's own. */
	let selectedProfile = $state('');
	let guide = $state<GuideState | null>(null);
	let guideWriting = $state(false);
	let guideError = $state<string | null>(null);

	let contentsOpen = $state(false);
	let addDialog: AddSources;
	let studyDialog: StudyDialog;
	let styleDialog: StyleDialog;
	let reportDialog: ReportDialog;
	let tableDialog: TableDialog;
	let composer = $state<Composer>();
	let startButton = $state<HTMLButtonElement>();

	const notices = new Notices();
	const sheet = new SheetState(() => slug);
	const talk = new Conversation({
		slug: () => slug,
		notices,
		box: () => composer,
		choice: () => {
			const profile = profiles.find((p) => p.name === selectedProfile);
			return profile && { model: profile.model, thinking: profile.thinking };
		},
		onempty: () => writeGuideIfNeeded(),
		onnote: () => refreshNotes()
	});
	const studio = new StudyRuns({ slug: () => slug, notices, sheet });
	const reports = new ReportDrafts({ slug: () => slug, notices, sheet, onnote: () => refreshNotes() });
	const additions = new Additions({
		slug: () => slug,
		notices,
		notebook: () => notebook,
		onadded: (next) => updateNotebook(next)
	});

	const sources = $derived(new Map((notebook?.sources ?? []).map((s) => [s.id, s])));
	const paneSourceId = $derived.by(() => {
		const pane = sheet.pane;
		return pane && 'sourceId' in pane ? pane.sourceId : pane && 'citations' in pane ? pane.citations[0].source_id : null;
	});
	const paneSource = $derived(paneSourceId ? sources.get(paneSourceId) : undefined);
	const paneItem = $derived.by(() => {
		const pane = sheet.pane;
		return pane && 'noteId' in pane ? pane.noteId : pane && 'studyId' in pane ? pane.studyId : (paneSourceId ?? null);
	});
	const showingPane = $derived.by(() => {
		const pane = sheet.pane;
		return (
			pane !== null &&
			(paneSource !== undefined ||
				'noteId' in pane ||
				'studyId' in pane ||
				'draft' in pane ||
				'search' in pane)
		);
	});
	const modal = $derived(narrow.current && showingPane);
	/** Something covers the page, so what's behind it can't be used. */
	const covered = $derived(modal || (contentsOpen && !wide.current));
	const included = $derived(notebook?.sources.filter((s) => !s.excluded).length ?? 0);

	async function load() {
		error = null;
		try {
			const [nb, chatList, noteList, guideState] = await Promise.all([
				api.notebook(slug),
				api.chats(slug),
				api.notes(slug),
				api.guide(slug),
				studio.refresh()
			]);
			notebook = nb;
			talk.chats = chatList;
			notes = noteList;
			guide = guideState;
			api.settings()
				.then((s) => {
					profiles = s.profiles;
					defaultModel = s.chat_model;
				})
				.catch(() => {});
			const wanted = page.url.searchParams.get('c');
			talk.chat = chatList.find((c) => c.id === wanted)?.id ?? chatList[0]?.id ?? null;
			talk.turns = talk.chat ? await api.turns(slug, talk.chat) : [];
			await tick();
			if (talk.turns.length) scroller().scrollTop = scroller().scrollHeight;
			const here = talk.chat ? `${page.url.pathname}?c=${encodeURIComponent(talk.chat)}` : page.url.pathname;
			const openSourceId = page.url.searchParams.get('source');
			const openNoteId = page.url.searchParams.get('note');
			if (openSourceId && nb.sources.some((s) => s.id === openSourceId)) {
				const ordParam = Number(page.url.searchParams.get('ord') ?? '0');
				const qParam = page.url.searchParams.get('q') ?? '';
				replaceState(here, page.state);
				sheet.open({ sourceId: openSourceId, ord: Number.isFinite(ordParam) ? ordParam : 0, highlight: qParam });
			} else if (openNoteId && noteList.some((n) => n.id === openNoteId)) {
				replaceState(here, page.state);
				sheet.open({ noteId: openNoteId });
			}
			if (page.url.searchParams.has('add')) {
				replaceState(page.url.pathname, page.state);
				if (nb.sources.length === 0) {
					// The dialog hands focus back to what held it when it opened.
					startButton?.focus();
					addDialog.show();
				}
			}
			// Sent from a mind map topic: ask it in the conversation on the page.
			const asked = page.url.searchParams.get('ask');
			if (asked) {
				replaceState(here, page.state);
				talk.ask(asked, false);
				return;
			}
			writeGuideIfNeeded();
		} catch (e) {
			error = describe(e);
		}
	}

	/** Writes the overview when the page would show it and the sources changed since. */
	async function writeGuideIfNeeded() {
		if (!notebook || guideWriting || talk.turns.length || talk.live || guide?.fresh) return;
		if (!notebook.sources.some((s) => !s.excluded && s.chunks > 0)) return;
		guideWriting = true;
		guideError = null;
		try {
			guide = await api.writeGuide(slug);
		} catch (e) {
			guideError = describe(e);
		} finally {
			guideWriting = false;
		}
	}

	// Recordings are transcribed, scans read, and passages embedded in the background; check on them until done.
	const background = $derived(
		(notebook?.sources.some((s) => s.transcribing || s.reading) ?? false) ||
			(notebook?.embedding !== undefined && notebook.embedding.stage !== 'failed')
	);
	$effect(() => {
		if (!background) return;
		const timer = setInterval(() => api.notebook(slug).then(updateNotebook, () => {}), 2000);
		return () => clearInterval(timer);
	});

	/** Takes a changed notebook; a changed set of sources makes the overview out of date. */
	function updateNotebook(next: Notebook) {
		// A recording or scan counts once its text is written, as on the server.
		const key = (nb: Notebook | null) =>
			(nb?.sources ?? [])
				.filter((s) => !s.excluded && !s.transcribing && !s.reading)
				.map((s) => s.id)
				.sort()
				.join();
		const changed = key(next) !== key(notebook);
		notebook = next;
		if (changed && guide) {
			guide.fresh = false;
			guideError = null;
			writeGuideIfNeeded();
		}
	}

	/** Hands the reader a file the server wrote, under `name`. */
	async function download(file: Promise<Blob>, name: string) {
		try {
			downloadBlob(await file, name);
		} catch (e) {
			notices.failed(e);
		}
	}

	async function refreshNotes() {
		notes = await api.notes(slug);
	}

	async function newNote() {
		try {
			const note = await api.createNote(slug, '', '');
			await refreshNotes();
			sheet.open({ noteId: note.id });
		} catch (e) {
			notices.failed(e);
		}
	}

	async function saveStyle(style: ChatStyle): Promise<string | null> {
		try {
			notebook = await api.setStyle(slug, style);
			return null;
		} catch (e) {
			return describe(e);
		}
	}

	async function exclude(changed: Source[], excluded: boolean) {
		try {
			let next = notebook;
			for (const source of changed) next = await api.setExcluded(slug, source.id, excluded);
			if (next) updateNotebook(next);
		} catch (e) {
			notices.failed(e);
		}
	}

	onMount(load);

	const openSource = (source: Source) => sheet.open({ sourceId: source.id });

	async function renameSource(title: string): Promise<string | null> {
		if (!paneSourceId) return null;
		try {
			updateNotebook(await api.renameSource(slug, paneSourceId, title));
			return null;
		} catch (e) {
			return describe(e);
		}
	}

	async function removeSource(): Promise<string | null> {
		if (!paneSource) return null;
		const { id, title } = paneSource;
		try {
			updateNotebook(await api.removeSource(slug, id));
			sheet.close();
			notices.say(`Removed ${title}.`);
			return null;
		} catch (e) {
			return describe(e);
		}
	}

	async function rereadSource(): Promise<string | null> {
		if (!paneSource) return null;
		try {
			updateNotebook(await api.rereadSource(slug, paneSource.id));
			notices.say(`Re-reading ${paneSource.title} with the current OCR script.`);
			return null;
		} catch (e) {
			return describe(e);
		}
	}

	function shortcut(e: KeyboardEvent) {
		const target = e.target as HTMLElement;
		const typing = target.closest('input, textarea, [contenteditable]') !== null;
		if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k' && notebook && notebook.sources.length > 0) {
			e.preventDefault();
			sheet.open({ search: true });
			return;
		}
		if (e.key === '/' && !typing && !e.metaKey && !e.ctrlKey && composer) {
			e.preventDefault();
			composer.focus();
		}
	}
</script>

{#snippet noticeLine(note: Notice)}
	{@const thought = note.thought}
	<p class="notice" class:problem={note.problem} role={note.problem ? 'alert' : 'status'}>
		{note.text}
		{#if thought?.text.trim()}
			<button class="action" onclick={() => (thought.shown = !thought.shown)}>
				{thought.shown ? 'Hide thinking' : 'Show thinking'}
			</button>
		{/if}
		<button class="action" onclick={() => (notices.notice = null)}>Dismiss</button>
	</p>
	{#if thought?.shown && thought.text.trim()}
		<div class="studio-thinking" role="region" aria-label="Studio model thinking">{thought.text}</div>
	{/if}
{/snippet}

<svelte:head><title>{notebook ? `${notebook.title} · Bowerbird` : 'Bowerbird'}</title></svelte:head>


<svelte:window onkeydown={shortcut} />

{#if notebook && notebook.sources.length > 0}<a class="skip" href="#question">Skip to the question</a>{/if}

<div class="spread">
	<NotebookHeading
		{notebook}
		{error}
		{covered}
		onretry={load}
		oncontents={() => (contentsOpen = true)}
		onsearch={() => sheet.open({ search: true })}
		onstyle={() => notebook && styleDialog.show(notebook.chat)}
		onexportzip={() => download(api.exportNotebook(slug), `${slug}.zip`)}
		onexportbib={() => download(api.exportBibtex(slug), `${slug}.bib`)}
		onrenamed={(next) => (notebook = next)}
	/>

	<Contents
		sources={notebook?.sources ?? []}
		embedding={notebook?.embedding}
		active={paneItem}
		ready={notebook !== null}
		inert={modal}
		pending={additions.pending}
		fresh={additions.fresh}
		chats={talk.chats}
		chat={talk.chat}
		{notes}
		studies={studio.sets}
		writing={studio.writing}
		reports={reports.writing}
		bind:open={contentsOpen}
		onadd={() => addDialog.show()}
		onopen={openSource}
		onexclude={exclude}
		onchat={talk.open}
		onnewchat={talk.startNew}
		onnote={(id) => sheet.open({ noteId: id })}
		onnewnote={newNote}
		onnewreport={() => reportDialog.show()}
		onstudy={studio.open}
		onnewstudy={(kind) =>
			kind === 'mindmap' || kind === 'infographic'
				? studio.create(kind, 1, 'medium', '')
				: kind === 'table'
					? tableDialog.show()
					: studyDialog.show(kind)}
	/>

	<main class="text" inert={covered}>
		{#if notebook}
			{#if notices.notice && notebook.sources.length === 0}{@render noticeLine(notices.notice)}{/if}
			{#if notebook.sources.length === 0 && additions.pending.length === 0}
				<div class="start">
					<p>Add a PDF, a link, or pasted text to start. You can also drop files anywhere on this page.</p>
					<button class="primary" bind:this={startButton} onclick={() => addDialog.show()}>Add sources</button>
				</div>
			{:else if talk.turns.length === 0 && !talk.live}
				<Guide
					state={guide}
					writing={guideWriting}
					error={guideError}
					{sources}
					active={sheet.pane && 'citations' in sheet.pane ? sheet.pane.citations : null}
					onopen={sheet.openCitations}
					onask={(question) => talk.ask(question, false)}
				/>
			{/if}
			{#each talk.turns as turn, i (i)}
				<TurnView
					{slug}
					{turn}
					number={i + 1}
					fill={talk.askedHere && !talk.live && i === talk.turns.length - 1}
					{sources}
					active={sheet.pane && 'citations' in sheet.pane ? sheet.pane.citations : null}
					onopen={sheet.openCitations}
					onsave={() => talk.saveAnswer(turn)}
					onopennote={talk.savedNotes.has(turn.ts) ? () => sheet.open({ noteId: talk.savedNotes.get(turn.ts)! }) : undefined}
				/>
			{/each}
			{#if talk.turns.length && !talk.live}
				<p class="chat-tools">
					<button class="action quiet" onclick={talk.startNew}>New conversation</button>
					<button
						class="action quiet"
						onclick={() => talk.chat && download(api.exportChat(slug, talk.chat), `${slug}-${talk.chat}.md`)}
					>
						Export Markdown
					</button>
					<button class="action quiet" onclick={talk.remove}>Delete this conversation</button>
				</p>
			{/if}
			{#if talk.live}
				<TurnView
					{slug}
					turn={talk.live}
					number={talk.turns.length + 1}
					{sources}
					active={sheet.pane && 'citations' in sheet.pane ? sheet.pane.citations : null}
					live
					fill
					error={talk.liveError}
					stopped={talk.liveStopped}
					reading={talk.reading}
					thinking={talk.thinking}
					since={talk.liveSince}
					onopen={sheet.openCitations}
					onretry={talk.retry}
				/>
			{/if}
			{#if notebook.sources.length > 0}
				<div class="composer-dock">
					<!-- Beside the composer, where the eye is on every screen size. -->
					{#if notices.notice}{@render noticeLine(notices.notice)}{/if}
					<Composer
						bind:this={composer}
						sourceCount={included}
						busy={talk.busy}
						{profiles}
						bind:selectedProfile
						onask={talk.ask}
						onstop={talk.stop}
					/>
				</div>
			{/if}
		{/if}
	</main>

	{#if sheet.pane && 'search' in sheet.pane}
		<SearchPane
			{slug}
			{modal}
			onclose={sheet.close}
			onpassage={(hit, q) => sheet.openSub({ sourceId: hit.source_id, ord: hit.ord, highlight: q })}
			onnote={(hit) => sheet.open({ noteId: hit.id })}
			onchat={(hit) => {
				sheet.close();
				talk.open(hit.chat_id);
			}}
		/>
	{:else if sheet.pane && 'draft' in sheet.pane && reports.draft}
		<DraftPane
			title={reports.draft.title}
			pieces={reports.draft.pieces}
			citations={reports.draft.citations}
			thinking={reports.draft.thinking}
			since={reports.draft.since}
			error={reports.draft.error}
			{sources}
			{modal}
			onclose={sheet.close}
			onopen={sheet.openFromSheet}
		/>
	{:else if sheet.pane && 'noteId' in sheet.pane}
		{#key sheet.pane.noteId}
			<NotePane
				{slug}
				id={sheet.pane.noteId}
				{modal}
				onclose={sheet.close}
				onopen={sheet.openFromSheet}
				onchange={(note) => {
					const listed = notes.find((n) => n.id === note.id);
					if (listed) listed.title = note.title;
				}}
				ondelete={() => {
					sheet.close();
					refreshNotes();
				}}
				onsource={(next) => {
					updateNotebook(next);
					notices.say(next.already?.length ? `Already a source, as ${next.already[0]}.` : 'Added the note as a source.');
				}}
			/>
		{/key}
	{:else if sheet.pane && 'studyId' in sheet.pane}
		{#key sheet.pane.studyId}
			<StudyPane
				{slug}
				id={sheet.pane.studyId}
				{sources}
				{modal}
				onclose={sheet.close}
				onopen={sheet.openFromSheet}
				onask={(question) => {
					if (narrow.current) sheet.close();
					talk.ask(question, false);
				}}
				onchange={studio.refresh}
				ondelete={() => {
					sheet.close();
					studio.refresh();
				}}
			/>
		{/key}
	{:else if sheet.pane && paneSource}
		<SourcePane
			{slug}
			source={paneSource}
			entries={'entries' in sheet.pane ? sheet.pane.entries : null}
			initialOrd={'ord' in sheet.pane ? (sheet.pane.ord ?? 0) : 0}
			highlight={'highlight' in sheet.pane ? (sheet.pane.highlight ?? '') : ''}
			{modal}
			onclose={sheet.close}
			onback={sheet.back ? sheet.goBack : undefined}
			backLabel={sheet.back && 'search' in sheet.back
				? 'Back to search'
				: sheet.back && 'noteId' in sheet.back
					? 'Back to the note'
					: sheet.back && 'draft' in sheet.back
						? 'Back to the report'
						: 'Back to the study set'}
			onrename={renameSource}
			onremove={removeSource}
			onexclude={(excluded) => paneSource && exclude([paneSource], excluded)}
			onreread={rereadSource}
		/>
	{/if}
</div>

<p class="visually-hidden" role="status">{notices.announcement}</p>

<DropZone title={notebook?.title ?? ''} enabled={notebook !== null} onfiles={additions.files} />

<AddSources
	bind:this={addDialog}
	onfiles={additions.files}
	onurl={additions.url}
	ontext={additions.text}
	onfind={(question) => api.research(slug, question).catch((e) => describe(e))}
/>
<StudyDialog
	bind:this={studyDialog}
	oncreate={(kind, count, difficulty, topic, format) => studio.create(kind, count, difficulty, topic, '', format)}
/>
<StyleDialog bind:this={styleDialog} {profiles} {defaultModel} onsave={saveStyle} />
<ReportDialog bind:this={reportDialog} oncreate={reports.write} />
<TableDialog bind:this={tableDialog} oncreate={(collect, topic) => studio.create('table', 1, 'medium', topic, collect)} />

<style>
	/* A centered spread: sources | text | margin. The margin column stays
	   empty in the grid; quotes float into it from the text, and the passage
	   sheet covers it, so the text never moves. */
	.spread {
		--sources: 15rem;
		--sources-gap: 3.5rem;
		--spread: calc(var(--sources) + var(--sources-gap) + var(--measure) + var(--margin-gap) + var(--margin));
		/* From just past the text column's right edge to the window's edge. */
		--pane-width: clamp(20rem, calc(50vw - var(--spread) / 2 + var(--margin-gap) / 2 + var(--margin)), 36rem);
		display: grid;
		grid-template-columns: var(--sources) var(--sources-gap) var(--measure) calc(var(--margin-gap) + var(--margin));
		grid-template-rows: auto auto 1fr;
		grid-template-areas:
			'head head head head'
			'sources . title .'
			'sources . text .';
		width: var(--spread);
		min-height: 100dvh;
		margin-inline: auto;
	}
	.skip {
		position: fixed;
		top: 0.5rem;
		left: 0.5rem;
		z-index: 40;
		padding: calc(0.5rem - var(--cap-lift)) 0.85rem calc(0.5rem + var(--cap-lift));
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		transform: translateY(-200%);
	}
	.skip:focus {
		transform: none;
	}
	@media (min-width: 1240px) {
		.spread > :global(.contents) {
			grid-area: sources;
			position: sticky;
			top: 3.5rem;
			align-self: start;
			max-height: calc(100dvh - 4rem);
			padding: 3.75rem 0 2rem;
			overflow-y: auto;
		}
	}
	/* The contents' scrollbar sits out in the gap beside the column rather
	   than against the checkboxes, and shows while the pointer is over it.
	   Chromium repaints a scrollbar only when its element's own style
	   changes, so hovering sets a property rather than styling the thumb. */
	@media (min-width: 1240px) and (pointer: fine) {
		.spread > :global(.contents) {
			--scrollbar-thumb: transparent;
			margin-right: -2rem;
			padding-right: calc(2rem - var(--scrollbar));
			scrollbar-gutter: stable;
		}
		.spread > :global(.contents:hover),
		.spread > :global(.contents:focus-within) {
			--scrollbar-thumb: var(--edge);
		}
	}
	.text {
		grid-area: text;
		display: flex;
		flex-direction: column;
		padding-bottom: 0;
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
	.start p {
		margin: 0 0 1.25rem;
		color: var(--ink-2);
		font-size: var(--t-m);
		font-style: italic;
	}
	.primary {
		align-self: flex-start;
		padding: calc(0.5rem - var(--cap-lift)) 1rem calc(0.5rem + var(--cap-lift));
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
	.notice {
		margin: 0 0 0.75rem;
		font-size: var(--t-s);
		font-style: italic;
	}
	.start + .notice,
	.notice:first-child {
		margin-bottom: 2rem;
	}
	.notice .action,
	.problem .action {
		margin-left: 0.75rem;
	}
	.problem {
		color: var(--danger);
	}
	.chat-tools {
		display: flex;
		gap: 1.5rem;
		margin: 2.5rem 0 0;
		font-size: var(--t-xs);
	}
	.chat-tools .action {
		padding: 0.3rem 0;
	}
	/* Fingers need more than the text's height to hit these. */
	@media (pointer: coarse) {
		.chat-tools .action {
			padding: 0.65rem 0;
		}
	}
	.composer-dock {
		position: sticky;
		bottom: 0;
		margin-top: auto;
		padding: 2rem 0 1.25rem;
		background: linear-gradient(transparent, var(--page) 1.5rem);
	}


	.studio-thinking {
		max-height: 9rem;
		margin: 0.4rem 0 0.75rem;
		padding: 0.1rem 0 0.1rem 0.85rem;
		overflow-y: auto;
		border-left: 2px solid var(--line);
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
		line-height: 1.5;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}

	/* Without room for the contents column, the contents open as a drawer
	   from the running head. */
	@media (max-width: 1239px) {
		.spread {
			--spread: calc(var(--measure) + var(--margin-gap) + var(--margin));
			grid-template-columns: var(--measure) calc(var(--margin-gap) + var(--margin));
			grid-template-rows: auto auto 1fr;
			grid-template-areas:
				'head head'
				'title .'
				'text .';
		}
	}

	/* No margin: one column, and quotes follow each answer. */
	@media (max-width: 959px) {
		.spread {
			grid-template-columns: minmax(0, 1fr);
			grid-template-areas: 'head' 'title' 'text';
			width: min(100% - 3rem, var(--measure));
		}
	}

	@media (max-width: 719px) {
		.spread {
			--strip: calc(3.4rem + env(safe-area-inset-bottom));
			grid-template-rows: auto auto 1fr;
			grid-template-areas: 'head' 'title' 'text';
			width: calc(100% - 2rem);
			padding-bottom: var(--strip);
		}
		.composer-dock {
			bottom: var(--strip);
			padding-bottom: 0.75rem;
		}
		/* The phone strip sits under the composer, so less of the screen is free. */
		.text {
			--reserved: calc(15rem + var(--strip));
		}
	}
</style>
