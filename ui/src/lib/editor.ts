// The note editor: ProseMirror over Markdown. Citations are atom nodes that
// read and write the same [[A3|quote]] markers answers use, so a saved answer
// keeps its citations through editing.

import MarkdownIt, { type StateInline } from 'markdown-it';
import { baseKeymap, setBlockType, toggleMark, wrapIn } from 'prosemirror-commands';
import { history, redo, undo } from 'prosemirror-history';
import {
	inputRules,
	textblockTypeInputRule,
	wrappingInputRule
} from 'prosemirror-inputrules';
import { keymap } from 'prosemirror-keymap';
import {
	MarkdownParser,
	MarkdownSerializer,
	defaultMarkdownParser,
	defaultMarkdownSerializer,
	schema as markdownSchema
} from 'prosemirror-markdown';
import { Schema, type Node } from 'prosemirror-model';
import { liftListItem, sinkListItem, splitListItem, wrapInList } from 'prosemirror-schema-list';
import { EditorState, Plugin, Selection, type Command } from 'prosemirror-state';
import { Decoration, DecorationSet, EditorView } from 'prosemirror-view';
import type { Citation } from './api.ts';

const schema = new Schema({
	// Headings take any inline content, citations included, so turning a cited
	// paragraph into a heading keeps its citations.
	nodes: markdownSchema.spec.nodes
		.update('heading', { ...markdownSchema.spec.nodes.get('heading')!, content: 'inline*' })
		.addBefore('image', 'cite', {
		inline: true,
		group: 'inline',
		atom: true,
		attrs: { label: {}, quote: {} },
		toDOM: (node) => ['sup', { class: 'ref', 'data-label': node.attrs.label }, node.attrs.label],
		parseDOM: [
			{
				tag: 'sup[data-label]',
				getAttrs: (dom) => ({ label: (dom as HTMLElement).dataset.label, quote: (dom as HTMLElement).title })
			}
		]
	}),
	marks: markdownSchema.spec.marks
});

/** Reads [[A3|quote]] markers the way the server's citation parser does:
 * the marker ends at the last `]]` of a run of brackets. */
function citeRule(state: StateInline, silent: boolean): boolean {
	const { src, pos } = state;
	if (!src.startsWith('[[', pos)) return false;
	const close = src.indexOf(']]', pos + 2);
	if (close < 0) return false;
	let end = close;
	while (src[end] === ']') end++;
	const inner = src.slice(pos + 2, end - 2);
	const bar = inner.indexOf('|');
	const label = inner.slice(0, bar).trim();
	if (bar < 0 || !/^[A-Z]+\d+$/.test(label)) return false;
	if (!silent) {
		const token = state.push('cite', '', 0);
		token.meta = { label, quote: inner.slice(bar + 1).trim().replace(/^["“”]+|["“”]+$/g, '').trim() };
	}
	state.pos = end;
	return true;
}

const markdown = MarkdownIt('commonmark', { html: false }).use((md) => md.inline.ruler.before('link', 'cite', citeRule));

const parser = new MarkdownParser(schema, markdown, {
	...defaultMarkdownParser.tokens,
	cite: { node: 'cite', noCloseToken: true, getAttrs: (token) => token.meta }
});

const serializer = new MarkdownSerializer(
	{
		...defaultMarkdownSerializer.nodes,
		cite(state, node) {
			state.write(`[[${node.attrs.label}|${node.attrs.quote}]]`);
		}
	},
	defaultMarkdownSerializer.marks
);

const key = (label: string, quote: string) => `${label}|${quote}`;
const letterOf = (label: string) => label.replace(/\d+$/, '');

/** The citations written one after another from `pos`, with only spaces between. */
function citeRun(doc: Node, pos: number): Node[] {
	const at = doc.resolve(pos);
	const run: Node[] = [];
	for (let i = at.index(); i < at.parent.childCount; i++) {
		const child = at.parent.child(i);
		if (child.type.name === 'cite') run.push(child);
		else if (!(child.isText && !child.text?.trim())) break;
	}
	return run;
}

/** Shows each source's letter once per run of citations, as answers do; the
 * first letter opens every quote of the run. */
const groupCitations = new Plugin({
	props: {
		decorations(state) {
			const repeats: Decoration[] = [];
			state.doc.descendants((block, blockPos) => {
				if (!block.isTextblock) return true;
				let letters = new Set<string>();
				block.forEach((child, offset) => {
					if (child.type.name === 'cite') {
						const letter = letterOf(child.attrs.label);
						if (letters.has(letter)) {
							const pos = blockPos + 1 + offset;
							repeats.push(Decoration.node(pos, pos + child.nodeSize, { class: 'repeat' }));
						}
						letters.add(letter);
					} else if (!(child.isText && !child.text?.trim())) {
						letters = new Set();
					}
				});
				return false;
			});
			return DecorationSet.create(state.doc, repeats);
		}
	}
});

/** Commands for the formatting buttons. */
export const format: Record<string, { label: string; command: Command }> = {
	bold: { label: 'Bold', command: toggleMark(schema.marks.strong) },
	italic: { label: 'Italic', command: toggleMark(schema.marks.em) },
	heading: { label: 'Heading', command: setBlockType(schema.nodes.heading, { level: 2 }) },
	text: { label: 'Text', command: setBlockType(schema.nodes.paragraph) },
	bullets: { label: 'List', command: wrapInList(schema.nodes.bullet_list) },
	numbers: { label: 'Numbered', command: wrapInList(schema.nodes.ordered_list) },
	quote: { label: 'Quote', command: wrapIn(schema.nodes.blockquote) }
};

export type Editor = {
	view: EditorView;
	markdown: () => string;
	/** Marks each citation verified or not, after the server checked them again. */
	check: (citations: Citation[]) => void;
	run: (command: Command) => void;
	destroy: () => void;
};

export function createEditor(
	place: HTMLElement,
	body: string,
	citations: Citation[],
	on: { change: () => void; open: (citations: Citation[], el: HTMLElement) => void; label: string }
): Editor {
	let byKey = new Map(citations.map((c) => [key(c.label, c.quote), c]));
	const lookup = (node: Node) => byKey.get(key(node.attrs.label, node.attrs.quote));

	const state = EditorState.create({
		doc: parser.parse(body) ?? undefined,
		plugins: [
			groupCitations,
			history(),
			inputRules({
				rules: [
					textblockTypeInputRule(/^(#{1,3})\s$/, schema.nodes.heading, (m) => ({ level: m[1].length + 1 })),
					wrappingInputRule(/^\s*[-*]\s$/, schema.nodes.bullet_list),
					wrappingInputRule(/^(\d+)\.\s$/, schema.nodes.ordered_list, (m) => ({ order: +m[1] })),
					wrappingInputRule(/^\s*>\s$/, schema.nodes.blockquote)
				]
			}),
			keymap({
				'Mod-z': undo,
				'Mod-y': redo,
				'Shift-Mod-z': redo,
				'Mod-b': toggleMark(schema.marks.strong),
				'Mod-i': toggleMark(schema.marks.em),
				'Mod-`': toggleMark(schema.marks.code),
				// Handled here rather than left to the browser, so the editor's
				// selection moves at once and a key pressed right after acts there.
				'Mod-Home': (state, dispatch) => (dispatch?.(state.tr.setSelection(Selection.atStart(state.doc)).scrollIntoView()), true),
				'Mod-End': (state, dispatch) => (dispatch?.(state.tr.setSelection(Selection.atEnd(state.doc)).scrollIntoView()), true),
				Enter: splitListItem(schema.nodes.list_item),
				Tab: sinkListItem(schema.nodes.list_item),
				'Shift-Tab': liftListItem(schema.nodes.list_item)
			}),
			keymap(baseKeymap)
		]
	});

	const view: EditorView = new EditorView(place, {
		state,
		attributes: { class: 'prose', 'aria-label': on.label, 'aria-multiline': 'true', role: 'textbox' },
		nodeViews: {
			cite(node, view, getPos) {
				const dom = document.createElement('sup');
				dom.className = 'ref';
				dom.dataset.key = key(node.attrs.label, node.attrs.quote);
				const button = document.createElement('button');
				button.type = 'button';
				button.className = 'mark';
				button.textContent = letterOf(node.attrs.label);
				button.title = `“${node.attrs.quote}”`;
				button.addEventListener('mousedown', (e) => e.preventDefault());
				button.addEventListener('click', () => {
					const pos = getPos();
					const run = pos === undefined ? [node] : citeRun(view.state.doc, pos);
					const citations = run.map(lookup).filter((c): c is Citation => c !== undefined);
					if (citations.length) on.open(citations, button);
				});
				dom.append(button);
				const citation = lookup(node);
				dom.classList.toggle('unverified', citation !== undefined && !citation.verified);
				return { dom, ignoreMutation: () => true };
			}
		},
		dispatchTransaction(tr) {
			view.updateState(view.state.apply(tr));
			if (tr.docChanged) on.change();
		}
	});

	return {
		view,
		markdown: () => serializer.serialize(view.state.doc),
		check(next) {
			byKey = new Map(next.map((c) => [key(c.label, c.quote), c]));
			for (const sup of view.dom.querySelectorAll<HTMLElement>('sup.ref')) {
				const citation = byKey.get(sup.dataset.key ?? '');
				sup.classList.toggle('unverified', citation !== undefined && !citation.verified);
			}
		},
		run(command) {
			command(view.state, view.dispatch);
			view.focus();
		},
		destroy: () => view.destroy()
	};
}
