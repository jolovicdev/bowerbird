// The study sets of a notebook, and the writing of new ones: what is being
// written, how far along it is, and the model's reasoning while it works.

import { goto } from '$app/navigation';
import {
	api,
	createStudyStream,
	type AudioFormat,
	type Difficulty,
	type StudyKind,
	type StudySet,
	type StudySummary
} from '#lib/api.ts';
import type { Notices, Thought } from '#lib/notices.svelte.ts';
import type { SheetState } from '#lib/sheet.svelte.ts';

/** Study sets that open on a page of their own rather than in the sheet. */
const PAGES: Partial<Record<StudyKind, string>> = { mindmap: 'map', slides: 'slides', infographic: 'infographic' };

const WRITING: Record<StudyKind, string> = {
	flashcards: 'Writing flashcards',
	quiz: 'Writing a quiz',
	mindmap: 'Drawing a mind map',
	table: 'Building a table',
	slides: 'Making slides',
	infographic: 'Laying out an infographic',
	audio: 'Writing an audio overview'
};

const READY: Record<StudyKind, string> = {
	flashcards: 'Flashcards ready.',
	quiz: 'Quiz ready.',
	mindmap: 'Mind map ready.',
	table: 'Table ready.',
	slides: 'Slides ready.',
	infographic: 'Infographic ready.',
	audio: 'Audio overview script ready; recording it now.'
};

/** One study set being written. */
export type StudyRun = {
	kind: StudyKind;
	/** How far along it is, such as "Thinking, 12 s". */
	detail: string;
	/** The model's reasoning for this set. */
	thought: Thought;
};

export class StudyRuns {
	sets = $state<StudySummary[]>([]);
	/** The study sets being written right now. Several can be, each with its own progress. */
	writing = $state<StudyRun[]>([]);

	#slug: () => string;
	#notices: Notices;
	#sheet: SheetState;

	constructor(options: { slug: () => string; notices: Notices; sheet: SheetState }) {
		this.#slug = options.slug;
		this.#notices = options.notices;
		this.#sheet = options.sheet;
	}

	refresh = async () => {
		this.sets = await api.studio(this.#slug());
	};

	/** Opens a study set where its kind is shown: on its own page, or in the sheet. */
	open = (set: { id: string; kind: StudyKind }) => {
		const path = PAGES[set.kind];
		if (path) goto(`/n/${encodeURIComponent(this.#slug())}/${path}/${set.id}`);
		else this.#sheet.open({ studyId: set.id });
	};

	create = async (
		kind: StudyKind,
		count: number,
		difficulty: Difficulty,
		topic: string,
		collect = '',
		format: AudioFormat = 'deep-dive'
	) => {
		const notices = this.#notices;
		this.writing.push({ kind, detail: '', thought: { text: '', shown: false } });
		// Read back, so that what is written to the run reaches the page.
		const run = this.writing.at(-1)!;
		const what = WRITING[kind];
		const startedAt = Date.now();
		let phase: 'passages' | 'thinking' | 'writing' = 'passages';
		const statusLine = () => {
			const sec = Math.max(0, Math.round((Date.now() - startedAt) / 1000));
			if (phase === 'thinking') return `Thinking, ${sec} s`;
			if (phase === 'writing') return `Writing, ${sec} s`;
			return `Reading sources, ${sec} s`;
		};
		// The notice is this run's for as long as it carries this run's reasoning.
		const noticed = () => notices.notice?.thought === run.thought;
		const report = () => {
			notices.notice = { text: `${what} (${run.detail}). It opens when ready.`, problem: false, thought: run.thought };
		};
		run.detail = statusLine();
		report();
		const tickTimer = setInterval(() => {
			run.detail = statusLine();
			if (noticed()) report();
		}, 1000);
		// An object, because TypeScript does not follow what the handlers assign to a plain variable.
		const outcome: { set: StudySet | null; error: string | null } = { set: null, error: null };
		try {
			await createStudyStream(this.#slug(), kind, count, difficulty, topic, collect, format, {
				stage: (s) => {
					phase = s.stage;
					run.detail = statusLine();
				},
				thinking: (chunk) => {
					phase = 'thinking';
					run.thought.text += chunk;
				},
				done: (set) => {
					outcome.set = set;
				},
				error: (message) => {
					outcome.error = message;
				}
			});
			if (outcome.error) {
				notices.problem(outcome.error);
				return;
			}
			if (!outcome.set) {
				notices.problem(`${what} stopped before finishing. Try again.`);
				return;
			}
			await this.refresh();
			notices.announcement = READY[kind];
			if (noticed()) notices.notice = null;
			if (PAGES[kind] || !this.#sheet.pane) this.open({ id: outcome.set.id, kind });
		} catch (e) {
			notices.failed(e);
		} finally {
			clearInterval(tickTimer);
			this.writing.splice(this.writing.indexOf(run), 1);
		}
	};
}
