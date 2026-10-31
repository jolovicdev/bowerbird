<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, describe, type MapNode, type StudySet } from '#lib/api.ts';

	const slug = $derived(page.params.slug ?? '');
	const id = $derived(page.params.id ?? '');

	let set = $state<StudySet | null>(null);
	let notebookTitle = $state('');
	let error = $state<string | null>(null);
	/** Paths of topics whose subtopics are shown, as "0/2/1". The root and its themes start open. */
	let open = $state(new Set<string>(['']));
	/** Path of the selected topic; the root's path is the empty string. */
	let selected = $state<string | null>(null);

	onMount(async () => {
		try {
			const [loaded, nb] = await Promise.all([api.study(slug, id), api.notebook(slug)]);
			set = loaded;
			notebookTitle = nb.title;
		} catch (e) {
			error = describe(e);
		}
	});

	// Layout: a tree read left to right. Each visible leaf takes a row; a
	// parent sits level with the middle of its children.
	const ROW = 34;
	const GAP = 56;
	const FONT = '15px "Literata Variable", Georgia, serif';
	const ROOT_FONT = 'italic 600 17px "Literata Variable", Georgia, serif';
	let measure: CanvasRenderingContext2D | null = null;
	const width = (text: string, depth: number) => {
		measure ??= document.createElement('canvas').getContext('2d');
		if (!measure) return text.length * 8;
		measure.font = depth ? FONT : ROOT_FONT;
		return measure.measureText(text).width;
	};

	type Placed = { node: MapNode; path: string; depth: number; x: number; y: number; w: number; parent: Placed | null };

	const layout = $derived.by(() => {
		if (!set?.map) return { placed: [] as Placed[], height: 0, total: 0 };
		const columns: number[] = [];
		const visit = (node: MapNode, path: string, depth: number) => {
			columns[depth] = Math.max(columns[depth] ?? 0, width(node.title, depth) + 24);
			if (open.has(path)) node.children?.forEach((c, i) => visit(c, path ? `${path}/${i}` : `${i}`, depth + 1));
		};
		visit(set.map, '', 0);
		const lefts = columns.reduce<number[]>((acc, w, i) => [...acc, i ? acc[i - 1] + columns[i - 1] + GAP : 0], []);
		const placed: Placed[] = [];
		let row = 0;
		const place = (node: MapNode, path: string, depth: number, parent: Placed | null): Placed => {
			const me: Placed = { node, path, depth, x: lefts[depth], y: 0, w: width(node.title, depth) + 24, parent };
			placed.push(me);
			const kids = open.has(path) ? (node.children ?? []) : [];
			if (kids.length) {
				const ys = kids.map((c, i) => place(c, path ? `${path}/${i}` : `${i}`, depth + 1, me).y);
				me.y = (ys[0] + ys[ys.length - 1]) / 2;
			} else {
				me.y = row++ * ROW + ROW / 2;
			}
			return me;
		};
		place(set.map, '', 0, null);
		return { placed, height: row * ROW, total: lefts.at(-1)! + columns.at(-1)! };
	});

	function toggle(p: Placed) {
		selected = p.path;
		if (!p.node.children?.length) return;
		// The root stays open; closing a topic closes everything under it.
		const next = new Set(open);
		if (next.has(p.path) && p.path !== '') {
			for (const key of [...next]) if (key === p.path || key.startsWith(`${p.path}/`)) next.delete(key);
		} else {
			next.add(p.path);
		}
		open = next;
	}

	function openAll() {
		const all = new Set<string>();
		const walk = (n: MapNode, path: string) => {
			all.add(path);
			n.children?.forEach((c, i) => walk(c, path ? `${path}/${i}` : `${i}`));
		};
		if (set?.map) walk(set.map, '');
		open = all;
	}

	const selectedNode = $derived(layout.placed.find((p) => p.path === selected) ?? null);
	const nodeAt = (path: string) => layout.placed.find((p) => p.path === path);
	const trail = (p: Placed | null): string[] => (p ? [...trail(p.parent), p.node.title] : []);

	function askAbout(p: Placed) {
		const where = trail(p.parent).slice(1);
		const question = where.length
			? `What do the sources say about ${p.node.title}, in the context of ${where.join(' and ')}?`
			: `What do the sources say about ${p.node.title}?`;
		goto(`/n/${encodeURIComponent(slug)}?ask=${encodeURIComponent(question)}`);
	}

	let svg = $state<SVGSVGElement>();
	function exportSvg() {
		if (!svg || !set) return;
		const copy = svg.cloneNode(true) as SVGSVGElement;
		copy.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
		const style = document.createElementNS('http://www.w3.org/2000/svg', 'style');
		style.textContent = `text{font:15px Georgia,serif;fill:#1f1f1d}.root text{font-size:17px;font-style:italic}path{fill:none;stroke:#8f908a;stroke-width:1}.count{fill:#a3161f;font-size:12px}`;
		copy.prepend(style);
		const link = document.createElement('a');
		link.href = URL.createObjectURL(new Blob([copy.outerHTML], { type: 'image/svg+xml' }));
		link.download = `${set.title}.svg`;
		link.click();
		URL.revokeObjectURL(link.href);
	}
</script>

{#snippet branch(node: MapNode, path: string)}
	{@const kids = node.children?.length ?? 0}
	{@const placed = nodeAt(path)}
	<li role="treeitem" aria-selected={path === selected} aria-expanded={kids ? open.has(path) : undefined}>
		<button class="topic" class:selected={path === selected} class:root={path === ''} onclick={() => placed && toggle(placed)}>
			{#if kids}<span class="twisty" aria-hidden="true">{open.has(path) ? '–' : '+'}</span>{/if}
			{node.title}
			{#if kids && !open.has(path)}<span class="more">{kids}</span>{/if}
		</button>
		{#if kids && open.has(path)}
			<ul role="group">
				{#each node.children ?? [] as child, i (i)}
					{@render branch(child, path ? `${path}/${i}` : `${i}`)}
				{/each}
			</ul>
		{/if}
	</li>
{/snippet}

<svelte:head><title>{set ? `${set.title} · Bowerbird` : 'Bowerbird'}</title></svelte:head>

<div class="page">
	<nav class="head">
		<a class="back" href="/n/{encodeURIComponent(slug)}">{notebookTitle || 'Notebook'}</a>
		{#if set}
			<span class="tools">
				<button class="action" onclick={openAll}>Open every topic</button>
				<button class="action" onclick={exportSvg}>Export as SVG</button>
			</span>
		{/if}
	</nav>

	{#if error}
		<p class="problem">{error}</p>
	{:else if !set}
		<p class="lede">Opening the mind map…</p>
	{:else}
		<h1>{set.title}</h1>
		<p class="lede">Select a topic to open it, then ask the sources about it.</p>
		<!-- Phones get the same map as an outline, read top to bottom. -->
		<ul class="outline" role="tree" aria-label={set.title}>
			{@render branch(set.map!, '')}
		</ul>
		<div class="canvas">
			<svg bind:this={svg} width={layout.total + 8} height={layout.height + 8} role="tree" aria-label={set.title}>
				<g transform="translate(4 4)">
					{#each layout.placed as p (p.path)}
						{#if p.parent}
							{@const from = { x: p.parent.x + p.parent.w - 8, y: p.parent.y }}
							<path d="M {from.x} {from.y} C {from.x + GAP / 2} {from.y}, {p.x - GAP / 2} {p.y}, {p.x} {p.y}" />
						{/if}
					{/each}
					{#each layout.placed as p (p.path)}
						{@const kids = p.node.children?.length ?? 0}
						<g
							class="node"
							class:root={p.depth === 0}
							class:selected={p.path === selected}
							transform="translate({p.x} {p.y})"
							role="treeitem"
							aria-level={p.depth + 1}
							aria-selected={p.path === selected}
							aria-expanded={kids ? open.has(p.path) : undefined}
							tabindex="0"
							onclick={() => toggle(p)}
							onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), toggle(p))}
						>
							<rect x="-6" y="-14" width={p.w} height="28" rx="3" />
							<text x="4" dy="0.35em">{p.node.title}</text>
							{#if kids && !open.has(p.path)}<text class="count" x={p.w - 14} dy="0.35em">+{kids}</text>{/if}
						</g>
					{/each}
				</g>
			</svg>
		</div>
		{#if selectedNode}
			<div class="selection" role="status">
				<span class="trail">
					<span class="long">{trail(selectedNode).slice(1).slice(-2).join(' › ') || selectedNode.node.title}</span>
					<span class="short">{selectedNode.node.title}</span>
				</span>
				<button class="primary" onclick={() => askAbout(selectedNode)}>Ask about this</button>
			</div>
		{/if}
	{/if}
</div>

<style>
	.page {
		min-height: 100dvh;
		padding: 0 2rem 7rem;
	}
	.head {
		display: flex;
		gap: 1.5rem;
		align-items: baseline;
		padding: 1rem 0 0.7rem;
		border-bottom: 1px solid var(--line);
		font-size: var(--t-xs);
		font-style: italic;
	}
	.back {
		padding: 0.3rem 0;
		color: var(--ink-2);
		text-decoration: none;
	}
	.back::before {
		content: '‹ ';
		font-style: normal;
	}
	.tools {
		display: flex;
		gap: 1.25rem;
		margin-left: auto;
	}
	.action {
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: inherit;
		font-style: normal;
		font-weight: 600;
		cursor: pointer;
	}
	h1 {
		margin: 3rem 0 0.5rem;
		font-size: var(--t-xxl);
		font-weight: 500;
		font-style: italic;
		font-variation-settings: 'opsz' 60;
	}
	.lede {
		margin: 0 0 2rem;
		color: var(--ink-2);
		font-style: italic;
	}
	.problem {
		margin-top: 3rem;
		color: var(--danger);
	}
	.canvas {
		overflow-x: auto;
		padding-bottom: 1rem;
	}
	svg {
		display: block;
		font: 15px var(--serif);
	}
	path {
		fill: none;
		stroke: var(--edge);
		stroke-width: 1;
	}
	.node {
		cursor: pointer;
		outline: none;
	}
	.node rect {
		fill: transparent;
		stroke: none;
	}
	.node text {
		fill: var(--ink);
	}
	.node:hover text {
		fill: var(--rubric);
	}
	.node.selected rect {
		fill: var(--highlight);
		stroke: var(--rubric);
		stroke-width: 1;
	}
	.node:focus-visible rect {
		stroke: var(--accent);
		stroke-width: 2;
	}
	.root text {
		font-size: 17px;
		font-style: italic;
		font-weight: 600;
	}
	.count {
		fill: var(--rubric) !important;
		font-size: 12px;
		font-style: italic;
	}
	.selection {
		position: fixed;
		right: 0;
		bottom: 0;
		left: 0;
		display: flex;
		gap: 1rem;
		align-items: center;
		justify-content: space-between;
		padding: 0.85rem 2rem calc(0.85rem + env(safe-area-inset-bottom));
		border-top: 1px solid var(--line);
		background: var(--raised);
	}
	.trail {
		min-width: 0;
		overflow: hidden;
		font-style: italic;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.primary {
		flex: none;
		padding: calc(0.5rem - var(--cap-lift)) 1rem calc(0.5rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		font-weight: 600;
		cursor: pointer;
	}
	.outline,
	.short {
		display: none;
	}
	@media (max-width: 719px) {
		.page {
			padding-inline: 1rem;
		}
		.canvas,
		.long {
			display: none;
		}
		.short {
			display: inline;
		}
		.outline {
			display: block;
			margin: 0;
			padding: 0;
			list-style: none;
		}
		.outline :global(ul) {
			margin: 0 0 0 0.6rem;
			padding: 0 0 0 0.75rem;
			border-left: 1px solid var(--line);
			list-style: none;
		}
		.topic {
			display: flex;
			gap: 0.5rem;
			align-items: baseline;
			width: 100%;
			min-height: 2.75rem;
			padding: 0.5rem 0.4rem;
			border: 0;
			border-radius: 3px;
			background: none;
			color: var(--ink);
			font-size: var(--t-m);
			line-height: 1.35;
			text-align: left;
			cursor: pointer;
		}
		.topic.root {
			font-style: italic;
			font-weight: 600;
		}
		.topic.selected {
			background: var(--highlight);
			color: var(--rubric);
		}
		.twisty {
			flex: none;
			width: 0.8rem;
			color: var(--rubric);
		}
		.more {
			margin-left: auto;
			color: var(--ink-2);
			font-size: var(--t-xs);
			font-style: italic;
		}
		.selection {
			padding-inline: 1rem;
		}
	}
</style>
