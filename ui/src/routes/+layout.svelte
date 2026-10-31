<script lang="ts">
	// The optical-size builds: large sizes get the high-contrast display cut.
	import '@fontsource-variable/literata/opsz.css';
	import '@fontsource-variable/literata/opsz-italic.css';
	import '../app.css';
	import { onMount } from 'svelte';
	import { replaceState } from '$app/navigation';
	import { takeTokenFromUrl } from '#lib/api.ts';

	let { children } = $props();

	// Runs before any page loads data, so the first request already carries the token.
	const cleanUrl = takeTokenFromUrl(new URL(location.href));
	onMount(() => {
		if (cleanUrl) replaceState(cleanUrl, {});
		// Opening the server's link in a tab that already shows Bowerbird only
		// changes the hash, so pick the token up here and reload with it.
		const tokenFromHash = () => {
			const clean = takeTokenFromUrl(new URL(location.href));
			if (clean) location.replace(clean);
		};
		addEventListener('hashchange', tokenFromHash);
		return () => removeEventListener('hashchange', tokenFromHash);
	});
</script>

{@render children()}
