// Renders one page of a PDF source with the quoted words marked on it. pdf.js
// is large, so it loads on first use.

import { api } from './api.ts';
import { quoteRange } from './quote.ts';

type PdfJs = typeof import('pdfjs-dist');
type Document = Awaited<ReturnType<PdfJs['getDocument']>['promise']>;

let library: Promise<PdfJs> | null = null;
const documents = new Map<string, Promise<Document>>();

function pdfjs(): Promise<PdfJs> {
	library ??= Promise.all([
		import('pdfjs-dist'),
		import('pdfjs-dist/build/pdf.worker.min.mjs?url')
	]).then(([lib, worker]) => {
		lib.GlobalWorkerOptions.workerSrc = worker.default;
		return lib;
	});
	return library;
}

/** The source's PDF, fetched once per page load. */
function document(slug: string, sourceId: string): Promise<Document> {
	const key = `${slug}/${sourceId}`;
	let doc = documents.get(key);
	if (!doc) {
		doc = Promise.all([pdfjs(), api.sourceFile(slug, sourceId)]).then(
			([lib, bytes]) => lib.getDocument({ data: new Uint8Array(bytes) }).promise
		);
		doc.catch(() => documents.delete(key));
		documents.set(key, doc);
	}
	return doc;
}

/** Draws page `number` to fit `holder`'s width and marks each quote found on it.
 * Resolves to how many quotes were found. */
export async function renderPage(
	holder: HTMLElement,
	slug: string,
	sourceId: string,
	number: number,
	quotes: string[]
): Promise<number> {
	const [lib, doc] = await Promise.all([pdfjs(), document(slug, sourceId)]);
	const page = await doc.getPage(number);
	const viewport = page.getViewport({ scale: holder.clientWidth / page.getViewport({ scale: 1 }).width });
	const ratio = window.devicePixelRatio || 1;

	const canvas = window.document.createElement('canvas');
	canvas.width = Math.floor(viewport.width * ratio);
	canvas.height = Math.floor(viewport.height * ratio);
	canvas.style.width = `${viewport.width}px`;
	canvas.style.height = `${viewport.height}px`;
	const text = window.document.createElement('div');
	text.className = 'textLayer';
	holder.replaceChildren(canvas, text);
	// The text layer sizes its spans from these; pdf.js's viewer stylesheet
	// would set them, but it also restyles the whole page.
	holder.style.setProperty('--total-scale-factor', String(viewport.scale));
	holder.style.setProperty('--scale-round-x', '1px');
	holder.style.setProperty('--scale-round-y', '1px');
	holder.style.height = `${viewport.height}px`;

	await page.render({
		canvas,
		viewport,
		transform: ratio === 1 ? undefined : [ratio, 0, 0, ratio, 0, 0]
	}).promise;
	const layer = new lib.TextLayer({ textContentSource: page.streamTextContent(), container: text, viewport });
	await layer.render();
	return mark(layer.textDivs, layer.textContentItemsStr, quotes);
}

/** Wraps the quoted characters of each text span in <mark>. Spans are runs of
 * text the page lays out together, so a quote usually crosses several. */
function mark(divs: HTMLElement[], strings: string[], quotes: string[]): number {
	const starts: number[] = [];
	let joined = '';
	for (const s of strings) {
		starts.push(joined.length);
		joined += s;
	}
	const ranges = quotes.map((q) => quoteRange(joined, q)).filter((r): r is [number, number] => r !== null);
	divs.forEach((div, i) => {
		const from = starts[i];
		const to = from + strings[i].length;
		const hits = ranges
			.map(([a, b]) => [Math.max(a, from) - from, Math.min(b, to) - from] as const)
			.filter(([a, b]) => a < b)
			.sort((x, y) => x[0] - y[0]);
		if (!hits.length) return;
		const parts: Node[] = [];
		let at = 0;
		for (const [a, b] of hits) {
			if (a < at) continue;
			if (a > at) parts.push(window.document.createTextNode(strings[i].slice(at, a)));
			const m = window.document.createElement('mark');
			m.textContent = strings[i].slice(a, b);
			parts.push(m);
			at = b;
		}
		if (at < strings[i].length) parts.push(window.document.createTextNode(strings[i].slice(at)));
		div.replaceChildren(...parts);
	});
	return ranges.length;
}
