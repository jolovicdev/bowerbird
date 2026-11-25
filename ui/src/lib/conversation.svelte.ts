// The conversation on a notebook page: its saved turns, the answer being
// written, and the notebook's other conversations.

import { tick } from 'svelte';
import { replaceState } from '$app/navigation';
import { page } from '$app/state';
import {
	api,
	ask,
	describe,
	markdownOf,
	type ChatSummary,
	type Reading,
	type Thinking,
	type Turn
} from '#lib/api.ts';
import type { Notices } from '#lib/notices.svelte.ts';

/** The question box, as far as the conversation uses it. */
type QuestionBox = { restore: (question: string) => void; forget: (question: string) => void; focus: () => void };

export const scroller = () => document.scrollingElement ?? document.documentElement;

/** Brings a new question to the top of the screen, so its answer is read
 * from its first line as it arrives, not chased from the bottom. */
async function revealQuestion() {
	await tick();
	document.querySelector('main .turn:last-of-type .question')?.scrollIntoView({ block: 'start' });
}

export class Conversation {
	turns = $state<Turn[]>([]);
	chats = $state<ChatSummary[]>([]);
	/** The conversation on the page; null until its first question is asked. */
	chat = $state<string | null>(null);

	live = $state<Turn | null>(null);
	liveError = $state<string | null>(null);
	liveStopped = $state(false);
	reading = $state<Reading | null>(null);
	thinking = $state('');
	liveSince = $state(0);
	readonly busy = $derived(this.live !== null && this.liveError === null && !this.liveStopped);

	/** Notes made from answers, by the answer's time, so the answer can open its note. */
	savedNotes = $state(new Map<number, string>());
	/** A question was asked on this page, so the newest turn keeps a screen's height. */
	askedHere = $state(false);

	#controller: AbortController | null = null;
	#slug: () => string;
	#notices: Notices;
	#box: () => QuestionBox | undefined;
	#choice: () => { model?: string; thinking?: Thinking } | undefined;
	#onempty: () => void;
	#onnote: () => Promise<void>;

	constructor(options: {
		slug: () => string;
		notices: Notices;
		/** The question box, once it is on the page. */
		box: () => QuestionBox | undefined;
		/** The model and thinking level picked for the next question, if any. */
		choice: () => { model?: string; thinking?: Thinking } | undefined;
		/** The page went back to a conversation with no turns. */
		onempty: () => void;
		/** An answer was saved as a note. */
		onnote: () => Promise<void>;
	}) {
		this.#slug = options.slug;
		this.#notices = options.notices;
		this.#box = options.box;
		this.#choice = options.choice;
		this.#onempty = options.onempty;
		this.#onnote = options.onnote;
	}

	ask = async (question: string, deep: boolean) => {
		this.#controller?.abort();
		this.#controller = new AbortController();
		const signal = this.#controller.signal;
		this.live = {
			ts: 0,
			question,
			model: '',
			pieces: [],
			citations: [],
			timings: { request_ms: 0, first_token_ms: null, total_ms: 0 },
			deep
		};
		this.liveError = null;
		this.liveStopped = false;
		this.reading = null;
		this.thinking = '';
		this.liveSince = Date.now();
		this.#notices.announcement = '';
		this.askedHere = true;
		revealQuestion();
		const turn = this.live;
		const fail = (message: string) => {
			this.liveError = message;
			this.#box()?.restore(question);
		};
		try {
			await ask(
				this.#slug(),
				this.chat,
				question,
				deep,
				{
					passages: (r) => {
						this.reading = r;
						this.chat = r.chat;
					},
					thinking: (text) => (this.thinking += text),
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
						this.turns.push(saved);
						this.live = null;
						if (page.url.searchParams.get('c') !== this.chat) {
							replaceState(`${page.url.pathname}?c=${encodeURIComponent(this.chat ?? '')}`, page.state);
						}
						api.chats(this.#slug()).then((list) => (this.chats = list));
						const missing = saved.citations.filter((c) => !c.verified).length;
						this.#notices.announcement = `Answer ready, with ${saved.citations.length} citations${missing ? `, ${missing} not found in the sources` : ''}.`;
					},
					error: fail
				},
				signal,
				this.#choice()
			);
			if (this.live === turn && !this.liveError && !this.liveStopped) fail('The answer broke off before it finished.');
		} catch (e) {
			if (!signal.aborted) fail(describe(e));
		}
	};

	retry = () => {
		if (!this.live) return;
		this.#box()?.forget(this.live.question);
		this.ask(this.live.question, this.live.deep);
	};

	stop = () => {
		this.#controller?.abort();
		this.liveStopped = true;
	};

	open = async (id: string) => {
		if (this.busy) this.stop();
		this.askedHere = false;
		try {
			this.turns = await api.turns(this.#slug(), id);
			this.chat = id;
			this.live = null;
			replaceState(`${page.url.pathname}?c=${encodeURIComponent(id)}`, page.state);
			await tick();
			scroller().scrollTop = 0;
		} catch (e) {
			this.#notices.failed(e);
		}
	};

	startNew = async () => {
		if (this.busy) this.stop();
		this.askedHere = false;
		this.chat = null;
		this.turns = [];
		this.live = null;
		replaceState(page.url.pathname, page.state);
		scroller().scrollTop = 0;
		this.#box()?.focus();
		this.#onempty();
	};

	remove = async () => {
		if (!this.chat) return;
		try {
			await api.deleteChat(this.#slug(), this.chat);
			this.chats = await api.chats(this.#slug());
			await this.startNew();
		} catch (e) {
			this.#notices.failed(e);
		}
	};

	/** Saves an answer as a note. Resolves to an error message, or null once saved. */
	saveAnswer = async (turn: Turn): Promise<string | null> => {
		try {
			const note = await api.createNote(this.#slug(), turn.question, markdownOf(turn.pieces, turn.citations));
			this.savedNotes.set(turn.ts, note.id);
			this.savedNotes = new Map(this.savedNotes);
			await this.#onnote();
			this.#notices.announcement = 'Saved to notes.';
			return null;
		} catch (e) {
			return describe(e);
		}
	};
}
