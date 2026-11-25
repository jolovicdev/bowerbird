// Browser tests of the flows that need no real model: search, export, the
// model list, profiles, and how the page copes when a request goes wrong.
// The app's model is the stand-in server in model-server.mjs. Run them with
// `npm run test:flows`; they need no API key.

import { readFile } from 'node:fs/promises';
import { expect, test, type APIRequestContext, type Page } from '@playwright/test';
import { MODEL_SERVER, TOKEN } from './flows-env.ts';

const auth = { Authorization: `Bearer ${TOKEN}` };
const WRITER = 'stand-in-writer:1b';
const THINKER = 'stand-in-thinker:1b';

const BOWERS =
	'The satin bowerbird decorates its bower with ultramarine bottle caps, feathers and berries. Females inspect several bowers before they choose a mate.';
const PEACE =
	'The Peace of Westphalia was signed in 1648 in Münster and Osnabrück. It ended the Thirty Years War in the Holy Roman Empire.';

// Every test starts from the same settings: the stand-in server answers, and
// no profile is saved. Set before any page opens, so nothing here can reach
// a real provider, whatever keys this machine has.
test.beforeEach(async ({ request }) => {
	const saved = await request.put('/api/settings', {
		headers: auth,
		data: { chat_model: WRITER, helper_model: '', thinking: 'default', profiles: [], model_server: MODEL_SERVER }
	});
	expect(saved.ok()).toBeTruthy();
	await request.delete(`${MODEL_SERVER}/asked`);
});

/** Makes a notebook with two short sources, and any more given, and returns its slug. */
async function notebook(request: APIRequestContext, name: string, more: Record<string, string> = {}): Promise<string> {
	const made = await (await request.post('/api/notebooks', { headers: auth, data: { title: `${name} ${Date.now()}` } })).json();
	for (const [title, text] of Object.entries({ Bowerbirds: BOWERS, Westphalia: PEACE, ...more })) {
		const added = await request.post(`/api/notebooks/${made.slug}/sources/text`, { headers: auth, data: { title, text } });
		expect(added.ok()).toBeTruthy();
	}
	return made.slug;
}

async function open(page: Page, slug: string) {
	await page.goto(`/n/${slug}#token=${TOKEN}`);
	await expect(page.getByLabel('Question')).toBeVisible();
}

async function ask(page: Page, question: string) {
	await page.getByLabel('Question').fill(question);
	await page.getByLabel('Question').press('Enter');
	await expect(page.locator('.mark:not(.unverified)').first()).toBeVisible();
	await expect(page.locator('[aria-busy=true]')).toHaveCount(0);
}

/** What the stand-in model server was asked since the test began. */
async function asked(request: APIRequestContext): Promise<{ model: string; effort: string | null; question: string }[]> {
	return (await request.get(`${MODEL_SERVER}/asked`)).json();
}

test('search in a notebook finds a word, opens its passage with the word marked, and comes back', async ({ page, request }) => {
	await open(page, await notebook(request, 'Search'));
	await page.keyboard.press('Control+k');
	const field = page.getByLabel('Search query');
	await field.fill('zzqqxxvv');
	await expect(page.getByText('No matches for “zzqqxxvv”.')).toBeVisible();

	await field.fill('ultramarine');
	const hit = page.locator('.results .hit').first();
	await expect(hit).toContainText('Bowerbirds');
	await hit.click();
	await expect(page.locator('mark', { hasText: 'ultramarine' })).toBeVisible();

	await page.getByRole('button', { name: 'Back to search' }).click();
	await expect(page.getByRole('heading', { name: 'Search notebook' })).toBeVisible();
});

test('search across the library finds a passage in another notebook and opens it there', async ({ page, request }) => {
	// A word no other notebook holds. Notebooks of other tests may still come
	// up beside it, for passages close to it in meaning, so the hit is picked
	// by the notebook it is in.
	const word = `quillwort${Date.now()}`;
	const slug = await notebook(request, 'Library', { Ferns: `The ${word} grows at the edge of cold lakes.` });
	await page.goto(`/#token=${TOKEN}`);
	await page.getByLabel('Search across all notebooks').fill(word);
	const hit = page.locator(`.search-results .hits a[href^="/n/${slug}?source="]`);
	await expect(hit).toHaveCount(1);
	await expect(hit).toContainText('Ferns');
	await hit.click();
	await expect(page).toHaveURL(new RegExp(`/n/${slug}`));
	await expect(page.locator('mark', { hasText: word })).toBeVisible();
});

test('a conversation exports as Markdown with its citation, and a notebook as BibTeX and as a ZIP', async ({ page, request }) => {
	const slug = await notebook(request, 'Export');
	await open(page, slug);
	await ask(page, 'What does the bowerbird collect?');

	const [chat] = await Promise.all([
		page.waitForEvent('download'),
		page.getByRole('button', { name: 'Export Markdown' }).click()
	]);
	const markdown = await readFile(await chat.path(), 'utf8');
	expect(markdown).toContain('## 1. What does the bowerbird collect?');
	expect(markdown).toContain('[^1]: **[A] Bowerbirds** — "The satin bowerbird decorates its bower"');

	await page.getByRole('button', { name: 'Export', exact: true }).click();
	const [bibtex] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'BibTeX' }).click()]);
	expect(bibtex.suggestedFilename()).toBe(`${slug}.bib`);
	expect(await readFile(await bibtex.path(), 'utf8')).toContain('title = {Bowerbirds}');

	await page.getByRole('button', { name: 'Export', exact: true }).click();
	const [zip] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'ZIP' }).click()]);
	expect(zip.suggestedFilename()).toBe(`${slug}.zip`);
	expect((await readFile(await zip.path())).subarray(0, 2).toString()).toBe('PK');
});

test('the model list in Settings comes from the chosen server, narrows as you type, and fills the field', async ({ page }) => {
	await page.goto(`/settings#token=${TOKEN}`);
	await page.getByRole('button', { name: 'Browse models' }).click();
	// Chosen by name: the list opens on the first provider that has a key, which depends on the machine.
	await page.getByLabel('Provider', { exact: true }).selectOption('server');
	await expect(page.getByText('Showing 3 of 3 models')).toBeVisible();

	await page.getByLabel('Filter models').fill('thinker');
	await expect(page.locator('.model-list li')).toHaveCount(1);
	await page.locator('.model-list button').click();
	await expect(page.getByLabel('Model for answers')).toHaveValue(THINKER);

	await page.locator('form.model').first().getByRole('button', { name: 'Save', exact: true }).click();
	await expect(page.getByText(`Saved. New questions use ${THINKER}.`)).toBeVisible();
});

test('a saved profile carries its model and its thinking into a question', async ({ page, request }) => {
	const slug = await notebook(request, 'Profiles');
	await page.goto(`/settings#token=${TOKEN}`);
	const thinking = page.getByRole('group', { name: 'Thinking' });
	const save = async (name: string, model: string, level: string) => {
		await page.getByLabel('Model for answers').fill(model);
		await thinking.getByText(level, { exact: true }).click();
		await page.getByLabel('Profile name').fill(name);
		await page.getByRole('button', { name: 'Save profile' }).click();
		await expect(page.getByLabel('Profile name')).toHaveValue('');
	};
	await save('Careful', THINKER, 'Low');
	await save('Plain', WRITER, 'Default');
	const rows = page.locator('.profiles .row');
	await expect(rows.filter({ hasText: 'Careful' })).toContainText(`${THINKER}, thinking: low`);
	// A profile that leaves thinking to the model says nothing about it.
	await expect(rows.filter({ hasText: 'Plain' }).locator('.status')).toHaveText(WRITER);

	// Using a profile sets both of its halves, the model's own default included.
	await rows.filter({ hasText: 'Careful' }).getByRole('button', { name: 'Use' }).click();
	await expect(thinking.getByRole('radio', { name: 'Low' })).toBeChecked();
	await rows.filter({ hasText: 'Plain' }).getByRole('button', { name: 'Use' }).click();
	await expect(page.getByLabel('Model for answers')).toHaveValue(WRITER);
	await expect(thinking.getByRole('radio', { name: 'Default' })).toBeChecked();

	// Picked for one question, the profile's model answers it, at the profile's thinking level.
	await open(page, slug);
	await page.getByLabel('Model profile').selectOption('Careful');
	await page.getByLabel('Question').fill('What ended the Thirty Years War?');
	await page.getByLabel('Question').press('Enter');
	await page.getByRole('button', { name: 'Show thinking' }).click();
	await expect(page.getByRole('region', { name: "The model's thinking" })).toContainText('first passage');
	await expect(page.locator('.mark:not(.unverified)').first()).toBeVisible();
	const question = (await asked(request)).find((q) => q.question.startsWith('What ended'));
	expect(question).toMatchObject({ model: THINKER, effort: 'low' });
});

test('links pasted together are all added even if the dialog closes first', async ({ page, request }) => {
	const slug = await notebook(request, 'Links');
	await open(page, slug);
	const thrown: string[] = [];
	page.on('pageerror', (e) => thrown.push(e.message));
	// Each link is answered, after a moment, with the notebook as it is.
	const current = await (await request.get(`/api/notebooks/${slug}`, { headers: auth })).json();
	const links: string[] = [];
	await page.route('**/sources/url', async (route) => {
		links.push(route.request().postDataJSON().url);
		await new Promise((resolve) => setTimeout(resolve, 700));
		await route.fulfill({ json: current });
	});
	const contents = page.getByRole('complementary', { name: 'Contents' });
	const dialog = page.getByRole('dialog', { name: 'Add sources' });
	await contents.getByRole('button', { name: 'Add sources' }).click();
	await dialog.getByText('Add multiple links').click();
	await dialog
		.getByLabel('One web page, PDF, or YouTube link per line')
		.fill('https://example.org/one\nhttps://example.org/two');
	await dialog.getByRole('button', { name: 'Add links' }).click();
	await page.keyboard.press('Escape');

	// Opened again for something else while the links are still on their way.
	await contents.getByRole('button', { name: 'Add sources' }).click();
	await dialog.getByText('Paste text').click();
	await dialog.getByLabel('Title').fill('Typed while the links load');
	await expect.poll(() => links).toEqual(['https://example.org/one', 'https://example.org/two']);
	expect(thrown).toEqual([]);

	// The batch is over: the dialog takes a link again, and what was typed in it is still there.
	await dialog.getByLabel('Web page, PDF, or YouTube link', { exact: true }).fill('https://example.org/three');
	await expect(dialog.getByRole('button', { name: 'Add link', exact: true })).toBeEnabled();
	await expect(dialog.getByLabel('Title')).toHaveValue('Typed while the links load');
});

test('a report whose connection drops says so instead of writing on forever', async ({ page, request }) => {
	await open(page, await notebook(request, 'Report'));
	// A stream that ends with neither the note nor an error, as when a proxy cuts it.
	await page.route('**/reports', (route) =>
		route.fulfill({
			status: 200,
			headers: { 'Content-Type': 'text/event-stream' },
			body: 'event: note\ndata: {"title":"Briefing"}\n\nevent: text\ndata: "Half a report."\n\n'
		})
	);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Report' }).click();
	await page.getByRole('button', { name: 'Write the report' }).click();
	const draft = page.getByRole('complementary', { name: /^Writing/ });
	await expect(draft.getByRole('status')).toContainText('The connection dropped');
	await expect(draft.locator('[aria-busy=true]')).toHaveCount(0);
});

test('two study sets written at once each keep their own progress', async ({ page, request }) => {
	await open(page, await notebook(request, 'Studio'));
	await page.route('**/studio/stream', async (route) => {
		await new Promise((resolve) => setTimeout(resolve, 7000));
		await route.fulfill({
			status: 200,
			headers: { 'Content-Type': 'text/event-stream' },
			body: 'event: error\ndata: {"error":"Stopped for the test."}\n\n'
		});
	});
	const contents = page.getByRole('complementary', { name: 'Contents' });
	const write = async () => {
		await contents.getByRole('button', { name: 'Flashcards' }).click();
		await page.getByRole('button', { name: 'Write the flashcards' }).click();
	};
	await write();
	await page.waitForTimeout(2500);
	await write();
	// Two sets of one kind, started seconds apart, count their own seconds.
	const progress = contents.locator('li.pending', { hasText: 'flashcards' }).locator('.meta');
	await expect(progress).toHaveCount(2);
	const seconds = (await progress.allInnerTexts()).map((line) => Number(/(\d+) s/.exec(line)?.[1]));
	expect(seconds[0] - seconds[1]).toBeGreaterThanOrEqual(1);
	// The two streams are still waiting to be answered; they end with the test.
	await page.unrouteAll({ behavior: 'ignoreErrors' });
});
