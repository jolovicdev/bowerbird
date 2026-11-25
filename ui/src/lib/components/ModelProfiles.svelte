<script lang="ts">
	import { api, describe, type ModelProfile, type Settings, type Thinking } from '#lib/api.ts';

	let {
		settings,
		model,
		thinking,
		onsaved,
		onuse
	}: {
		settings: Settings;
		/** The model and thinking level in the form above, which a new profile saves. */
		model: string;
		thinking: Thinking;
		onsaved: (settings: Settings) => void;
		onuse: (profile: ModelProfile) => void;
	} = $props();

	let name = $state('');
	let error = $state<string | null>(null);

	async function add(event: SubmitEvent) {
		event.preventDefault();
		error = null;
		const cleanName = name.trim();
		if (!cleanName) return;
		// A profile saved under a name already in use takes that profile's place.
		const next: ModelProfile[] = [
			...settings.profiles.filter((p) => p.name.toLowerCase() !== cleanName.toLowerCase()),
			{ name: cleanName, model: model.trim() || settings.default_model, thinking }
		];
		try {
			onsaved(await api.saveSettings({ profiles: next }));
			name = '';
		} catch (e) {
			error = describe(e);
		}
	}

	async function remove(profile: string) {
		error = null;
		try {
			onsaved(await api.saveSettings({ profiles: settings.profiles.filter((p) => p.name !== profile) }));
		} catch (e) {
			error = describe(e);
		}
	}
</script>

<div class="profiles">
	<h3>Saved profiles</h3>
	<p class="hint">Save model &amp; thinking presets to switch in one click here or inside any notebook.</p>
	{#if settings.profiles.length > 0}
		<ul class="providers">
			{#each settings.profiles as profile (profile.name)}
				<li>
					<div class="row">
						<span class="name">{profile.name}</span>
						<span class="status">
							<code>{profile.model}</code>{#if profile.thinking !== 'default'}, thinking: {profile.thinking}{/if}
						</span>
						<span class="row-actions">
							<button type="button" class="action" onclick={() => onuse(profile)}>Use</button>
							<button type="button" class="action quiet" onclick={() => remove(profile.name)}>Remove</button>
						</span>
					</div>
				</li>
			{/each}
		</ul>
	{/if}
	<form class="inline profile-add" onsubmit={add}>
		<label class="visually-hidden" for="profile-name">Profile name</label>
		<input
			id="profile-name"
			bind:value={name}
			placeholder="Save current model + thinking as… (e.g. Fast, Deep reasoning)"
		/>
		<button class="primary" disabled={!name.trim()}>Save profile</button>
	</form>
	{#if error}<p class="problem" role="alert">{error}</p>{/if}
</div>

<style>
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
	.hint + form {
		margin-top: 1rem;
	}
	code {
		font-family: var(--mono);
	}
	.problem {
		margin: 0.5rem 0 0;
		color: var(--danger);
	}
	.providers {
		margin: 1.25rem 0 0;
		padding: 0;
		list-style: none;
		border-bottom: 1px solid var(--line);
	}
	.providers li {
		padding: 0.75rem 0;
		border-top: 1px solid var(--line);
	}
	.row {
		display: flex;
		gap: 1rem;
		align-items: baseline;
	}
	.name {
		font-weight: 600;
	}
	.status {
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.row-actions {
		display: flex;
		gap: 1rem;
		margin-left: auto;
	}
	.action {
		padding: 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font-size: var(--t-xs);
		font-weight: 600;
		cursor: pointer;
	}
	.action.quiet {
		color: var(--ink-2);
		font-weight: 400;
	}
	.profiles {
		margin-top: 1.5rem;
		padding-top: 1.25rem;
		border-top: 1px solid var(--line);
	}
	h3 {
		margin: 0;
		font-size: var(--t-m);
		font-style: italic;
		font-weight: 400;
	}
	.profile-add {
		margin-top: 0.85rem;
	}
</style>
