<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import { goto, replaceState } from '$app/navigation';
	import { page } from '$app/state';
	import {
		api,
		ask,
		describe,
		markdownOf,
		UPLOAD_MB,
		writeReport as streamReport,
		type AudioFormat,
		type ChatStyle,
		type ChatSummary,
		type Citation,
		type Difficulty,
		type GuideState,
		type Notebook,
		type NoteSummary,
		type Piece,
		type ReportKind,
		type Reading,
		type Source,
		type StudyKind,
		type StudySummary,
		type Turn
	} from '#lib/api.ts';
	import AddSources from '#lib/components/AddSources.svelte';
	import Composer from '#lib/components/Composer.svelte';
	import Contents from '#lib/components/Contents.svelte';
	import DropZone from '#lib/components/DropZone.svelte';
	import Guide from '#lib/components/Guide.svelte';
	import DraftPane from '#lib/components/DraftPane.svelte';
	import NotebookHeading from '#lib/components/NotebookHeading.svelte';
	import NotePane from '#lib/components/NotePane.svelte';
	import ReportDialog from '#lib/components/ReportDialog.svelte';
	import SourcePane from '#lib/components/SourcePane.svelte';
	import StudyDialog from '#lib/components/StudyDialog.svelte';
	import StudyPane from '#lib/components/StudyPane.svelte';
	import StyleDialog from '#lib/components/StyleDialog.svelte';
	import TableDialog from '#lib/components/TableDialog.svelte';
	import TurnView from '#lib/components/Turn.svelte';
	import { SheetState } from '#lib/sheet.svelte.ts';

	const slug = $derived(page.params.slug ?? '');
	// Below this width there is no margin, so passages open in a sheet over the text.
	const narrow = new MediaQuery('max-width: 959px');
	// From this width the contents are a column; below it they open over the page.
	const wide = new MediaQuery('min-width: 1240px');

	let notebook = $state<Notebook | null>(null);
	let turns = $state<Turn[]>([]);
	let error = $state<string | null>(null);
	let notice = $state<{ text: string; problem: boolean } | null>(null);
	/** Read out by screen readers when something finishes. */
	let announcement = $state('');

	let live = $state<Turn | null>(null);
	let liveError = $state<string | null>(null);
	let liveStopped = $state(false);
	let reading = $state<Reading | null>(null);
	let liveSince = $state(0);
	let controller: AbortController | null = null;

	let pending = $state<string[]>([]);
	let fresh = $state(new Set<string>());

	let chats = $state<ChatSummary[]>([]);
	/** The conversation on the page; null until its first question is asked. */
	let chat = $state<string | null>(null);
	let notes = $state<NoteSummary[]>([]);
	let studies = $state<StudySummary[]>([]);
	let writing = $state<StudyKind[]>([]);
	let reports = $state<string[]>([]);
	/** A report being written, shown as it arrives. */
	let draft = $state<{
		title: string;
		pieces: Piece[];
		citations: Citation[];
		error: string | null;
	} | null>(null);
	let guide = $state<GuideState | null>(null);
	let guideWriting = $state(false);
	let guideError = $state<string | null>(null);

	const sheet = new SheetState(() => slug);

	let contentsOpen = $state(false);
	let addDialog: AddSources;
	let studyDialog: StudyDialog;
	let styleDialog: StyleDialog;
	let reportDialog: ReportDialog;
	let tableDialog: TableDialog;
	let composer = $state<Composer>();
	let startButton = $state<HTMLButtonElement>();

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
		return pane !== null && (paneSource !== undefined || 'noteId' in pane || 'studyId' in pane || 'draft' in pane);
	});
	const modal = $derived(narrow.current && showingPane);
	/** Something covers the page, so what's behind it can't be used. */
	const covered = $derived(modal || (contentsOpen && !wide.current));
	const busy = $derived(live !== null && liveError === null && !liveStopped);
	const included = $derived(notebook?.sources.filter((s) => !s.excluded).length ?? 0);

	async function load() {
		error = null;
		try {
			const [nb, chatList, noteList, studyList, guideState] = await Promise.all([
				api.notebook(slug),
				api.chats(slug),
				api.notes(slug),
				api.studio(slug),
				api.guide(slug)
			]);
			notebook = nb;
			chats = chatList;
			notes = noteList;
			studies = studyList;
			guide = guideState;
			const wanted = page.url.searchParams.get('c');
			chat = chatList.find((c) => c.id === wanted)?.id ?? chatList[0]?.id ?? null;
			turns = chat ? await api.turns(slug, chat) : [];
			await tick();
			if (turns.length) scroller().scrollTop = scroller().scrollHeight;
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
				replaceState(chat ? `${page.url.pathname}?c=${encodeURIComponent(chat)}` : page.url.pathname, page.state);
				askQuestion(asked, false);
				return;
			}
			writeGuideIfNeeded();
		} catch (e) {
			error = describe(e);
		}
	}

	/** Writes the overview when the page would show it and the sources changed since. */
	async function writeGuideIfNeeded() {
		if (!notebook || guideWriting || turns.length || live || guide?.fresh) return;
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

	async function openChat(id: string) {
		if (busy) stop();
		askedHere = false;
		try {
			turns = await api.turns(slug, id);
			chat = id;
			live = null;
			replaceState(`${page.url.pathname}?c=${encodeURIComponent(id)}`, page.state);
			await tick();
			scroller().scrollTop = 0;
		} catch (e) {
			notice = { text: describe(e), problem: true };
		}
	}

	async function newChat() {
		if (busy) stop();
		askedHere = false;
		chat = null;
		turns = [];
		live = null;
		replaceState(page.url.pathname, page.state);
		scroller().scrollTop = 0;
		composer?.focus();
		writeGuideIfNeeded();
	}

	async function deleteChat() {
		if (!chat) return;
		try {
			await api.deleteChat(slug, chat);
			chats = await api.chats(slug);
			await newChat();
		} catch (e) {
			notice = { text: describe(e), problem: true };
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
			notice = { text: describe(e), problem: true };
		}
	}

	/** Notes made from answers, by the answer's time, so the answer can open its note. */
	let savedNotes = $state(new Map<number, string>());
	/** A question was asked on this page, so the newest turn keeps a screen's height. */
	let askedHere = $state(false);

	async function saveAnswer(turn: Turn): Promise<string | null> {
		try {
			const note = await api.createNote(slug, turn.question, markdownOf(turn.pieces, turn.citations));
			savedNotes.set(turn.ts, note.id);
			savedNotes = new Map(savedNotes);
			await refreshNotes();
			announcement = 'Saved to notes.';
			return null;
		} catch (e) {
			return describe(e);
		}
	}

	async function createStudy(
		kind: StudyKind,
		count: number,
		difficulty: Difficulty,
		topic: string,
		collect = '',
		format: AudioFormat = 'deep-dive'
	) {
		writing.push(kind);
		const what = {
			flashcards: 'Writing flashcards',
			quiz: 'Writing a quiz',
			mindmap: 'Drawing a mind map',
			table: 'Building a table',
			slides: 'Making slides',
			infographic: 'Laying out an infographic',
			audio: 'Writing an audio overview'
		}[kind];
		notice = { text: `${what} from the sources. It opens when ready, usually within a minute.`, problem: false };
		try {
			const set = await api.createStudy(slug, kind, count, difficulty, topic, collect, format);
			studies = await api.studio(slug);
			announcement = {
				flashcards: 'Flashcards ready.',
				quiz: 'Quiz ready.',
				mindmap: 'Mind map ready.',
				table: 'Table ready.',
				slides: 'Slides ready.',
				infographic: 'Infographic ready.',
				audio: 'Audio overview script ready; recording it now.'
			}[kind];
			if (notice?.text.startsWith(what)) notice = null;
			if (PAGES[kind]) goto(`/n/${encodeURIComponent(slug)}/${PAGES[kind]}/${set.id}`);
			else if (!sheet.pane) sheet.open({ studyId: set.id });
		} catch (e) {
			notice = { text: describe(e), problem: true };
		} finally {
			writing.splice(writing.indexOf(kind), 1);
		}
	}

	/** Study sets that open on a page of their own rather than in the sheet. */
	const PAGES: Partial<Record<StudyKind, string>> = { mindmap: 'map', slides: 'slides', infographic: 'infographic' };

	const REPORT_NAMES: Record<ReportKind, string> = {
		'study-guide': 'study guide',
		briefing: 'briefing',
		timeline: 'timeline',
		faq: 'FAQ',
		custom: 'report'
	};

	async function writeReport(kind: ReportKind, topic: string, instructions: string) {
		const name = REPORT_NAMES[kind];
		reports.push(name);
		const unlist = () => reports.includes(name) && reports.splice(reports.indexOf(name), 1);
		draft = { title: `Writing a ${name}…`, pieces: [], citations: [], error: null };
		const current = draft;
		if (!sheet.pane) sheet.open({ draft: true });
		const fail = (message: string) => {
			current.error = message;
			if (!(sheet.pane && 'draft' in sheet.pane)) notice = { text: message, problem: true };
		};
		try {
			await streamReport(slug, kind, topic, instructions, {
				note: (note) => {
					current.title = note.title;
				},
				text: (text) => {
					const last = current.pieces.at(-1);
					if (last && 'text' in last) last.text += text;
					else current.pieces.push({ text });
				},
				cite: (citation) => {
					current.citations.push(citation);
					current.pieces.push({ cite: current.citations.length - 1 });
				},
				done: (note) => {
					announcement = 'Report ready.';
					unlist();
					refreshNotes();
					// The draft becomes the note, in the same sheet and history entry.
					if (sheet.pane && 'draft' in sheet.pane) sheet.pane = { noteId: note.id };
					if (draft === current) draft = null;
				},
				error: fail
			});
		} catch (e) {
			fail(describe(e));
		} finally {
			unlist();
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
			notice = { text: describe(e), problem: true };
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
			notice = { text: `Removed ${title}.`, problem: false };
			return null;
		} catch (e) {
			return describe(e);
		}
	}

	const scroller = () => document.scrollingElement ?? document.documentElement;

	/** Brings a new question to the top of the screen, so its answer is read
	 * from its first line as it arrives, not chased from the bottom. */
	async function revealQuestion() {
		await tick();
		document.querySelector('main .turn:last-of-type .question')?.scrollIntoView({ block: 'start' });
	}

	async function askQuestion(question: string, deep: boolean) {
		controller?.abort();
		controller = new AbortController();
		const signal = controller.signal;
		live = {
			ts: 0,
			question,
			model: '',
			pieces: [],
			citations: [],
			timings: { request_ms: 0, first_token_ms: null, total_ms: 0 },
			deep
		};
		liveError = null;
		liveStopped = false;
		reading = null;
		liveSince = Date.now();
		announcement = '';
		askedHere = true;
		revealQuestion();
		const turn = live;
		const fail = (message: string) => {
			liveError = message;
			composer?.restore(question);
		};
		try {
			await ask(
				slug,
				chat,
				question,
				deep,
				{
					passages: (r) => {
						reading = r;
						chat = r.chat;
					},
					text: (text) => {
						const last = turn.pieces.at(-1);
						if (last && 'text' in last) last.text += text;
						else turn.pieces.push({ text });
					},
					cite: (citation) => {
						turn.citations.push(citation);
						turn.pieces.push({ cite: turn.citations.length - 1 });
					},
					done: (saved) => {
						turns.push(saved);
						live = null;
						if (page.url.searchParams.get('c') !== chat) {
							replaceState(`${page.url.pathname}?c=${encodeURIComponent(chat ?? '')}`, page.state);
						}
						api.chats(slug).then((list) => (chats = list));
						const missing = saved.citations.filter((c) => !c.verified).length;
						announcement = `Answer ready, with ${saved.citations.length} citations${missing ? `, ${missing} not found in the sources` : ''}.`;
					},
					error: fail
				},
				signal
			);
			if (live === turn && !liveError && !liveStopped) fail('The answer broke off before it finished.');
		} catch (e) {
			if (!signal.aborted) fail(describe(e));
		}
	}

	function retry() {
		if (!live) return;
		composer?.forget(live.question);
		askQuestion(live.question, live.deep);
	}

	function stop() {
		controller?.abort();
		liveStopped = true;
	}

	const lettersList = (letters: string[]) =>
		letters.length < 3 ? letters.join(' and ') : `${letters.slice(0, -1).join(', ')} and ${letters.at(-1)}`;

	/**
	 * Runs an add, whose work resolves to the updated notebook, or null when
	 * nothing was added. Resolves to an error message, or null when it worked.
	 */
	async function adding(names: string[], work: () => Promise<Notebook | null>): Promise<string | null> {
		pending.push(...names);
		notice = null;
		const before = new Set(notebook?.sources.map((s) => s.id));
		try {
			const updated = await work();
			if (!updated) return null;
			fresh = new Set(updated.sources.filter((s) => !before.has(s.id)).map((s) => s.id));
			updateNotebook(updated);
			if (updated.already?.length) {
				notice = { text: `Already in this notebook as ${lettersList(updated.already)}.`, problem: false };
			} else if (fresh.size) {
				announcement = fresh.size === 1 ? 'Source added.' : `${fresh.size} sources added.`;
			}
			return null;
		} catch (e) {
			return describe(e);
		} finally {
			for (const name of names) pending.splice(pending.indexOf(name), 1);
		}
	}

	async function addFiles(files: File[]) {
		const problems = new Set<string>();
		await adding(
			files.map((f) => f.name),
			async () => {
				// One request per file: the server's limit covers a whole request,
				// and a file it refuses leaves the others to be added.
				let updated: Notebook | null = null;
				const already: string[] = [];
				for (const file of files) {
					if (file.size > UPLOAD_MB * 2 ** 20) {
						problems.add(`Can't add ${file.name}. A file can be up to ${UPLOAD_MB} MB.`);
						continue;
					}
					try {
						updated = await api.addFiles(slug, [file]);
						already.push(...(updated.already ?? []));
					} catch (e) {
						problems.add(describe(e));
					}
				}
				return updated && { ...updated, already };
			}
		);
		if (problems.size) notice = { text: [...problems].join(' '), problem: true };
	}
	const addUrl = (url: string) => adding([url], () => api.addUrl(slug, url));
	const addText = (title: string, text: string) => adding([title || 'Pasted text'], () => api.addText(slug, title, text));

	function shortcut(e: KeyboardEvent) {
		const target = e.target as HTMLElement;
		const typing = target.closest('input, textarea, [contenteditable]') !== null;
		if (e.key === '/' && !typing && !e.metaKey && !e.ctrlKey && composer) {
			e.preventDefault();
			composer.focus();
		}
	}
</script>

{#snippet noticeLine(note: { text: string; problem: boolean })}
	<p class="notice" class:problem={note.problem} role={note.problem ? 'alert' : 'status'}>
		{note.text}
		<button class="action" onclick={() => (notice = null)}>Dismiss</button>
	</p>
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
		onstyle={() => notebook && styleDialog.show(notebook.chat)}
		onrenamed={(next) => (notebook = next)}
	/>

	<Contents
		sources={notebook?.sources ?? []}
		embedding={notebook?.embedding}
		active={paneItem}
		ready={notebook !== null}
		inert={modal}
		{pending}
		{fresh}
		{chats}
		{chat}
		{notes}
		{studies}
		{writing}
		{reports}
		bind:open={contentsOpen}
		onadd={() => addDialog.show()}
		onopen={openSource}
		onexclude={exclude}
		onchat={openChat}
		onnewchat={newChat}
		onnote={(id) => sheet.open({ noteId: id })}
		onnewnote={newNote}
		onnewreport={() => reportDialog.show()}
		onstudy={(set) =>
			PAGES[set.kind] ? goto(`/n/${encodeURIComponent(slug)}/${PAGES[set.kind]}/${set.id}`) : sheet.open({ studyId: set.id })}
		onnewstudy={(kind) =>
			kind === 'mindmap' || kind === 'infographic'
				? createStudy(kind, 1, 'medium', '')
				: kind === 'table'
					? tableDialog.show()
					: studyDialog.show(kind)}
	/>

	<main class="text" inert={covered}>
		{#if notebook}
			{#if notice && notebook.sources.length === 0}{@render noticeLine(notice)}{/if}
			{#if notebook.sources.length === 0 && pending.length === 0}
				<div class="start">
					<p>Add a PDF, a link, or pasted text to start. You can also drop files anywhere on this page.</p>
					<button class="primary" bind:this={startButton} onclick={() => addDialog.show()}>Add sources</button>
				</div>
			{:else if turns.length === 0 && !live}
				<Guide
					state={guide}
					writing={guideWriting}
					error={guideError}
					{sources}
					active={sheet.pane && 'citations' in sheet.pane ? sheet.pane.citations : null}
					onopen={sheet.openCitations}
					onask={(question) => askQuestion(question, false)}
				/>
			{/if}
			{#each turns as turn, i (i)}
				<TurnView
					{slug}
					{turn}
					number={i + 1}
					fill={askedHere && !live && i === turns.length - 1}
					{sources}
					active={sheet.pane && 'citations' in sheet.pane ? sheet.pane.citations : null}
					onopen={sheet.openCitations}
					onsave={() => saveAnswer(turn)}
					onopennote={savedNotes.has(turn.ts) ? () => sheet.open({ noteId: savedNotes.get(turn.ts)! }) : undefined}
				/>
			{/each}
			{#if turns.length && !live}
				<p class="chat-tools">
					<button class="action quiet" onclick={newChat}>New conversation</button>
					<button class="action quiet" onclick={deleteChat}>Delete this conversation</button>
				</p>
			{/if}
			{#if live}
				<TurnView
					{slug}
					turn={live}
					number={turns.length + 1}
					{sources}
					active={sheet.pane && 'citations' in sheet.pane ? sheet.pane.citations : null}
					live
					fill
					error={liveError}
					stopped={liveStopped}
					{reading}
					since={liveSince}
					onopen={sheet.openCitations}
					onretry={retry}
				/>
			{/if}
			{#if notebook.sources.length > 0}
				<div class="composer-dock">
					<!-- Beside the composer, where the eye is on every screen size. -->
					{#if notice}{@render noticeLine(notice)}{/if}
					<Composer
						bind:this={composer}
						sourceCount={included}
						{busy}
						onask={askQuestion}
						onstop={stop}
					/>
				</div>
			{/if}
		{/if}
	</main>

	{#if sheet.pane && 'draft' in sheet.pane && draft}
		<DraftPane
			title={draft.title}
			pieces={draft.pieces}
			citations={draft.citations}
			error={draft.error}
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
					notice = {
						text: next.already?.length ? `Already a source, as ${next.already[0]}.` : 'Added the note as a source.',
						problem: false
					};
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
					askQuestion(question, false);
				}}
				onchange={() => api.studio(slug).then((list) => (studies = list))}
				ondelete={() => {
					sheet.close();
					api.studio(slug).then((list) => (studies = list));
				}}
			/>
		{/key}
	{:else if sheet.pane && paneSource}
		<SourcePane
			{slug}
			source={paneSource}
			entries={'entries' in sheet.pane ? sheet.pane.entries : null}
			{modal}
			onclose={sheet.close}
			onback={sheet.back ? sheet.goBack : undefined}
			backLabel={sheet.back && 'noteId' in sheet.back
				? 'Back to the note'
				: sheet.back && 'draft' in sheet.back
					? 'Back to the report'
					: 'Back to the study set'}
			onrename={renameSource}
			onremove={removeSource}
			onexclude={(excluded) => paneSource && exclude([paneSource], excluded)}
		/>
	{/if}
</div>

<p class="visually-hidden" role="status">{announcement}</p>

<DropZone title={notebook?.title ?? ''} enabled={notebook !== null} onfiles={addFiles} />

<AddSources
	bind:this={addDialog}
	onfiles={addFiles}
	onurl={addUrl}
	ontext={addText}
	onfind={(question) => api.research(slug, question).catch((e) => describe(e))}
/>
<StudyDialog
	bind:this={studyDialog}
	oncreate={(kind, count, difficulty, topic, format) => createStudy(kind, count, difficulty, topic, '', format)}
/>
<StyleDialog bind:this={styleDialog} onsave={saveStyle} />
<ReportDialog bind:this={reportDialog} oncreate={writeReport} />
<TableDialog bind:this={tableDialog} oncreate={(collect, topic) => createStudy('table', 1, 'medium', topic, collect)} />

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
