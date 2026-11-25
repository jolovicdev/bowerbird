// Reports being written into notes: the draft shown as it arrives, and the
// names of the reports still on their way.

import {
	describe,
	writeReport,
	type Citation,
	type Piece,
	type ReportKind
} from '#lib/api.ts';
import type { Notices } from '#lib/notices.svelte.ts';
import type { SheetState } from '#lib/sheet.svelte.ts';

const NAMES: Record<ReportKind, string> = {
	'study-guide': 'study guide',
	briefing: 'briefing',
	timeline: 'timeline',
	faq: 'FAQ',
	custom: 'report'
};

export type Draft = {
	title: string;
	pieces: Piece[];
	citations: Citation[];
	thinking: string;
	since: number;
	error: string | null;
};

export class ReportDrafts {
	/** The report being written, shown as it arrives. */
	draft = $state<Draft | null>(null);
	/** Names of the reports being written right now. */
	writing = $state<string[]>([]);

	#slug: () => string;
	#notices: Notices;
	#sheet: SheetState;
	#onnote: () => void;

	constructor(options: {
		slug: () => string;
		notices: Notices;
		sheet: SheetState;
		/** A report became a note. */
		onnote: () => void;
	}) {
		this.#slug = options.slug;
		this.#notices = options.notices;
		this.#sheet = options.sheet;
		this.#onnote = options.onnote;
	}

	write = async (kind: ReportKind, topic: string, instructions: string) => {
		const sheet = this.#sheet;
		const name = NAMES[kind];
		this.writing.push(name);
		const unlist = () => this.writing.includes(name) && this.writing.splice(this.writing.indexOf(name), 1);
		this.draft = { title: `Writing a ${name}…`, pieces: [], citations: [], thinking: '', since: Date.now(), error: null };
		const current = this.draft;
		if (!sheet.pane) sheet.open({ draft: true });
		// Whether the stream said how the report ended, as a note or as a failure.
		let ended = false;
		const fail = (message: string) => {
			ended = true;
			current.error = message;
			if (!(sheet.pane && 'draft' in sheet.pane)) this.#notices.problem(message);
		};
		try {
			await writeReport(this.#slug(), kind, topic, instructions, {
				note: (note) => {
					current.title = note.title;
				},
				thinking: (text) => {
					current.thinking += text;
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
					ended = true;
					this.#notices.announcement = 'Report ready.';
					unlist();
					this.#onnote();
					// The draft becomes the note, in the same sheet and history entry.
					if (sheet.pane && 'draft' in sheet.pane) sheet.pane = { noteId: note.id };
					if (this.draft === current) this.draft = null;
				},
				error: fail
			});
			// The server writes the report to its end whether or not anyone is listening.
			if (!ended) fail('The connection dropped while the report was being written. It should still arrive in your notes.');
		} catch (e) {
			fail(describe(e));
		} finally {
			unlist();
		}
	};
}
