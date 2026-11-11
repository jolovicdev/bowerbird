<script lang="ts">
	// The optical-size builds: large sizes get the high-contrast display cut.
	import '@fontsource-variable/literata/opsz.css';
	import '@fontsource-variable/literata/opsz-italic.css';
	import '../app.css';
	import { onMount } from 'svelte';
	import { replaceState } from '$app/navigation';
	import { SIGN_IN_EVENT, signInNeeded, takeTokenFromUrl } from '#lib/api.ts';
	import SignIn from '#lib/components/SignIn.svelte';

	let { children } = $props();
	let signingIn = $state(false);

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
		const askForPassword = () => (signingIn = true);
		if (signInNeeded()) askForPassword();
		addEventListener(SIGN_IN_EVENT, askForPassword);
		// Links that open outside the app go to Android's browser in the phone app.
		const android = window.BowerbirdAndroid;
		const openOutside = (event: MouseEvent) => {
			const link = (event.target as Element).closest?.('a[target="_blank"]');
			if (!android || !(link instanceof HTMLAnchorElement)) return;
			event.preventDefault();
			android.openLink(link.href);
		};
		addEventListener('click', openOutside);
		return () => {
			removeEventListener('hashchange', tokenFromHash);
			removeEventListener(SIGN_IN_EVENT, askForPassword);
			removeEventListener('click', openOutside);
		};
	});
</script>

{#if signingIn}<SignIn />{:else}{@render children()}{/if}
