<script lang="ts">
	import type { AudioFormat, Difficulty, StudyKind } from '#lib/api.ts';
	import Choice from './Choice.svelte';
	import Dialog from './Dialog.svelte';

	type Kind = Exclude<StudyKind, 'mindmap' | 'table' | 'infographic'>;

	let {
		oncreate
	}: {
		oncreate: (kind: Kind, count: number, difficulty: Difficulty, topic: string, format: AudioFormat) => void;
	} = $props();

	let dialog: Dialog;
	let kind = $state<Kind>('flashcards');
	let size = $state<'fewer' | 'standard' | 'more'>('standard');
	let difficulty = $state<Difficulty>('medium');
	let format = $state<AudioFormat>('deep-dive');
	let topic = $state('');

	const COUNTS = {
		flashcards: { fewer: 10, standard: 20, more: 30 },
		quiz: { fewer: 5, standard: 10, more: 15 },
		slides: { fewer: 6, standard: 10, more: 15 },
		audio: { fewer: 20, standard: 40, more: 70 }
	};
	/** A brief is short whatever length is picked. */
	const BRIEF_LINES = 12;
	const counts = $derived(COUNTS[kind]);
	const WORDS = {
		flashcards: { title: 'New flashcards', items: 'Cards', item: 'card', action: 'Write the flashcards' },
		quiz: { title: 'New quiz', items: 'Questions', item: 'question', action: 'Write the quiz' },
		slides: { title: 'New slides', items: 'Slides', item: 'slide', action: 'Make the slides' },
		audio: { title: 'New audio overview', items: 'Length', item: 'line', action: 'Write and record it' }
	};
	const FORMATS: Record<AudioFormat, string> = {
		'deep-dive': 'Two hosts unpack the sources: one explains, the other asks what a curious listener would.',
		brief: 'A couple of minutes on the essentials.',
		critique: 'An expert review of the sources: their strengths, their gaps, and what to question.',
		debate: 'The hosts argue opposing sides the sources support, then say where they agree.'
	};

	export function show(next: Kind) {
		kind = next;
		topic = '';
		dialog.show();
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		const count = kind === 'audio' && format === 'brief' ? BRIEF_LINES : counts[size];
		oncreate(kind, count, difficulty, topic.trim(), format);
		dialog.close();
	}
</script>

<Dialog bind:this={dialog} title={WORDS[kind].title}>
	<form class="form" onsubmit={submit}>
		<div class="field">
			<label for="study-topic">Topic, if you want to focus on one</label>
			<input id="study-topic" bind:value={topic} placeholder="Leave empty to cover every source" />
		</div>
		{#if kind === 'audio'}
			<Choice
				legend="Format"
				name="audio-format"
				options={[
					{ value: 'deep-dive', label: 'Deep dive' },
					{ value: 'brief', label: 'Brief' },
					{ value: 'critique', label: 'Critique' },
					{ value: 'debate', label: 'Debate' }
				]}
				bind:value={format}
			/>
		{/if}
		{#if kind !== 'audio' || format !== 'brief'}
			<Choice
				legend={WORDS[kind].items}
				name="study-size"
				options={kind === 'audio'
					? [
							{ value: 'fewer', label: 'Shorter (about 3 minutes)' },
							{ value: 'standard', label: 'Standard (about 6 minutes)' },
							{ value: 'more', label: 'Longer (about 10 minutes)' }
						]
					: [
							{ value: 'fewer', label: `Fewer (${counts.fewer})` },
							{ value: 'standard', label: `Standard (${counts.standard})` },
							{ value: 'more', label: `More (${counts.more})` }
						]}
				bind:value={size}
			/>
		{/if}
		{#if kind === 'flashcards' || kind === 'quiz'}
			<Choice
				legend="Difficulty"
				name="study-difficulty"
				options={[
					{ value: 'easy', label: 'Easy' },
					{ value: 'medium', label: 'Medium' },
					{ value: 'hard', label: 'Hard' }
				]}
				bind:value={difficulty}
			/>
		{/if}
		<p class="hint">
			{#if kind === 'audio'}
				{FORMATS[format]} Spoken in English and recorded on this computer; the first recording downloads the voices
				(163 MB) once.
			{:else if kind === 'slides'}
				Titles, points, and speaker notes, from an opening slide to a summary.
			{:else}
				{difficulty === 'easy'
					? 'Key terms, people, places, and dates.'
					: difficulty === 'medium'
						? 'Facts plus how and why: causes, effects, connections.'
						: 'Reasoning: comparisons, consequences, applying ideas to new cases.'}
			{/if}
			Each {WORDS[kind].item} links to the passages it comes from.
		</p>
		<button class="submit">{WORDS[kind].action}</button>
	</form>
</Dialog>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
	}
</style>
