<script lang="ts" module>
	/** A quote shortened at a word boundary for the margin. */
	export function clip(quote: string, max = 90): string {
		if (quote.length <= max) return quote;
		const cut = quote.slice(0, max);
		return `${cut.slice(0, cut.lastIndexOf(' ') > 40 ? cut.lastIndexOf(' ') : max)}…`;
	}
</script>

<script lang="ts">
	import type { Citation } from '#lib/api.ts';

	let {
		citations,
		title,
		active = false,
		note = false,
		onopen
	}: {
		citations: Citation[];
		title: string;
		active?: boolean;
		/** Also set the quote in the margin beside the line. */
		note?: boolean;
		onopen: (el: HTMLElement) => void;
	} = $props();

	const verified = $derived(citations.every((c) => c.verified));
	const pages = $derived([...new Set(citations.map((c) => c.page).filter((p) => p !== null))]);
	const where = $derived(pages.length === 1 ? `p. ${pages[0]}` : pages.length > 1 ? `pp. ${pages.join(', ')}` : '');
	const label = $derived(
		`Source ${citations[0].letter}, ${title}` +
			(pages.length === 1 ? `, page ${pages[0]}` : pages.length > 1 ? `, pages ${pages.join(' and ')}` : '') +
			'. ' +
			(verified
				? citations.length === 1
					? 'Quote found in the source.'
					: `All ${citations.length} quotes found in the source.`
				: 'A quote was not found in the source.')
	);

	let button: HTMLButtonElement;
</script>

<sup class="ref"
	><button
		bind:this={button}
		class="mark"
		class:unverified={!verified}
		class:active
		aria-label={label}
		title={label}
		onclick={(e) => onopen(e.currentTarget)}>{citations[0].letter}{#if !verified}†{/if}</button
	></sup
>{#if note}<button
		class="note"
		class:unverified={!verified}
		class:active
		tabindex="-1"
		aria-hidden="true"
		onclick={() => onopen(button)}
		><span class="siglum">{citations[0].letter}{#if !verified}†{/if}</span
		>{#if !verified}<span class="doubt">Not found in the source.</span>{' '}{/if}“{clip(citations[0].quote)}”{#if citations.length > 1}{' '}<span
				class="more">and {citations.length - 1} more</span
			>{/if}{#if where}{' '}<span class="loc">{where}</span>{/if}</button
	>{/if}

<style>
	/* Line height 0 keeps superscripts from pushing lines apart. */
	.ref {
		font-size: 0.72em;
		line-height: 0;
		vertical-align: super;
	}
	.mark {
		position: relative;
		padding: 0 0.12em;
		border: 0;
		border-radius: 2px;
		background: none;
		color: var(--rubric);
		font: italic 600 1em/1 var(--serif);
		cursor: pointer;
	}
	/* Hit area beyond the letter: tall for fingers, but barely wider, so
	   letters set side by side ("A C") each keep their own half. */
	.mark::after {
		content: '';
		position: absolute;
		inset: -7px -2px;
	}
	@media (pointer: coarse) {
		.mark::after {
			inset: -14px -3px;
		}
	}
	.mark:hover,
	.mark.active {
		background: var(--highlight);
		box-shadow: inset 0 -1px 0 var(--rubric);
	}
	.mark.unverified {
		color: var(--ink-2);
	}

	/* The quote in the margin, floated out of the text column so it sits on
	   the line that cites it. Floats stack, so notes never overlap. */
	.note {
		float: right;
		clear: right;
		width: var(--margin);
		margin: 0.2rem calc(-1 * (var(--margin) + var(--margin-gap))) 0.75rem 0;
		padding: 0 0 0 0.6rem;
		border: 0;
		border-left: 1px solid var(--line);
		background: none;
		color: var(--ink);
		font: 400 var(--t-s) / 1.45 var(--serif);
		text-align: left;
		white-space: normal;
		hyphens: manual;
		cursor: pointer;
	}
	.note:hover,
	.note.active {
		border-left-color: var(--rubric);
		color: var(--ink);
	}
	.siglum {
		margin-right: 0.35em;
		color: var(--rubric);
		font-style: italic;
		font-weight: 600;
	}
	.note.unverified .siglum,
	.doubt {
		color: var(--ink-2);
		font-style: italic;
	}
	.loc,
	.more {
		font-style: italic;
		white-space: nowrap;
	}
	@media (max-width: 959px) {
		.note {
			display: none;
		}
	}
</style>
