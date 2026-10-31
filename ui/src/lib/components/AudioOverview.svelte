<script lang="ts">
	import { onDestroy, tick } from 'svelte';
	import { api, describe, type Citation, type StudySet } from '#lib/api.ts';

	let {
		slug,
		id,
		set,
		onopen,
		onupdate
	}: {
		slug: string;
		id: string;
		set: StudySet;
		/** Opens the passage a line came from. */
		onopen: (citations: Citation[], el: HTMLElement) => void;
		/** The set changed: recorded, or further along. */
		onupdate: (set: StudySet) => void;
	} = $props();

	const POLL_MS = 1500;
	const audio = $derived(set.audio!);
	const recorded = $derived(audio.seconds > 0);
	let problem = $state<string | null>(null);
	let url = $state<string | null>(null);
	let player = $state<HTMLAudioElement>();
	let now = $state(0);
	let playing = $state(false);

	// Until it is recorded, check on the recording.
	$effect(() => {
		if (recorded || audio.problem) return;
		const timer = setInterval(async () => {
			try {
				onupdate(await api.study(slug, id));
			} catch (e) {
				problem = describe(e);
			}
		}, POLL_MS);
		return () => clearInterval(timer);
	});

	$effect(() => {
		if (!recorded || url) return;
		api.studyAudio(slug, id)
			.then((blob) => (url = URL.createObjectURL(blob)))
			.catch((e) => (problem = describe(e)));
	});
	onDestroy(() => url && URL.revokeObjectURL(url));

	const current = $derived.by(() => {
		if (!recorded) return -1;
		let at = -1;
		for (const [i, line] of audio.lines.entries()) if (line.start <= now) at = i;
		return at;
	});

	// Keep the line being spoken in view while the recording plays.
	$effect(() => {
		if (!playing || current < 0) return;
		tick().then(() => document.getElementById(`line-${current}`)?.scrollIntoView({ block: 'nearest' }));
	});

	function play(start: number) {
		if (!player) return;
		player.currentTime = start;
		player.play();
	}

	function download() {
		if (!url) return;
		const link = document.createElement('a');
		link.href = url;
		link.download = `${set.title}.ogg`;
		link.click();
	}

	const mb = (bytes: number) => Math.round(bytes / 1_000_000);
	const clock = (seconds: number) =>
		`${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`;
	/** A line's citations, one entry per source letter. */
	const bySource = (citations: Citation[]) => {
		const groups = new Map<string, Citation[]>();
		for (const c of citations) groups.set(c.letter, [...(groups.get(c.letter) ?? []), c]);
		return [...groups.values()];
	};
	const HOSTS = ['First host', 'Second host'];
</script>

<div class="audio">
	{#if problem}<p class="problem" role="alert">{problem}</p>{/if}
	{#if audio.problem}
		<p class="problem" role="alert">The recording failed. {audio.problem}</p>
	{:else if !recorded}
		{@const r = set.recording}
		<p class="status" role="status">
			{#if r?.stage === 'downloading'}
				Downloading the voices, once{#if r.total}: {mb(r.done)} of {mb(r.total)} MB{/if}.
			{:else if r?.stage === 'speaking'}
				Recording line {r.done} of {r.total}.
			{:else}
				Waiting for another recording to finish.
			{/if}
		</p>
		{#if r?.stage === 'speaking' || r?.stage === 'downloading'}
			<progress max={r.total || 1} value={r.done}></progress>
		{/if}
	{:else if url}
		<audio
			bind:this={player}
			src={url}
			controls
			preload="auto"
			ontimeupdate={(e) => (now = e.currentTarget.currentTime)}
			onplay={() => (playing = true)}
			onpause={() => (playing = false)}
		></audio>
		<p class="actions"><button class="action" onclick={download}>Download audio</button></p>
	{/if}

	<ol class="script" aria-label="Transcript">
		{#each audio.lines as line, i (i)}
			<li id="line-{i}" class:current={i === current}>
				{#if i === 0 || audio.lines[i - 1].host !== line.host}<p class="host">{HOSTS[line.host]}</p>{/if}
				<p class="said">
					{#if recorded}<button class="time" onclick={() => play(line.start)} aria-label="Play from {clock(line.start)}"
							>{clock(line.start)}</button
						>{/if}{line.text}
					{#each bySource(line.citations ?? []) as group (group[0].letter)}
						{@const doubtful = group.some((c) => !c.verified)}
						<button
							class="siglum-button"
							class:unverified={doubtful}
							aria-label="Source {group[0].letter}{doubtful ? ', a quote not found' : ''}: {group
								.map((c) => c.quote)
								.join('; ')}"
							title={group.map((c) => c.quote).join('\n')}
							onclick={(e) => onopen(group, e.currentTarget)}>{group[0].letter}{#if doubtful}†{/if}</button
						>
					{/each}
				</p>
			</li>
		{/each}
	</ol>
</div>

<style>
	.status {
		margin: 0 0 0.5rem;
		font-style: italic;
		color: var(--ink-2);
	}
	progress {
		width: 100%;
		height: 3px;
		margin: 0 0 1.5rem;
		appearance: none;
		border: 0;
		background: var(--line);
		accent-color: var(--rubric);
	}
	progress::-webkit-progress-bar {
		background: var(--line);
	}
	progress::-webkit-progress-value {
		background: var(--rubric);
	}
	progress::-moz-progress-bar {
		background: var(--rubric);
	}
	audio {
		display: block;
		width: 100%;
		margin: 0 0 0.25rem;
	}
	.actions {
		margin: 0 0 1.5rem;
	}
	.action {
		padding: 0.3rem 0;
		border: 0;
		background: none;
		color: var(--rubric);
		font: inherit;
		font-size: var(--t-xs);
		font-weight: 600;
		cursor: pointer;
	}
	.script {
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.script li {
		margin: 0 0 0.6rem;
		padding-left: 0.75rem;
		border-left: 2px solid transparent;
	}
	.script li.current {
		border-left-color: var(--rubric);
	}
	.host {
		margin: 1.1rem 0 0.25rem;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}
	.said {
		margin: 0;
	}
	.time {
		margin-right: 0.5em;
		padding: 0;
		border: 0;
		background: none;
		color: var(--ink-2);
		font: inherit;
		font-size: var(--t-xs);
		font-variant-numeric: tabular-nums;
		cursor: pointer;
	}
	.time:hover,
	.current .time {
		color: var(--rubric);
	}
	.siglum-button {
		margin-left: 0.3em;
		padding: 0 0.15em;
		border: 0;
		background: none;
		color: var(--rubric);
		font: inherit;
		font-size: var(--t-xs);
		font-style: italic;
		font-weight: 600;
		vertical-align: super;
		line-height: 1;
		cursor: pointer;
	}
	.siglum-button.unverified {
		color: var(--ink-2);
	}
	.problem {
		font-style: italic;
		color: var(--danger);
	}
</style>
