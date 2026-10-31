<script lang="ts">
	import type { Citation, Piece, Source } from '#lib/api.ts';
	import { citeRuns, joinPieces, parse } from '#lib/markdown.ts';
	import Mark, { clip } from './Mark.svelte';
	import Rich from './Rich.svelte';

	let {
		pieces,
		citations,
		sources,
		active,
		apparatus = true,
		onopen
	}: {
		pieces: Piece[];
		citations: Citation[];
		sources: Map<string, Source>;
		active: Citation[] | null;
		/** List the quotes after the text on screens without a margin. */
		apparatus?: boolean;
		onopen: (citations: Citation[], el: HTMLElement) => void;
	} = $props();

	const joined = $derived(joinPieces(pieces, citations));
	const blocks = $derived(parse(joined.text));
	// The margin holds at most two quotes per paragraph or list item, so dense
	// answers don't stack notes far below the lines that cite them.
	const noted = $derived(new Set(citeRuns(blocks).flatMap((run) => run.slice(0, 2))));
	const groupCitations = (group: number) => joined.groups[group].map((i) => citations[i]);
	const groupPage = (group: Citation[]) => group.find((c) => c.page !== null)?.page;
</script>

<div class="answer">
	<Rich {blocks}>
		{#snippet cite(group: number)}
			{@const cited = groupCitations(group)}
			{@const source = cited[0].source_id ? sources.get(cited[0].source_id) : undefined}
			<Mark
				citations={cited}
				title={source?.title ?? 'a removed source'}
				active={active !== null && active[0] === cited[0]}
				note={noted.has(group)}
				onopen={(el) => onopen(cited, el)}
			/>
		{/snippet}
	</Rich>
</div>
{#if apparatus && joined.groups.length}
	<!-- On narrow screens there is no margin; the quotes follow the text,
	     as the apparatus does under the text of a printed edition. -->
	<ol class="apparatus" aria-label="Quotes in this text">
		{#each joined.groups as _, group (group)}
			{@const cited = groupCitations(group)}
			{@const page = groupPage(cited)}
			<li>
				<button class="entry" onclick={(e) => onopen(cited, e.currentTarget)}>
					<span class="siglum">{cited[0].letter}{#if !cited.every((c) => c.verified)}†{/if}</span>
					{#if page}<span class="loc">p. {page}</span>{/if}
					“{clip(cited[0].quote, 120)}”
				</button>
			</li>
		{/each}
	</ol>
{/if}

<style>
	.answer {
		font-size: var(--t-m);
		line-height: 1.6;
	}
	.answer:empty {
		display: none;
	}
	.apparatus {
		display: none;
		margin: 1.25rem 0 0;
		padding: 0.75rem 0 0;
		border-top: 1px solid var(--line);
		list-style: none;
	}
	.entry {
		display: block;
		width: 100%;
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--ink);
		font-size: var(--t-s);
		line-height: 1.45;
		text-align: left;
		cursor: pointer;
	}
	.siglum {
		margin-right: 0.3em;
		color: var(--rubric);
		font-style: italic;
		font-weight: 600;
	}
	.loc {
		margin-right: 0.3em;
		font-style: italic;
	}
	@media (max-width: 959px) {
		.apparatus {
			display: block;
		}
	}
</style>
