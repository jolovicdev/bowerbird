// The sheet beside the text, or over it on phones: what it shows, the way
// back to the note or study set a passage was opened from, and where focus
// returns when it closes. It takes a history entry, so Back closes it.

import { tick } from 'svelte';
import { pushState } from '$app/navigation';
import { page } from '$app/state';
import { api, describe, passageOrd, type Citation } from '#lib/api.ts';
import type { Entry } from '#lib/components/SourcePane.svelte';

/** Cited passages, a source, a note, a study set, or a report being written. */
export type Pane =
	| { citations: Citation[]; entries: Entry[] }
	| { sourceId: string }
	| { noteId: string }
	| { studyId: string }
	| { draft: true };

export class SheetState {
	pane = $state<Pane | null>(null);
	/** The note or study set a passage was opened from, to go back to. */
	back = $state<Pane | null>(null);
	#opener: HTMLElement | null = null;
	#slug: () => string;

	/** Call while a component initializes: it watches the history entry. */
	constructor(slug: () => string) {
		this.#slug = slug;
		// Back, from the browser or the phone, closes the sheet: the history
		// entry the sheet pushed is gone. page.state updates after pushState
		// returns, so only a change from shown to gone counts.
		let inHistory = false;
		$effect(() => {
			const shown = page.state.sheet === true;
			if (inHistory && !shown && this.pane) this.#forget();
			inHistory = shown;
		});
	}

	open = (item: Pane) => {
		this.#opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
		this.back = null;
		this.#show(item);
	};

	openCitations = (citations: Citation[], el: HTMLElement) => {
		this.#opener = el;
		this.back = null;
		const byLabel = new Map<string, Entry>();
		for (const c of citations) {
			if (!c.source_id) continue;
			const entry = byLabel.get(c.label);
			if (entry) entry.citations.push(c);
			else byLabel.set(c.label, { label: c.label, sourceId: c.source_id, citations: [c], passage: null, error: null });
		}
		this.#show({ citations, entries: [...byLabel.values()] });
		// Through the state proxy, so loaded passages show up.
		const shown = this.pane as { entries: Entry[] };
		for (const entry of shown.entries) {
			api.passage(this.#slug(), entry.sourceId, passageOrd(entry.citations[0]))
				.then((p) => (entry.passage = p))
				.catch((e) => (entry.error = describe(e)));
		}
	};

	/** Opens a passage cited in the note or study set on the sheet, keeping the way back. */
	openFromSheet = (citations: Citation[], el: HTMLElement) => {
		const from = this.pane;
		const keep = this.#opener;
		this.openCitations(citations, el);
		this.back = from;
		this.#opener = keep;
	};

	goBack = () => {
		this.pane = this.back;
		this.back = null;
	};

	close = () => {
		if (page.state.sheet) history.back();
		else this.#forget();
	};

	#show(next: Pane) {
		if (!page.state.sheet) pushState('', { sheet: true });
		this.pane = next;
	}

	async #forget() {
		this.pane = null;
		this.back = null;
		await tick();
		this.#opener?.focus();
	}
}
