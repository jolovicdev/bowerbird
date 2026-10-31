<script lang="ts">
	import type { Figure } from '#lib/api.ts';
	import { figureUrl } from '#lib/images.ts';

	let { slug, sourceId, figure }: { slug: string; sourceId: string; figure: Figure } = $props();

	let src = $state<string>();
	$effect(() => {
		let gone = false;
		figureUrl(slug, sourceId, figure.file)
			.then((url) => !gone && (src = url))
			.catch(() => {});
		return () => {
			gone = true;
		};
	});
</script>

<!-- Width and height hold the picture's place while it loads. -->
<img {src} alt={figure.caption} width={figure.width} height={figure.height} class:loading={!src} />

<style>
	img {
		display: block;
		max-width: 100%;
		height: auto;
	}
	.loading {
		background: var(--line);
	}
</style>
