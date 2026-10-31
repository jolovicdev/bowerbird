import type { Progress } from '#lib/api.ts';

const relative = new Intl.RelativeTimeFormat(undefined, { numeric: 'auto' });
const STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
	['year', 31_536_000],
	['month', 2_592_000],
	['week', 604_800],
	['day', 86_400],
	['hour', 3_600],
	['minute', 60]
];

/** "2 hours ago", "yesterday", "just now". */
export function ago(unixSeconds: number, now = Date.now() / 1000): string {
	const diff = unixSeconds - now;
	for (const [unit, seconds] of STEPS) {
		if (Math.abs(diff) >= seconds) return relative.format(Math.round(diff / seconds), unit);
	}
	return 'just now';
}

export function seconds(ms: number): string {
	return ms < 10_000 ? (ms / 1000).toFixed(1) : String(Math.round(ms / 1000));
}

// Pasted text is stored as Markdown; to the reader both are text.
const KIND_NAMES = {
	pdf: 'PDF',
	markdown: 'Text',
	text: 'Text',
	web: 'Web page',
	docx: 'Word document',
	epub: 'EPUB',
	youtube: 'YouTube video',
	audio: 'Recording'
} as const;

export function sourceMeta(kind: keyof typeof KIND_NAMES, pages: number | null): string {
	return pages ? `${KIND_NAMES[kind]}, ${pages} ${pages === 1 ? 'page' : 'pages'}` : KIND_NAMES[kind];
}

/** "1:02:03" or "2:45". */
export function clock(seconds: number): string {
	const s = Math.floor(seconds);
	const mmss = `${String(Math.floor(s / 60) % 60)}:${String(s % 60).padStart(2, '0')}`;
	return s >= 3600 ? `${Math.floor(s / 3600)}:${mmss.padStart(5, '0')}` : mmss;
}

/** Where a recording's transcription is, for a source's meta line. */
export function transcribingNote(p: Progress): string {
	const mb = (bytes: number) => Math.round(bytes / 1_000_000);
	if (p.stage === 'downloading')
		return p.total ? `Downloading the transcriber, ${mb(p.done)} of ${mb(p.total)} MB` : 'Downloading the transcriber';
	if (p.stage === 'listening' && p.done)
		return `Transcribing, ${clock(p.done)}${p.total ? ` of ${clock(p.total)}` : ''}`;
	if (p.stage === 'listening') return 'Transcribing';
	return 'Waiting to be transcribed';
}

/** Where the OCR of a scanned PDF is, for a source's meta line. */
export function readingNote(p: Progress): string {
	const mb = (bytes: number) => Math.round(bytes / 1_000_000);
	if (p.stage === 'downloading')
		return p.total ? `Downloading the text reader, ${mb(p.done)} of ${mb(p.total)} MB` : 'Downloading the text reader';
	if (p.stage === 'reading') return `Reading scanned pages, ${p.done} of ${p.total}`;
	return 'Waiting to read the scanned pages';
}

/** Where the embedding of a notebook's passages is, under the source list. */
export function embeddingNote(p: Progress): string {
	const mb = (bytes: number) => Math.round(bytes / 1_000_000);
	if (p.stage === 'downloading')
		return p.total
			? `Downloading the search model, ${mb(p.done)} of ${mb(p.total)} MB`
			: 'Downloading the search model';
	if (p.stage === 'failed') return `Search by meaning is off: ${p.problem} Keyword search still works.`;
	if (p.stage === 'embedding' && p.total)
		return `Indexing by meaning, ${Math.floor((p.done / p.total) * 100)}%. Keyword search works meanwhile.`;
	return 'Waiting to index by meaning';
}
