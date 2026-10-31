// Client for bowerbird-server. Types mirror the server's JSON.

export type Spine = { letter: string; color: number };

export type NotebookSummary = {
	slug: string;
	title: string;
	sources: Spine[];
	/** Unix seconds. */
	updated: number;
};

export type SourceKind = 'pdf' | 'markdown' | 'text' | 'web' | 'docx' | 'epub' | 'youtube' | 'audio';

export type Source = {
	id: string;
	title: string;
	kind: SourceKind;
	letter: string;
	color: number;
	url: string | null;
	pages: number | null;
	chunks: number;
	error: string | null;
	/** Left out of answers and study material. */
	excluded: boolean;
	/** A recording's transcription, until its transcript is ready. */
	transcribing?: Progress;
	/** A scanned PDF's OCR, until its text is ready. */
	reading?: Progress;
};

export type Goal = 'default' | 'tutor' | 'custom';
export type Length = 'shorter' | 'default' | 'longer';
/** How answers in a notebook are written. */
export type ChatStyle = { goal: Goal; custom?: string; length: Length };

export type Notebook = {
	slug: string;
	title: string;
	chat: ChatStyle;
	sources: Source[];
	/** Letters of sources an add request found already in the notebook. */
	already?: string[];
	/** Embedding of new passages for search by meaning, until every passage has a vector. */
	embedding?: Progress;
};

export type Citation = {
	/** Source letter + passage number, e.g. "B12". */
	label: string;
	letter: string;
	source_id: string | null;
	page: number | null;
	quote: string;
	verified: boolean;
};

export type Piece = { text: string } | { cite: number };

export type Turn = {
	ts: number;
	question: string;
	model: string;
	pieces: Piece[];
	citations: Citation[];
	timings: { request_ms: number; first_token_ms: number | null; total_ms: number };
	/** Answered with Deep search. */
	deep: boolean;
	/** Pictures in the passages the answer cites; none while it streams. */
	figures?: Figure[];
};

/** A picture from a web source. */
export type Figure = {
	file: string;
	caption: string;
	width: number;
	height: number;
	/** In a passage: where it stands in the text, in UTF-16 units. */
	at?: number;
	/** Under an answer: the citation whose passage holds it. */
	citation?: number;
};

/** Which passages an answer is being written from, and its conversation. */
export type Reading = { chat: string; count: number; sources: string[] };

export type ChatSummary = { id: string; title: string; turns: number; updated: number };

/** Overview of the sources and questions to start with. */
export type Guide = { model: string; pieces: Piece[]; citations: Citation[]; questions: string[] };
export type GuideState = { guide: Guide | null; fresh: boolean };

export type NoteSummary = { id: string; title: string; updated: number };
export type Note = NoteSummary & {
	/** Markdown with citations as [[A3|quote]] markers. */
	body: string;
	/** The body's citations, checked against the sources again. */
	citations: Citation[];
};

export type StudyKind = 'flashcards' | 'quiz' | 'mindmap' | 'table' | 'slides' | 'infographic' | 'audio';
export type AudioFormat = 'deep-dive' | 'brief' | 'critique' | 'debate';
/** One turn of an audio overview; start is 0 until it is recorded. */
export type Line = { host: 0 | 1; text: string; citations?: Citation[]; start: number };
export type AudioOverview = { format: AudioFormat; lines: Line[]; seconds: number; problem?: string };
/** How far a recording, a transcription, OCR, or embedding got; seconds while listening. */
export type Progress =
	| { stage: 'waiting' }
	| { stage: 'downloading'; done: number; total: number }
	| { stage: 'speaking'; done: number; total: number }
	| { stage: 'listening'; done: number; total: number | null }
	| { stage: 'embedding'; done: number; total: number }
	| { stage: 'reading'; done: number; total: number }
	| { stage: 'failed'; problem: string };
export type Slide = { title: string; points: string[]; notes?: string; citations: Citation[] };
/** A figure (head = value), an event (head = date), or a takeaway (head = title). */
export type Fact = { head: string; text: string; citation: Citation | null };
export type Infographic = { subtitle: string; figures: Fact[]; timeline: Fact[]; points: Fact[] };
export type Table = { columns: string[]; rows: { cells: string[]; citation: Citation | null }[] };
export type MapNode = { title: string; children?: MapNode[] };
export type ReportKind = 'study-guide' | 'briefing' | 'timeline' | 'faq' | 'custom';
export type Difficulty = 'easy' | 'medium' | 'hard';
export type StudySummary = {
	id: string;
	kind: StudyKind;
	title: string;
	count: number;
	due: number;
	best: number | null;
	/** Audio overviews: length of the recording, 0 while it is recorded. */
	seconds?: number;
	updated: number;
};
export type Card = { front: string; back: string; citation: Citation | null; level: number; due: number };
export type Question = {
	question: string;
	choices: string[];
	answer: number;
	hint: string;
	explanation: string;
	citation: Citation | null;
};
export type Attempt = { ts: number; answers: (number | null)[]; score: number };
export type StudySet = {
	id: string;
	kind: StudyKind;
	title: string;
	created: number;
	model: string;
	difficulty: Difficulty;
	topic?: string;
	cards?: Card[];
	questions?: Question[];
	attempts?: Attempt[];
	map?: MapNode;
	table?: Table;
	slides?: Slide[];
	infographic?: Infographic;
	audio?: AudioOverview;
	recording?: Progress;
	due: number;
};

export type Heading = { title: string; depth: number; ord: number };

export type Provider = { name: string; key_env: string; has_key: boolean; saved: boolean };

export type SearchBackend = 'duckduckgo' | 'searxng' | 'brave' | 'tavily' | 'exa';
export type SearchSettings = { backend: SearchBackend; searxng_url?: string };

export type Settings = {
	chat_model: string;
	default_model: string;
	providers: Provider[];
	search: SearchSettings;
	/** Search services that take a key. */
	search_keys: Provider[];
};

/** A web page found for a research question. */
export type Found = { title: string; url: string; snippet: string; already: string | null };
export type Research = { queries: string[]; results: Found[] };

export type Passage = {
	section: string;
	page_start: number | null;
	page_end: number | null;
	text: string;
	before: string | null;
	after: string | null;
	figures: Figure[];
};

export class ApiError extends Error {
	constructor(
		readonly status: number,
		message: string
	) {
		super(message);
	}
}

const TOKEN_KEY = 'bowerbird-token';
// Fallback for when sessionStorage is blocked: the token lasts for this page load.
let pageToken = '';

function remember(value: string) {
	pageToken = value;
	try {
		sessionStorage.setItem(TOKEN_KEY, value);
	} catch {
		// Storage unavailable; pageToken still works until reload.
	}
}

/** The server prints a link ending in `#token=…`. Keep the token for this tab and return the cleaned URL. */
export function takeTokenFromUrl(url: URL): string | null {
	const match = url.hash.match(/token=([^&]+)/);
	if (!match) return null;
	remember(decodeURIComponent(match[1]));
	return url.pathname + url.search;
}

// sessionStorage is per tab, so a notebook opened in a new tab starts without
// the token. Open tabs of this origin hand it over on request.
const tabs = typeof BroadcastChannel === 'undefined' ? null : new BroadcastChannel('bowerbird-token');
tabs?.addEventListener('message', (e: MessageEvent) => {
	if (e.data === 'ask' && token()) tabs.postMessage({ token: token() });
	else if (typeof e.data?.token === 'string' && !token()) remember(e.data.token);
});

/** Waits briefly for another tab to share the token. */
function borrowToken(): Promise<void> {
	if (!tabs) return Promise.resolve();
	return new Promise((resolve) => {
		const finish = () => {
			clearTimeout(timer);
			tabs.removeEventListener('message', finish);
			resolve();
		};
		const timer = setTimeout(finish, 300);
		tabs.addEventListener('message', finish);
		tabs.postMessage('ask');
	});
}

async function authorization(): Promise<string> {
	if (!token()) await borrowToken();
	return `Bearer ${token()}`;
}

function token(): string {
	if (pageToken) return pageToken;
	try {
		return sessionStorage.getItem(TOKEN_KEY) ?? '';
	} catch {
		return '';
	}
}

async function failure(res: Response): Promise<ApiError> {
	let message = `The server answered ${res.status}.`;
	try {
		message = (await res.json()).error ?? message;
	} catch {
		// Body wasn't JSON; keep the status message.
	}
	return new ApiError(res.status, message);
}

async function fetchApi(path: string, init: RequestInit = {}): Promise<Response> {
	const headers = new Headers(init.headers);
	headers.set('Authorization', await authorization());
	const res = await fetch(`/api${path}`, { ...init, headers });
	if (!res.ok) throw await failure(res);
	return res;
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
	const res = await fetchApi(path, init);
	if (res.status === 204) return undefined as T;
	return res.json() as Promise<T>;
}

const send = <T>(method: string, path: string, body: unknown) =>
	request<T>(path, { method, headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
const post = <T>(path: string, body: unknown) => send<T>('POST', path, body);

/** Largest file one upload takes, in MiB: UPLOAD_LIMIT in crates/server/src/lib.rs. */
export const UPLOAD_MB = 200;

const nb = (slug: string) => `/notebooks/${encodeURIComponent(slug)}`;
const source = (slug: string, id: string) => `${nb(slug)}/sources/${encodeURIComponent(id)}`;

export const api = {
	notebooks: () => request<NotebookSummary[]>('/notebooks'),
	notebook: (slug: string) => request<Notebook>(nb(slug)),
	chats: (slug: string) => request<ChatSummary[]>(`${nb(slug)}/chats`),
	turns: (slug: string, chat: string) => request<Turn[]>(`${nb(slug)}/chats/${encodeURIComponent(chat)}`),
	deleteChat: (slug: string, chat: string) =>
		request<void>(`${nb(slug)}/chats/${encodeURIComponent(chat)}`, { method: 'DELETE' }),
	guide: (slug: string) => request<GuideState>(`${nb(slug)}/guide`),
	writeGuide: (slug: string) => post<GuideState>(`${nb(slug)}/guide`, {}),
	notes: (slug: string) => request<NoteSummary[]>(`${nb(slug)}/notes`),
	note: (slug: string, id: string) => request<Note>(`${nb(slug)}/notes/${encodeURIComponent(id)}`),
	createNote: (slug: string, title: string, body: string) => post<Note>(`${nb(slug)}/notes`, { title, body }),
	saveNote: (slug: string, id: string, title: string, body: string) =>
		send<Note>('PUT', `${nb(slug)}/notes/${encodeURIComponent(id)}`, { title, body }),
	deleteNote: (slug: string, id: string) =>
		request<void>(`${nb(slug)}/notes/${encodeURIComponent(id)}`, { method: 'DELETE' }),
	noteToSource: (slug: string, id: string) => post<Notebook>(`${nb(slug)}/notes/${encodeURIComponent(id)}/source`, {}),
	studio: (slug: string) => request<StudySummary[]>(`${nb(slug)}/studio`),
	study: (slug: string, id: string) => request<StudySet>(`${nb(slug)}/studio/${encodeURIComponent(id)}`),
	createStudy: (
		slug: string,
		kind: StudyKind,
		count: number,
		difficulty: Difficulty,
		topic: string,
		describe = '',
		format: AudioFormat = 'deep-dive'
	) => post<StudySet>(`${nb(slug)}/studio`, { kind, count, difficulty, topic, describe, format }),
	studyAudio: async (slug: string, id: string) =>
		(await fetchApi(`${nb(slug)}/studio/${encodeURIComponent(id)}/audio`)).blob(),
	deleteStudy: (slug: string, id: string) =>
		request<void>(`${nb(slug)}/studio/${encodeURIComponent(id)}`, { method: 'DELETE' }),
	review: (slug: string, id: string, card: number, knew: boolean) =>
		send<StudySet>('PUT', `${nb(slug)}/studio/${encodeURIComponent(id)}/review`, { card, knew }),
	recordAttempt: (slug: string, id: string, answers: (number | null)[]) =>
		post<StudySet>(`${nb(slug)}/studio/${encodeURIComponent(id)}/attempts`, { answers }),
	slidesPptx: async (slug: string, id: string) =>
		(await fetchApi(`${nb(slug)}/studio/${encodeURIComponent(id)}/pptx`)).blob(),
	passage: (slug: string, sourceId: string, ord: number) =>
		request<Passage>(`${source(slug, sourceId)}/passages/${ord}`),
	outline: (slug: string, sourceId: string) => request<Heading[]>(`${source(slug, sourceId)}/outline`),
	/** The source as stored: the PDF itself for PDF sources. */
	sourceImage: async (slug: string, sourceId: string, file: string) =>
		(await fetchApi(`${source(slug, sourceId)}/images/${encodeURIComponent(file)}`)).blob(),
	sourceFile: async (slug: string, sourceId: string) =>
		(await fetchApi(`${source(slug, sourceId)}/file`)).arrayBuffer(),
	createNotebook: (title: string) => post<Notebook>('/notebooks', { title }),
	renameNotebook: (slug: string, title: string) => send<Notebook>('PATCH', nb(slug), { title }),
	setStyle: (slug: string, chat: ChatStyle) => send<Notebook>('PATCH', nb(slug), { chat }),
	setExcluded: (slug: string, id: string, excluded: boolean) =>
		send<Notebook>('PATCH', source(slug, id), { excluded }),
	deleteNotebook: (slug: string) => request<void>(nb(slug), { method: 'DELETE' }),
	renameSource: (slug: string, id: string, title: string) => send<Notebook>('PATCH', source(slug, id), { title }),
	removeSource: (slug: string, id: string) => request<Notebook>(source(slug, id), { method: 'DELETE' }),
	settings: () => request<Settings>('/settings'),
	saveSettings: (update: { chat_model?: string; keys?: Record<string, string>; search?: SearchSettings }) =>
		send<Settings>('PUT', '/settings', update),
	addFiles: (slug: string, files: File[]) => {
		const form = new FormData();
		for (const file of files) form.append('file', file, file.name);
		return request<Notebook>(`${nb(slug)}/sources/file`, { method: 'POST', body: form });
	},
	addUrl: (slug: string, url: string) => post<Notebook>(`${nb(slug)}/sources/url`, { url }),
	research: (slug: string, question: string) => post<Research>(`${nb(slug)}/research`, { question }),
	addText: (slug: string, title: string, text: string) => post<Notebook>(`${nb(slug)}/sources/text`, { title, text })
};

export type AskHandlers = {
	passages: (reading: Reading) => void;
	text: (text: string) => void;
	cite: (citation: Citation) => void;
	done: (turn: Turn) => void;
	error: (message: string) => void;
};

/** POSTs and reads the server-sent events of the reply, calling `on` with
 * each event's name and JSON payload. EventSource can't POST or send headers,
 * so this reads the stream itself. */
async function stream(path: string, body: unknown, on: (event: string, payload: any) => void, signal?: AbortSignal) {
	const res = await fetch(`/api${path}`, {
		method: 'POST',
		headers: { Authorization: await authorization(), 'Content-Type': 'application/json' },
		body: JSON.stringify(body),
		signal
	});
	if (!res.ok || !res.body) throw await failure(res);
	const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
	let buffer = '';
	for (;;) {
		const { value, done } = await reader.read();
		if (done) return;
		buffer += value;
		let end;
		while ((end = buffer.indexOf('\n\n')) >= 0) {
			const block = buffer.slice(0, end);
			buffer = buffer.slice(end + 2);
			let event = 'message';
			let data = '';
			for (const line of block.split('\n')) {
				if (line.startsWith('event:')) event = line.slice(6).trim();
				else if (line.startsWith('data:')) data += line.slice(5).trimStart();
			}
			// Keep-alive comments carry no data.
			if (data) on(event, JSON.parse(data));
		}
	}
}

/** Asks a question and streams the answer. Aborting `signal` closes the
 * stream, which stops the answer on the server. */
export function ask(
	slug: string,
	chat: string | null,
	question: string,
	deep: boolean,
	on: AskHandlers,
	signal?: AbortSignal
): Promise<void> {
	return stream(
		`${nb(slug)}/ask`,
		{ question, deep, chat },
		(event, payload) => {
			if (event === 'passages') on.passages(payload);
			else if (event === 'text') on.text(payload);
			else if (event === 'cite') on.cite(payload);
			else if (event === 'done') on.done(payload);
			else if (event === 'error') on.error(payload.error);
		},
		signal
	);
}

export type ReportHandlers = {
	note: (note: { title: string }) => void;
	text: (text: string) => void;
	cite: (citation: Citation) => void;
	done: (note: Note) => void;
	error: (message: string) => void;
};

/** Writes a report into a new note, streaming it as it is written. The
 * server finishes and saves it even if this page stops listening. */
export function writeReport(
	slug: string,
	kind: ReportKind,
	topic: string,
	instructions: string,
	on: ReportHandlers
): Promise<void> {
	return stream(`${nb(slug)}/reports`, { kind, topic, instructions }, (event, payload) => {
		if (event === 'note') on.note(payload);
		else if (event === 'text') on.text(payload);
		else if (event === 'cite') on.cite(payload);
		else if (event === 'done') on.done(payload);
		else if (event === 'error') on.error(payload.error);
	});
}

/** A sentence for the page when a request fails. */
export function describe(error: unknown): string {
	if (error instanceof ApiError && error.status === 401) {
		return 'This page needs the access token. Open Bowerbird from the link bowerbird-server printed when it started.';
	}
	if (error instanceof ApiError) return error.message;
	return 'Bowerbird’s server isn’t answering. Check that bowerbird-server is running.';
}

/** Passage number from a citation label: "B12" -> 12. */
export function passageOrd(c: Citation): number {
	return Number(c.label.slice(c.letter.length));
}

/** Answer text with its citations written back as [[A3|quote]] markers, for saving as a note. */
export function markdownOf(pieces: Piece[], citations: Citation[]): string {
	return pieces.map((p) => ('text' in p ? p.text : `[[${citations[p.cite].label}|${citations[p.cite].quote}]]`)).join('');
}
