<script lang="ts">
	let {
		sourceCount,
		busy = false,
		onask,
		onstop
	}: {
		sourceCount: number;
		busy?: boolean;
		onask: (question: string, deep: boolean) => void;
		onstop: () => void;
	} = $props();

	let question = $state('');
	let deep = $state(false);
	let field: HTMLTextAreaElement;

	export function focus() {
		field.focus();
	}

	/** Puts a question back after its answer failed, unless a new one is already typed. */
	export function restore(text: string) {
		if (question.trim()) return;
		question = text;
		requestAnimationFrame(grow);
	}

	/** Clears a restored question once it's asked again some other way. */
	export function forget(text: string) {
		if (question.trim() !== text) return;
		question = '';
		requestAnimationFrame(grow);
	}

	function grow() {
		field.style.height = 'auto';
		field.style.height = `${field.scrollHeight}px`;
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		const q = question.trim();
		if (!q || busy) return;
		onask(q, deep);
		question = '';
		deep = false;
		requestAnimationFrame(grow);
	}

	function keydown(event: KeyboardEvent) {
		if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
			event.preventDefault();
			field.form?.requestSubmit();
		}
	}
</script>

<form class="composer" onsubmit={submit}>
	<label class="visually-hidden" for="question">Question</label>
	<textarea
		id="question"
		rows="1"
		placeholder={sourceCount === 1 ? 'Ask this source' : `Ask the ${sourceCount} sources`}
		bind:value={question}
		bind:this={field}
		oninput={grow}
		onkeydown={keydown}
	></textarea>
	<div class="controls">
		<label class="deep">
			<input class="check" type="checkbox" bind:checked={deep} aria-describedby="deep-hint" />
			Deep search
		</label>
		{#if busy}
			<button type="button" class="stop" onclick={onstop}>Stop</button>
		{:else}
			<button type="submit" disabled={!question.trim()}>Ask</button>
		{/if}
	</div>
</form>
<p id="deep-hint" class={deep ? 'hint' : 'visually-hidden'}>
	Deep search reads every source's outline first, then whole sections that fit. Slower, finds more.
</p>

<style>
	.composer {
		display: flex;
		gap: 0.75rem;
		align-items: flex-end;
		padding: 0.55rem 0.55rem 0.55rem 1rem;
		border-radius: 6px;
		background: var(--raised);
		box-shadow: inset 0 0 0 1px var(--edge);
	}
	.composer:focus-within {
		box-shadow: inset 0 0 0 1px var(--ink);
	}
	textarea {
		flex: 1;
		min-width: 0;
		max-height: 12rem;
		padding: 0.4rem 0;
		border: 0;
		outline: 0;
		background: transparent;
		font-size: var(--t-m);
		line-height: 1.45;
		resize: none;
	}
	textarea::placeholder {
		color: var(--ink-2);
		font-style: italic;
	}
	.controls {
		display: flex;
		gap: 0.75rem;
		align-items: center;
	}
	.deep {
		display: flex;
		gap: 0.45rem;
		align-items: center;
		padding: 0.45rem 0.1rem;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
		white-space: nowrap;
		cursor: pointer;
	}
	.deep:has(:checked) {
		color: var(--ink);
	}
	button {
		padding: calc(0.45rem - var(--cap-lift)) 1rem calc(0.45rem + var(--cap-lift));
		border: 1px solid var(--rubric);
		border-radius: 4px;
		background: var(--rubric);
		color: var(--on-accent);
		font-size: var(--t-s);
		font-weight: 600;
		cursor: pointer;
	}
	button:disabled {
		border-color: var(--line);
		background: transparent;
		color: var(--ink-2);
		cursor: default;
	}
	.stop {
		background: transparent;
		color: var(--rubric);
	}
	.hint {
		margin: 0.45rem 0 0;
		font-size: var(--t-xs);
		font-style: italic;
		color: var(--ink-2);
	}

	/* Phones: the question gets the full width; options and the button go below. */
	@media (max-width: 719px) {
		.composer {
			flex-wrap: wrap;
			padding: 0.5rem 0.5rem 0.5rem 0.85rem;
		}
		textarea {
			flex-basis: 100%;
		}
		.controls {
			flex: 1;
			justify-content: space-between;
		}
	}
</style>
