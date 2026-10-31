<script lang="ts">
	import type { ReportKind } from '#lib/api.ts';
	import Choice from './Choice.svelte';
	import Dialog from './Dialog.svelte';

	let {
		oncreate
	}: {
		oncreate: (kind: ReportKind, topic: string, instructions: string) => void;
	} = $props();

	const INSTRUCTIONS_MAX = 5_000;
	const ABOUT: Record<ReportKind, string> = {
		'study-guide': 'Summary, key terms with definitions, review questions with answers, and essay questions.',
		briefing: 'The main points by theme, with key facts and quotations, and what the sources leave open.',
		timeline: 'Events in date order, then the people involved and their roles.',
		faq: 'Common questions about the sources with short answers, from basic to advanced.',
		custom: 'Any format you describe.'
	};

	let dialog: Dialog;
	let kind = $state<ReportKind>('study-guide');
	let topic = $state('');
	let instructions = $state('');
	let topicField = $state<HTMLInputElement>();

	export function show() {
		topic = '';
		dialog.show();
		topicField?.focus();
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		oncreate(kind, topic.trim(), instructions.trim());
		dialog.close();
	}
</script>

<Dialog bind:this={dialog} title="New report">
	<form class="form" onsubmit={submit}>
		<Choice
			legend="Kind"
			name="report-kind"
			options={[
				{ value: 'study-guide', label: 'Study guide' },
				{ value: 'briefing', label: 'Briefing' },
				{ value: 'timeline', label: 'Timeline' },
				{ value: 'faq', label: 'FAQ' },
				{ value: 'custom', label: 'Your own' }
			]}
			bind:value={kind}
		/>
		<p class="hint">{ABOUT[kind]} Every statement links to the passage it comes from.</p>
		{#if kind === 'custom'}
			<div class="field">
				<label for="report-instructions">Describe the report</label>
				<textarea
					id="report-instructions"
					rows="4"
					maxlength={INSTRUCTIONS_MAX}
					bind:value={instructions}
					placeholder="For example: a one-page summary for a museum visitor, with the five most important dates."
				></textarea>
			</div>
		{/if}
		<div class="field">
			<label for="report-topic">Topic, if you want to focus on one</label>
			<input id="report-topic" bind:this={topicField} bind:value={topic} placeholder="Leave empty to cover every source" />
		</div>
		<button class="submit" disabled={kind === 'custom' && !instructions.trim()}>Write the report</button>
	</form>
</Dialog>

<style>
	.form {
		display: flex;
		flex-direction: column;
		gap: 1.25rem;
	}
	.submit:disabled {
		border-color: var(--line);
		background: transparent;
		color: var(--ink-2);
		cursor: default;
	}
</style>
