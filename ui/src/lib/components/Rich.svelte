<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { Block, Inline } from '#lib/markdown.ts';

	let { blocks, cite }: { blocks: Block[]; cite: Snippet<[number]> } = $props();
</script>

<!-- Inline branches stay on one line so the template adds no stray spaces. -->
{#snippet inlines(nodes: Inline[])}{#each nodes as node, i (i)}{#if node.kind === 'text'}{node.text}{:else if node.kind === 'strong'}<strong>{@render inlines(node.children)}</strong>{:else if node.kind === 'em'}<em>{@render inlines(node.children)}</em>{:else if node.kind === 'code'}<code>{node.text}</code>{:else if node.kind === 'link'}<a href={node.href} target="_blank" rel="noopener noreferrer">{@render inlines(node.children)}</a>{:else if node.kind === 'cite'}{@render cite(node.index)}{:else if node.kind === 'glue'}<span class="glue">{@render inlines(node.children)}</span>{/if}{/each}{/snippet}

{#each blocks as block, i (i)}
	{#if block.kind === 'p'}
		<p>{@render inlines(block.children)}</p>
	{:else if block.kind === 'h'}
		<h3>{@render inlines(block.children)}</h3>
	{:else if block.kind === 'ul'}
		<ul>
			{#each block.items as item, j (j)}<li>{@render inlines(item)}</li>{/each}
		</ul>
	{:else if block.kind === 'ol'}
		<ol>
			{#each block.items as item, j (j)}<li>{@render inlines(item)}</li>{/each}
		</ol>
	{:else if block.kind === 'quote'}
		<blockquote>{@render inlines(block.children)}</blockquote>
	{:else if block.kind === 'pre'}
		<pre><code>{block.text}</code></pre>
	{/if}
{/each}

<style>
	p,
	ul,
	ol,
	blockquote,
	pre {
		margin: 0 0 0.85em;
	}
	h3 {
		margin: 1.5em 0 0.4em;
		font-size: var(--t-m);
		font-style: italic;
		font-weight: 600;
	}
	h3:first-child {
		margin-top: 0;
	}
	ul,
	ol {
		padding-left: 1.3em;
	}
	li + li {
		margin-top: 0.35em;
	}
	blockquote {
		padding-left: 1em;
		border-left: 1px solid var(--rubric);
		color: var(--ink-2);
	}
	code {
		padding: 0.05em 0.3em;
		border-radius: 3px;
		background: var(--sunken);
		font-family: var(--mono);
		font-size: 0.85em;
	}
	pre {
		padding: 0.75rem 1rem;
		overflow-x: auto;
		border-radius: 6px;
		background: var(--sunken);
	}
	pre code {
		padding: 0;
		background: none;
	}
	.glue {
		white-space: nowrap;
	}
</style>
