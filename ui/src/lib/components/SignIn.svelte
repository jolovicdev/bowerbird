<script lang="ts">
	import { onMount } from 'svelte';
	import { ApiError, api, describe } from '#lib/api.ts';

	let password = $state('');
	let error = $state<string | null>(null);
	let busy = $state(false);
	let input: HTMLInputElement;

	onMount(() => input.focus());

	async function signIn(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = null;
		try {
			await api.signIn(password);
			location.reload();
		} catch (e) {
			// describe() reads any 401 as a missing token link.
			error = e instanceof ApiError ? e.message : describe(e);
			busy = false;
		}
	}
</script>

<svelte:head><title>Sign in · Bowerbird</title></svelte:head>

<div class="page">
	<h1>Bowerbird</h1>
	<form onsubmit={signIn}>
		<label for="password">Password</label>
		<div class="inline">
			<input
				id="password"
				type="password"
				autocomplete="current-password"
				aria-describedby="password-hint"
				bind:this={input}
				bind:value={password}
				required
			/>
			<button class="primary" disabled={busy || !password}>Sign in</button>
		</div>
		<p class="hint" id="password-hint">
			The password is the <code>BOWERBIRD_TOKEN</code> this server was started with. This browser stays signed in for
			30 days.
		</p>
		{#if error}<p class="problem" role="alert">{error}</p>{/if}
	</form>
</div>

<style>
	.page {
		width: min(100% - 2.5rem, 30rem);
		margin: 0 auto;
		padding: 18vh 0 6rem;
	}
	h1 {
		margin: 0 0 1.75rem;
		font-size: var(--t-display);
		font-weight: 500;
		font-variation-settings: 'opsz' 72;
		line-height: 1.08;
		letter-spacing: -0.015em;
	}
	label {
		display: block;
		margin-bottom: 0.4rem;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.inline {
		display: flex;
		gap: 0.5rem;
		align-items: center;
	}
	input {
		flex: 1;
		min-width: 0;
		padding: calc(0.5rem - var(--cap-lift)) 0.65rem calc(0.5rem + var(--cap-lift));
		border: 0;
		border-radius: 4px;
		background: var(--raised);
		box-shadow: inset 0 0 0 1px var(--edge);
	}
	input:focus {
		outline: 1px solid var(--ink);
		outline-offset: 0;
	}
	.primary {
		padding: calc(0.45rem - var(--cap-lift)) 1rem calc(0.45rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		font-weight: 600;
		cursor: pointer;
	}
	.primary:disabled {
		border-color: var(--line);
		background: transparent;
		color: var(--ink-2);
		cursor: default;
	}
	.hint {
		margin: 0.5rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.problem {
		margin: 0.5rem 0 0;
		color: var(--danger);
	}
</style>
