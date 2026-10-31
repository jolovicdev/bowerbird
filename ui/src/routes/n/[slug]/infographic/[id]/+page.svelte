<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { api, describe, type Citation, type Fact, type Source, type StudySet } from '#lib/api.ts';
	import italicFont from '@fontsource-variable/literata/files/literata-latin-opsz-italic.woff2?url';
	import normalFont from '@fontsource-variable/literata/files/literata-latin-opsz-normal.woff2?url';

	const slug = $derived(page.params.slug ?? '');
	const id = $derived(page.params.id ?? '');

	let set = $state<StudySet | null>(null);
	let sources = $state<Source[]>([]);
	let notebookTitle = $state('');
	let error = $state<string | null>(null);
	let svg = $state<SVGSVGElement>();

	onMount(async () => {
		try {
			const [loaded, nb] = await Promise.all([api.study(slug, id), api.notebook(slug)]);
			await document.fonts.ready;
			set = loaded;
			notebookTitle = nb.title;
			sources = nb.sources;
		} catch (e) {
			error = describe(e);
		}
	});

	// A poster 1080 px wide, laid out top to bottom. Text is measured with the
	// same font the page shows, so lines wrap where they will be drawn.
	const W = 1080;
	const PAD = 72;
	const FAMILY = '"Literata Variable", Georgia, serif';
	const INK = '#1f1f1d';
	const INK2 = '#5f5e59';
	const RUBRIC = '#a3161f';
	const LINE = '#d3d4ce';

	type Text = { x: number; y: number; text: string; font: string; fill: string; anchor?: 'start' | 'middle'; cite?: string };
	type Shape = { kind: 'line'; x1: number; y1: number; x2: number; y2: number; stroke: string } | { kind: 'dot'; cx: number; cy: number };

	let measure: CanvasRenderingContext2D | null = null;
	function wrap(text: string, font: string, width: number): string[] {
		measure ??= document.createElement('canvas').getContext('2d');
		if (!measure) return [text];
		measure.font = font;
		const lines: string[] = [];
		let line = '';
		for (const word of text.split(/\s+/)) {
			const next = line ? `${line} ${word}` : word;
			if (line && measure.measureText(next).width > width) {
				lines.push(line);
				line = word;
			} else line = next;
		}
		if (line) lines.push(line);
		return lines;
	}

	/** Whether the text wraps to at most `lines` lines with no word wider than `width`. */
	function fits(text: string, font: string, width: number, lines: number): boolean {
		const wrapped = wrap(text, font, width);
		return wrapped.length <= lines && wrapped.every((line) => !measure || measure.measureText(line).width <= width);
	}

	const layout = $derived.by(() => {
		const texts: Text[] = [];
		const shapes: Shape[] = [];
		const info = set?.infographic;
		if (!set || !info) return { texts, shapes, height: 0 };
		let y = PAD + 8;
		const add = (lines: string[], x: number, size: number, font: string, fill: string, gap: number, anchor?: 'middle', cite = '') => {
			lines.forEach((text, i) => {
				const last = cite && i === lines.length - 1;
				y += size;
				texts.push({ x, y, text: last ? text.slice(0, -cite.length).trimEnd() : text, font, fill, anchor, cite: last ? cite : undefined });
				y += gap;
			});
		};
		// The siglum wraps with the text so it never overruns the column, then draws in rubric.
		const fact = (text: string, c: Citation | null, x: number, font: string, fill: string, width: number, anchor?: 'middle') => {
			const siglum = c ? `${c.letter}${c.verified ? '' : '†'}` : '';
			add(wrap(siglum ? `${text} ${siglum}` : text, font, width), x, 22, font, fill, 8, anchor, siglum);
		};
		const titleFont = `italic 500 60px ${FAMILY}`;
		add(wrap(set.title, titleFont, W - 2 * PAD), PAD, 60, titleFont, INK, 10);
		if (info.subtitle) {
			const font = `400 26px ${FAMILY}`;
			y += 8;
			add(wrap(info.subtitle, font, W - 2 * PAD), PAD, 26, font, INK2, 10);
		}
		y += 24;
		shapes.push({ kind: 'line', x1: PAD, y1: y, x2: W - PAD, y2: y, stroke: RUBRIC });
		y += 36;

		if (info.figures.length) {
			const columns = info.figures.length;
			const col = (W - 2 * PAD) / columns;
			const top = y;
			let bottom = y;
			// One size for every figure: the largest at which each fits its
			// column in two lines without a word running into the next column.
			const valueFont = (px: number) => `italic 600 ${px}px ${FAMILY}`;
			let size = 72;
			while (size > 32 && !info.figures.every((f) => fits(f.head, valueFont(size), col - 24, 2))) size -= 4;
			info.figures.forEach((f, i) => {
				const cx = PAD + col * i + col / 2;
				y = top;
				add(wrap(f.head, valueFont(size), col - 24), cx, size, valueFont(size), RUBRIC, 8, 'middle');
				const labelFont = `400 22px ${FAMILY}`;
				y += 6;
				fact(f.text, f.citation, cx, labelFont, INK2, col - 32, 'middle');
				bottom = Math.max(bottom, y);
			});
			y = bottom + 40;
		}

		const heading = (label: string) => {
			const font = `italic 400 26px ${FAMILY}`;
			add([label], PAD, 26, font, INK2, 18);
		};

		if (info.timeline.length) {
			heading('Timeline');
			const lineX = PAD + 8;
			const start = y;
			for (const event of info.timeline) {
				const dotY = y + 12;
				shapes.push({ kind: 'dot', cx: lineX, cy: dotY });
				const dateFont = `600 24px ${FAMILY}`;
				add([event.head], PAD + 36, 24, dateFont, INK, 6);
				const textFont = `400 22px ${FAMILY}`;
				fact(event.text, event.citation, PAD + 36, textFont, INK, W - 2 * PAD - 36);
				y += 16;
			}
			shapes.unshift({ kind: 'line', x1: lineX, y1: start + 12, x2: lineX, y2: y - 28, stroke: LINE });
			y += 20;
		}

		if (info.points.length) {
			heading('What to remember');
			const col = (W - 2 * PAD - 40) / 2;
			for (let i = 0; i < info.points.length; i += 2) {
				const top = y;
				let bottom = y;
				for (const [j, point] of info.points.slice(i, i + 2).entries()) {
					y = top;
					const x = PAD + j * (col + 40);
					const titleFont = `600 24px ${FAMILY}`;
					add(wrap(point.head, titleFont, col), x, 24, titleFont, INK, 6);
					const textFont = `400 22px ${FAMILY}`;
					fact(point.text, point.citation, x, textFont, INK2, col);
					bottom = Math.max(bottom, y);
				}
				y = bottom + 28;
			}
		}

		const cited = new Set(
			[...info.figures, ...info.timeline, ...info.points].flatMap((f: Fact) => (f.citation?.letter ? [f.citation.letter] : []))
		);
		const list = sources.filter((s) => cited.has(s.letter)).map((s) => `${s.letter}. ${s.title}`);
		y += 8;
		shapes.push({ kind: 'line', x1: PAD, y1: y, x2: W - PAD, y2: y, stroke: LINE });
		y += 12;
		if (list.length) {
			const font = `400 17px ${FAMILY}`;
			add(wrap(`Sources: ${list.join('; ')}`, font, W - 2 * PAD), PAD, 17, font, INK2, 6);
		}
		return { texts, shapes, height: y + PAD };
	});

	/** The poster as a standalone SVG file, with the font embedded so it looks the same anywhere. */
	async function standalone(): Promise<string> {
		const embed = async (url: string) => {
			const bytes = new Uint8Array(await (await fetch(url)).arrayBuffer());
			let binary = '';
			for (const b of bytes) binary += String.fromCharCode(b);
			return `data:font/woff2;base64,${btoa(binary)}`;
		};
		const [normal, italic] = await Promise.all([embed(normalFont), embed(italicFont)]);
		const copy = svg!.cloneNode(true) as SVGSVGElement;
		copy.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
		const style = document.createElementNS('http://www.w3.org/2000/svg', 'style');
		style.textContent =
			`@font-face{font-family:"Literata Variable";font-style:normal;font-weight:200 900;src:url(${normal}) format("woff2")}` +
			`@font-face{font-family:"Literata Variable";font-style:italic;font-weight:200 900;src:url(${italic}) format("woff2")}`;
		copy.prepend(style);
		return copy.outerHTML;
	}

	function download(blob: Blob, name: string) {
		const link = document.createElement('a');
		link.href = URL.createObjectURL(blob);
		link.download = name;
		link.click();
		URL.revokeObjectURL(link.href);
	}

	async function exportSvg() {
		if (!set) return;
		download(new Blob([await standalone()], { type: 'image/svg+xml' }), `${set.title}.svg`);
	}

	async function exportPng() {
		if (!set) return;
		const image = new Image();
		image.src = URL.createObjectURL(new Blob([await standalone()], { type: 'image/svg+xml' }));
		await image.decode();
		const canvas = document.createElement('canvas');
		canvas.width = W * 2;
		canvas.height = layout.height * 2;
		const context = canvas.getContext('2d')!;
		context.scale(2, 2);
		context.drawImage(image, 0, 0);
		URL.revokeObjectURL(image.src);
		canvas.toBlob((blob) => blob && download(blob, `${set!.title}.png`), 'image/png');
	}
</script>

<svelte:head><title>{set ? `${set.title} · Bowerbird` : 'Bowerbird'}</title></svelte:head>

<div class="page">
	<nav class="head">
		<a class="back" href="/n/{encodeURIComponent(slug)}">{notebookTitle || 'Notebook'}</a>
		{#if set}
			<span class="tools">
				<button class="action" onclick={exportPng}>Download PNG</button>
				<button class="action" onclick={exportSvg}>Download SVG</button>
			</span>
		{/if}
	</nav>
	{#if error}
		<p class="problem">{error}</p>
	{:else if !set}
		<p class="lede">Opening the infographic…</p>
	{:else}
		<div class="poster">
			<svg
				bind:this={svg}
				viewBox="0 0 {W} {layout.height}"
				width={W}
				height={layout.height}
				role="img"
				aria-label="{set.title}. {set.infographic?.subtitle ?? ''}"
			>
				<rect width={W} height={layout.height} fill="#fafaf8" />
				{#each layout.shapes as shape, i (i)}
					{#if shape.kind === 'line'}
						<line x1={shape.x1} y1={shape.y1} x2={shape.x2} y2={shape.y2} stroke={shape.stroke} stroke-width="1.5" />
					{:else}
						<circle cx={shape.cx} cy={shape.cy} r="7" fill={RUBRIC} />
					{/if}
				{/each}
				{#each layout.texts as t, i (i)}
					<text x={t.x} y={t.y} fill={t.fill} text-anchor={t.anchor ?? 'start'} style:font={t.font}
						>{t.text}{#if t.cite}<tspan fill={RUBRIC} font-style="italic" font-weight="600">{t.text ? ' ' : ''}{t.cite}</tspan>{/if}</text
					>
				{/each}
			</svg>
		</div>
		<p class="hint">Letters after a fact name the source it comes from; † marks a quote that wasn't found there.</p>
	{/if}
</div>

<style>
	.page {
		width: min(100% - 2.5rem, 68rem);
		margin: 0 auto;
		padding-bottom: 5rem;
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		gap: 0 1.5rem;
		align-items: baseline;
		padding: 1rem 0 0.7rem;
		border-bottom: 1px solid var(--line);
		font-size: var(--t-xs);
		font-style: italic;
	}
	.back {
		margin-right: auto;
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
	.poster {
		margin-top: 2.5rem;
	}
	.poster svg {
		display: block;
		width: 100%;
		max-width: 1080px;
		height: auto;
		margin: 0 auto;
		box-shadow: 0 0 0 1px var(--line);
	}
	.lede,
	.hint {
		color: var(--ink-2);
		font-size: var(--t-xs);
		font-style: italic;
		text-align: center;
	}
	.problem {
		margin-top: 3rem;
		color: var(--danger);
	}
</style>
