// Adding sources to a notebook: what is on its way in, what just arrived,
// and what the reader is told about sources that were already there.

import { api, describe, UPLOAD_MB, type Notebook } from '#lib/api.ts';
import type { Notices } from '#lib/notices.svelte.ts';

const lettersList = (letters: string[]) =>
	letters.length < 3 ? letters.join(' and ') : `${letters.slice(0, -1).join(', ')} and ${letters.at(-1)}`;

export class Additions {
	/** Names of the sources being added right now. */
	pending = $state<string[]>([]);
	/** Ids of the sources the last add brought in. */
	fresh = $state(new Set<string>());

	#slug: () => string;
	#notices: Notices;
	#notebook: () => Notebook | null;
	#onadded: (notebook: Notebook) => void;

	constructor(options: {
		slug: () => string;
		notices: Notices;
		notebook: () => Notebook | null;
		/** The notebook as it is after an add. */
		onadded: (notebook: Notebook) => void;
	}) {
		this.#slug = options.slug;
		this.#notices = options.notices;
		this.#notebook = options.notebook;
		this.#onadded = options.onadded;
	}

	/**
	 * Runs an add, whose work resolves to the updated notebook, or null when
	 * nothing was added. Resolves to an error message, or null when it worked.
	 */
	async #run(names: string[], work: () => Promise<Notebook | null>): Promise<string | null> {
		const notices = this.#notices;
		this.pending.push(...names);
		notices.notice = null;
		const before = new Set(this.#notebook()?.sources.map((s) => s.id));
		try {
			const updated = await work();
			if (!updated) return null;
			this.fresh = new Set(updated.sources.filter((s) => !before.has(s.id)).map((s) => s.id));
			this.#onadded(updated);
			const notes: string[] = [];
			if (updated.already?.length) {
				notes.push(`Already in this notebook as ${lettersList(updated.already)}.`);
			}
			if (updated.skipped?.length) {
				notes.push(
					`Skipped ${updated.skipped.length === 1 ? updated.skipped[0] : `${updated.skipped.length} unsupported items`}.`
				);
			}
			if (notes.length) {
				notices.say(notes.join(' '));
			} else if (this.fresh.size) {
				notices.announcement = this.fresh.size === 1 ? 'Source added.' : `${this.fresh.size} sources added.`;
			}
			return null;
		} catch (e) {
			return describe(e);
		} finally {
			for (const name of names) this.pending.splice(this.pending.indexOf(name), 1);
		}
	}

	files = async (files: File[]) => {
		const slug = this.#slug();
		const problems = new Set<string>();
		await this.#run(
			files.map((f) => f.name),
			async () => {
				// One request per file: the server's limit covers a whole request,
				// and a file it refuses leaves the others to be added.
				let updated: Notebook | null = null;
				const already: string[] = [];
				const skipped: string[] = [];
				for (const file of files) {
					if (file.size > UPLOAD_MB * 2 ** 20) {
						problems.add(`Can't add ${file.name}. A file can be up to ${UPLOAD_MB} MB.`);
						continue;
					}
					try {
						updated = await api.addFiles(slug, [file]);
						already.push(...(updated.already ?? []));
						skipped.push(...(updated.skipped ?? []));
					} catch (e) {
						problems.add(describe(e));
					}
				}
				return updated && { ...updated, already, skipped };
			}
		);
		if (problems.size) this.#notices.problem([...problems].join(' '));
	};

	url = (url: string) => this.#run([url], () => api.addUrl(this.#slug(), url));

	text = (title: string, text: string) =>
		this.#run([title || 'Pasted text'], () => api.addText(this.#slug(), title, text));
}
