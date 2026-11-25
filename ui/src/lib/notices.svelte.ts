// What a notebook page tells the reader outside its text: one line they can
// dismiss, and words for screen readers when something finishes.

import { describe } from '#lib/api.ts';

/** A model's reasoning that a notice offers to show, kept by the work that is still adding to it. */
export type Thought = { text: string; shown: boolean };

export type Notice = { text: string; problem: boolean; thought?: Thought };

export class Notices {
	notice = $state<Notice | null>(null);
	/** Read out by screen readers when something finishes. */
	announcement = $state('');

	say = (text: string) => {
		this.notice = { text, problem: false };
	};

	problem = (text: string) => {
		this.notice = { text, problem: true };
	};

	/** Shows what went wrong with a request. */
	failed = (error: unknown) => this.problem(describe(error));
}
