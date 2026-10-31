// A passage from a Markdown source, split into blocks for reading, with the
// quoted words marked. Ranges are offsets into the stored text, so markup is
// hidden without moving a quote off the words it points at.

export type Run = { text: string; hit: boolean; strong: boolean; em: boolean; code: boolean };

export type Block =
	| { kind: 'heading'; level: number; runs: Run[] }
	| { kind: 'paragraph'; runs: Run[] }
	| { kind: 'item'; depth: number; marker: string; runs: Run[] }
	| { kind: 'quote'; runs: Run[] }
	| { kind: 'code'; runs: Run[] }
	| { kind: 'table'; rows: Run[][][] }
	/** A picture: its position in the list of figures passed in. */
	| { kind: 'figure'; index: number };

export type Span = [number, number];

const HEADING = /^ {0,3}(#{1,6})[ \t]+/;
const ITEM = /^([ \t]*)([-*+]|\d{1,9}[.)])[ \t]+/;
const QUOTE = /^ {0,3}>[ \t]?/;
const FENCE = /^ {0,3}(```|~~~)/;
const RULE = /^ {0,3}([-*_])([ \t]*\1){2,}[ \t]*$/;
const TABLE_DIVIDER = /^[ \t]*\|?([ \t]*:?-+:?[ \t]*\|)+[ \t]*(:?-+:?[ \t]*)?$/;

type Open =
	| { kind: 'paragraph' | 'quote' | 'code'; spans: Span[] }
	| { kind: 'item'; depth: number; marker: string; spans: Span[] }
	| { kind: 'table'; rows: Span[][] };

/** `figures` are sorted by `at`, a UTF-16 offset into `text`; each goes before
 * the first line that starts at or after it. */
export function markdownBlocks(text: string, hits: Span[], figures: { at?: number }[] = []): Block[] {
	const blocks: Block[] = [];
	let open: Open | null = null;
	let nextFigure = 0;
	const close = () => {
		if (!open) return;
		if (open.kind === 'table') {
			blocks.push({ kind: 'table', rows: open.rows.map((row) => row.map((cell) => runs(text, [cell], hits))) });
		} else if (open.kind === 'item') {
			blocks.push({ kind: 'item', depth: open.depth, marker: open.marker, runs: runs(text, open.spans, hits) });
		} else if (open.spans.length) {
			blocks.push({ kind: open.kind, runs: runs(text, open.spans, hits, open.kind === 'code') });
		}
		open = null;
	};

	const placeFigures = (offset: number) => {
		while (nextFigure < figures.length && (figures[nextFigure].at ?? 0) <= offset) {
			close();
			blocks.push({ kind: 'figure', index: nextFigure++ });
		}
	};

	let start = 0;
	for (const line of text.split('\n')) {
		const span: Span = [start, start + line.length];
		start += line.length + 1;
		placeFigures(span[0]);
		if (open?.kind === 'code') {
			if (FENCE.test(line)) close();
			else open.spans.push(span);
			continue;
		}
		if (FENCE.test(line)) {
			close();
			open = { kind: 'code', spans: [] };
			continue;
		}
		if (!line.trim() || RULE.test(line)) {
			close();
			continue;
		}
		const heading = HEADING.exec(line);
		if (heading) {
			close();
			const end = span[0] + line.replace(/[ \t]+#+[ \t]*$/, '').trimEnd().length;
			blocks.push({ kind: 'heading', level: heading[1].length, runs: runs(text, [[span[0] + heading[0].length, end]], hits) });
			continue;
		}
		if (line.trimStart().startsWith('|')) {
			if (open?.kind !== 'table') {
				close();
				open = { kind: 'table', rows: [] };
			}
			if (!TABLE_DIVIDER.test(line)) open.rows.push(cells(line, span[0]));
			continue;
		}
		const item = ITEM.exec(line);
		if (item) {
			close();
			const indent = item[1].replace(/\t/g, '  ').length;
			open = {
				kind: 'item',
				depth: Math.floor(indent / 2),
				marker: /\d/.test(item[2]) ? item[2] : '•',
				spans: [[span[0] + item[0].length, span[1]]]
			};
			continue;
		}
		const quote = QUOTE.exec(line);
		if (quote) {
			if (open?.kind !== 'quote') {
				close();
				open = { kind: 'quote', spans: [] };
			}
			open.spans.push([span[0] + quote[0].length, span[1]]);
			continue;
		}
		if (open?.kind === 'paragraph' || open?.kind === 'item' || open?.kind === 'quote') {
			open.spans.push([span[0] + (line.length - line.trimStart().length), span[1]]);
		} else {
			close();
			open = { kind: 'paragraph', spans: [span] };
		}
	}
	close();
	placeFigures(Infinity);
	return blocks;
}

/** Cells of a table row, as spans without the pipes and padding around them. */
function cells(line: string, offset: number): Span[] {
	const out: Span[] = [];
	let from = 0;
	for (let i = 0; i <= line.length; i++) {
		if (i < line.length && (line[i] !== '|' || line[i - 1] === '\\')) continue;
		const raw = line.slice(from, i);
		const lead = raw.length - raw.trimStart().length;
		out.push([offset + from + lead, offset + from + raw.trimEnd().length]);
		from = i + 1;
	}
	// The pipes that open and close the row leave an empty cell at each end.
	if (line.trimStart().startsWith('|')) out.shift();
	if (line.trimEnd().endsWith('|')) out.pop();
	return out;
}

const word = (c: string | undefined) => !!c && /[\p{L}\p{N}]/u.test(c);
const space = (c: string | undefined) => !c || /\s/.test(c);

/** Visible text of the spans with emphasis, code, escapes, and link targets
 * taken out; a line break inside a block reads as a space. */
function runs(text: string, spans: Span[], hits: Span[], preformatted = false): Run[] {
	const out: Run[] = [];
	const style = { strong: false, em: false, code: preformatted };
	const limit = spans.at(-1)?.[1] ?? 0;
	const push = (ch: string, at: number) => {
		const hit = hits.some(([s, e]) => at >= s && at < e);
		const last = out.at(-1);
		if (last && last.hit === hit && last.strong === style.strong && last.em === style.em && last.code === style.code) {
			last.text += ch;
		} else {
			out.push({ text: ch, hit, ...style });
		}
	};
	const later = (marker: string, from: number) => {
		const at = text.indexOf(marker, from);
		return at !== -1 && at < limit;
	};
	spans.forEach(([start, end], n) => {
		if (n > 0) push(preformatted ? '\n' : ' ', start - 1);
		let i = start;
		while (i < end) {
			const c = text[i];
			if (preformatted) {
				push(c, i++);
			} else if (style.code) {
				if (c === '`') style.code = false;
				else push(c, i);
				i++;
			} else if (c === '\\' && i + 1 < end && /[\\`*_{}[\]()#+\-.!|>~]/.test(text[i + 1])) {
				push(text[i + 1], i + 1);
				i += 2;
			} else if (c === '`' && later('`', i + 1)) {
				style.code = true;
				i++;
			} else if ((c === '*' || c === '_') && text[i + 1] === c) {
				const inWord = c === '_' && word(text[i - 1]) && word(text[i + 2]);
				if (!inWord && (style.strong || (!space(text[i + 2]) && later(c + c, i + 2)))) {
					style.strong = !style.strong;
					i += 2;
				} else {
					push(c, i);
					push(c, i + 1);
					i += 2;
				}
			} else if (c === '*' || c === '_') {
				const inWord = c === '_' && word(text[i - 1]) && word(text[i + 1]);
				const opens = !style.em && !space(text[i + 1]) && later(c, i + 1);
				const closes = style.em && !space(text[i - 1]);
				if (!inWord && (opens || closes)) style.em = !style.em;
				else push(c, i);
				i++;
			} else if (c === '[') {
				const middle = text.indexOf('](', i);
				const close = middle === -1 ? -1 : text.indexOf(')', middle);
				if (middle !== -1 && close !== -1 && close < end && !text.slice(i, close).includes('\n')) {
					for (let j = i + 1; j < middle; j++) push(text[j], j);
					i = close + 1;
				} else {
					push(c, i++);
				}
			} else {
				push(c, i++);
			}
		}
	});
	return out;
}
