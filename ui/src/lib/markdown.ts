// The subset of Markdown models write in answers, parsed to a tree that
// components render, so no model output ever reaches the page as HTML.
// Citations travel through the text as private-use placeholders.

import type { Citation, Piece } from './api.ts';

export type Inline =
	| { kind: 'text'; text: string }
	| { kind: 'strong' | 'em'; children: Inline[] }
	| { kind: 'code'; text: string }
	| { kind: 'link'; href: string; children: Inline[] }
	| { kind: 'cite'; index: number }
	/** A citation with the word before it and the punctuation after it, kept on one line. */
	| { kind: 'glue'; children: Inline[] };

export type Block =
	| { kind: 'p'; children: Inline[] }
	| { kind: 'h'; children: Inline[] }
	| { kind: 'ul' | 'ol'; items: Inline[][] }
	| { kind: 'quote'; children: Inline[] }
	| { kind: 'pre'; text: string };

const OPEN = '';
const CLOSE = '';

/** One string with `OPEN group CLOSE` where citations were, plus the groups.
 * Citations directly next to each other from the same source share one group,
 * so "[A][A]" renders as one mark that opens both passages. */
export function joinPieces(pieces: Piece[], citations: Citation[]): { text: string; groups: number[][] } {
	let text = '';
	const groups: number[][] = [];
	let open: number[] | null = null;
	for (const piece of pieces) {
		if ('text' in piece) {
			if (piece.text.trim()) open = null;
			// Model text must never forge a placeholder.
			text += piece.text.replace(/[\uE000\uE001]/g, '');
			continue;
		}
		const citation = citations[piece.cite];
		if (!citation) continue;
		if (open && citations[open[0]].letter === citation.letter) {
			open.push(piece.cite);
			text = text.replace(/[ \t]+$/, '');
			continue;
		}
		open = [piece.cite];
		groups.push(open);
		text = text.replace(/[ \t]+$/, '') + `${OPEN}${groups.length - 1}${CLOSE}`;
	}
	return { text, groups };
}

const INLINE = new RegExp(
	[
		'`([^`]+)`',
		'\\*\\*([\\s\\S]+?)\\*\\*',
		'__([\\s\\S]+?)__',
		'\\*([^\\s*][\\s\\S]*?)\\*',
		'(?<![\\p{L}\\p{N}])_([^\\s_][\\s\\S]*?)_(?![\\p{L}\\p{N}])',
		'\\\\\\(([\\s\\S]+?)\\\\\\)',
		'\\[([^\\]]+)\\]\\((https?://[^)\\s]+)\\)',
		`${OPEN}(\\d+)${CLOSE}`
	].join('|'),
	'u'
);

export function inline(src: string): Inline[] {
	const out: Inline[] = [];
	const text = (t: string) => {
		const last = out[out.length - 1];
		if (last?.kind === 'text') last.text += t;
		else if (t) out.push({ kind: 'text', text: t });
	};
	let rest = src;
	while (rest) {
		const m = INLINE.exec(rest);
		if (!m) {
			text(rest);
			break;
		}
		text(rest.slice(0, m.index));
		if (m[1] !== undefined) out.push({ kind: 'code', text: m[1] });
		else if (m[2] !== undefined || m[3] !== undefined) out.push({ kind: 'strong', children: inline(m[2] ?? m[3]) });
		else if (m[4] !== undefined || m[5] !== undefined) out.push({ kind: 'em', children: inline(m[4] ?? m[5]) });
		// Inline TeX: show the formula text without its delimiters.
		else if (m[6] !== undefined) text(m[6]);
		else if (m[7] !== undefined) out.push({ kind: 'link', href: m[8], children: inline(m[7]) });
		else if (m[9] !== undefined) out.push({ kind: 'cite', index: Number(m[9]) });
		rest = rest.slice(m.index + m[0].length);
	}
	return glue(out);
}

const CLOSING_PUNCTUATION = /^[.,;:!?)\]”’"']+/;

/** Wraps each run of citations with the word before it and the punctuation
 * after it, so a mark never starts a line or leaves a lone period behind. */
function glue(nodes: Inline[]): Inline[] {
	const out: Inline[] = [];
	for (let i = 0; i < nodes.length; i++) {
		if (nodes[i].kind !== 'cite') {
			out.push(nodes[i]);
			continue;
		}
		const children: Inline[] = [];
		const before = out.at(-1);
		const word = before?.kind === 'text' ? before.text.match(/\S+$/)?.[0] : undefined;
		if (before?.kind === 'text' && word) {
			before.text = before.text.slice(0, -word.length);
			if (!before.text) out.pop();
			children.push({ kind: 'text', text: word });
		}
		children.push(nodes[i]);
		while (nodes[i + 1]?.kind === 'cite') children.push(nodes[++i]);
		const after = nodes[i + 1];
		const punctuation = after?.kind === 'text' ? after.text.match(CLOSING_PUNCTUATION)?.[0] : undefined;
		if (after?.kind === 'text' && punctuation) {
			after.text = after.text.slice(punctuation.length);
			children.push({ kind: 'text', text: punctuation });
		}
		out.push({ kind: 'glue', children });
	}
	return out;
}

export function parse(src: string): Block[] {
	const blocks: Block[] = [];
	const lines = src.replace(/\r\n?/g, '\n').split('\n');
	let para: string[] = [];
	let list: { kind: 'ul' | 'ol'; items: string[] } | null = null;
	const flushPara = () => {
		if (para.length) blocks.push({ kind: 'p', children: inline(para.join(' ')) });
		para = [];
	};
	const flushList = () => {
		if (list) blocks.push({ kind: list.kind, items: list.items.map(inline) });
		list = null;
	};

	for (let i = 0; i < lines.length; i++) {
		const line = lines[i];
		const fence = line.match(/^\s*(```|~~~)/);
		if (fence) {
			flushPara();
			flushList();
			const body: string[] = [];
			for (i++; i < lines.length && !lines[i].trimStart().startsWith(fence[1]); i++) body.push(lines[i]);
			blocks.push({ kind: 'pre', text: body.join('\n') });
			continue;
		}
		if (!line.trim()) {
			flushPara();
			flushList();
			continue;
		}
		const heading = line.match(/^\s{0,3}#{1,6}\s+(.*?)[\s#]*$/);
		if (heading) {
			flushPara();
			flushList();
			blocks.push({ kind: 'h', children: inline(heading[1]) });
			continue;
		}
		const bullet = line.match(/^\s*[-*+]\s+(.*)$/);
		const numbered = line.match(/^\s*\d+[.)]\s+(.*)$/);
		const item = bullet ?? numbered;
		if (item) {
			flushPara();
			const kind = bullet ? 'ul' : 'ol';
			if (list && list.kind !== kind) flushList();
			list ??= { kind, items: [] };
			list.items.push(item[1]);
			continue;
		}
		if (list && /^\s{2,}\S/.test(line)) {
			list.items[list.items.length - 1] += ' ' + line.trim();
			continue;
		}
		const quote = line.match(/^\s*>\s?(.*)$/);
		if (quote) {
			flushPara();
			flushList();
			blocks.push({ kind: 'quote', children: inline(quote[1]) });
			continue;
		}
		flushList();
		para.push(line.trim());
	}
	flushPara();
	flushList();
	return blocks;
}

/** Citation groups in each paragraph, heading, quote, and list item, in reading order. */
export function citeRuns(blocks: Block[]): number[][] {
	const collect = (nodes: Inline[], out: number[]): number[] => {
		for (const node of nodes) {
			if (node.kind === 'cite') out.push(node.index);
			else if ('children' in node) collect(node.children, out);
		}
		return out;
	};
	return blocks.flatMap((block) =>
		block.kind === 'ul' || block.kind === 'ol'
			? block.items.map((item) => collect(item, []))
			: 'children' in block
				? [collect(block.children, [])]
				: []
	);
}
