import { mkdtempSync, truncateSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { crc32 } from 'node:zlib';
import { expect, test, type Page } from '@playwright/test';

const ARTICLE = `<!doctype html><title>Bowerbird Courtship Displays</title>
<h1>Bowerbird Courtship Displays</h1>
<h2>Building the bower</h2>
<p>The Vogelkop bowerbird builds a hut of sticks over a mat of green moss. In front of the hut
it lays out its finds in separate piles sorted by color: red fruit, black beetle wings, and blue berries.</p>
<h2>Choosing a mate</h2>
<p>Females visit several bowers and judge the arrangement before choosing a male. Males that keep
their piles tidy and well sorted are chosen more often.</p>`;

/** A real PDF with a text layer, printed by the browser itself. */
async function articlePdf(page: Page): Promise<string> {
	await page.setContent(ARTICLE);
	const path = join(mkdtempSync(join(tmpdir(), 'bowerbird-pdf-')), 'bowerbird-courtship.pdf');
	await page.pdf({ path });
	return path;
}

async function notebookWithArticle(page: Page) {
	const pdf = await articlePdf(page);
	await page.goto('/#token=e2e');
	await page.getByRole('button', { name: 'New notebook' }).click();
	await page.getByLabel('Notebook name').fill(`Birds ${Date.now()}`);
	await page.getByRole('button', { name: 'Create' }).click();
	await expect(page.getByRole('dialog', { name: 'Add sources' })).toBeVisible();
	await page.locator('dialog input[type=file]').setInputFiles(pdf);
	await expect(page.getByRole('complementary', { name: 'Contents' }).getByText('Bowerbird Courtship Displays')).toBeVisible({
		timeout: 60_000
	});
}

async function askAndWait(page: Page, question: string) {
	await page.getByLabel('Question').fill(question);
	await page.getByLabel('Question').press('Enter');
	const mark = page.locator('.mark:not(.unverified)').first();
	await expect(mark).toBeVisible({ timeout: 120_000 });
	await expect(page.locator('[aria-busy=true]')).toHaveCount(0, { timeout: 120_000 });
	return mark;
}

test('adds a PDF, asks, and opens a verified citation at its passage', async ({ page }) => {
	await notebookWithArticle(page);
	const mark = await askAndWait(page, 'How does the Vogelkop bowerbird arrange its finds?');
	await mark.click();
	const highlight = page.locator('.pane .passage mark').first();
	await expect(highlight).toBeVisible();
	await expect(highlight).toContainText(/\S/);
	await page.getByRole('button', { name: /See page \d+ as printed/ }).click();
	await expect(page.locator('.page-view canvas')).toBeVisible();
	await expect(page.locator('.page-view mark').first()).toBeAttached();
	await expect(page.locator('.page-view figcaption')).toContainText('quoted words marked');
});

test('citations work from the keyboard alone', async ({ page }) => {
	await notebookWithArticle(page);
	const mark = await askAndWait(page, 'What do females judge before choosing a male?');
	await mark.focus();
	await expect(mark).toHaveCSS('outline-style', 'solid');
	await page.keyboard.press('Enter');
	const close = page.getByRole('button', { name: 'Close passage' }).last();
	await expect(close).toBeFocused();
	await page.keyboard.press('Escape');
	await expect(page.locator('.pane')).toHaveCount(0);
	await expect(mark).toBeFocused();
});

test('saves an answer as a note whose citations still open their passage', async ({ page }) => {
	await notebookWithArticle(page);
	const question = 'How does the Vogelkop bowerbird arrange its finds?';
	await askAndWait(page, question);
	await page.getByRole('button', { name: 'Save to note' }).click();
	await expect(page.getByRole('button', { name: 'Saved; open the note' })).toBeVisible();
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: question, exact: true }).click();
	const cite = page.locator('.prose sup.ref:not(.unverified) .mark').first();
	await expect(cite).toBeVisible();
	await cite.click();
	await expect(page.locator('.pane mark').first()).toBeVisible();
	await page.getByRole('button', { name: 'Back to the note' }).click();
	await expect(page.locator('.prose')).toContainText(/\S/);
});

test('writes flashcards from the sources and reviews one', async ({ page }) => {
	await notebookWithArticle(page);
	const contents = page.getByRole('complementary', { name: 'Contents' });
	await contents.getByRole('button', { name: 'Flashcards' }).click();
	await page.getByText('Fewer (10)').click();
	await page.getByRole('button', { name: 'Write the flashcards' }).click();
	await page.getByRole('button', { name: 'Show answer' }).click({ timeout: 150_000 });
	await expect(page.locator('.pane .cited').first()).toBeVisible();
	await page.getByRole('button', { name: /Got it/ }).click();
	await expect(page.locator('.pane .progress')).toContainText('1 done, 9 to go');
	await expect(contents.getByText(/cards, \d+ due/)).toBeVisible();
});

test('a notebook with every source switched off says so instead of answering', async ({ page }) => {
	await notebookWithArticle(page);
	await page.getByRole('checkbox', { name: /Use Bowerbird Courtship Displays in answers/ }).uncheck();
	await expect(page.getByText('All switched off.')).toBeVisible();
	await page.getByLabel('Question').fill('What do females judge?');
	await page.getByLabel('Question').press('Enter');
	await expect(page.getByRole('alert').filter({ hasText: 'Every source is switched off' })).toBeVisible();
});

test('draws a mind map and asks about one of its topics', async ({ page }) => {
	await notebookWithArticle(page);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Mind map' }).click();
	const topic = page.locator('.node:not(.root)').first();
	await expect(topic).toBeVisible({ timeout: 150_000 });
	const title = (await topic.textContent())?.replace(/\+\d+$/, '').trim() ?? '';
	await topic.click();
	await page.getByRole('button', { name: 'Ask about this' }).click();
	await expect(page.locator('.turn .question').last()).toContainText(title);
	await expect(page.locator('.mark').first()).toBeVisible({ timeout: 120_000 });
});

test('builds a table from the sources and exports it as CSV', async ({ page }) => {
	await notebookWithArticle(page);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Table' }).click();
	await page.getByLabel('What should the table collect?').fill('finds the bowerbird collects, with their colour');
	await page.getByRole('button', { name: 'Build the table' }).click();
	const table = page.locator('.pane table');
	await expect(table).toBeVisible({ timeout: 150_000 });
	await expect(table.locator('tbody tr').first()).toBeVisible();
	const [download] = await Promise.all([
		page.waitForEvent('download'),
		page.getByRole('button', { name: 'Export as CSV' }).click()
	]);
	expect(download.suggestedFilename()).toMatch(/\.csv$/);
});

test('writes a study guide that streams into a note with its citations', async ({ page }) => {
	await notebookWithArticle(page);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Report' }).click();
	await page.getByRole('button', { name: 'Write the report' }).click();
	await expect(page.getByRole('complementary', { name: /^Writing/ })).toBeVisible();
	await expect(page.locator('.prose sup.ref').first()).toBeVisible({ timeout: 150_000 });
	await expect(page.locator('#note-title')).toHaveValue(/^Study guide/);
	await expect(
		page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: /^Study guide/ })
	).toBeVisible();
});

test('a report keeps writing after its page closes and arrives whole', async ({ page }) => {
	await notebookWithArticle(page);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Report' }).click();
	await page.getByRole('button', { name: 'Write the report' }).click();
	await expect(page.getByRole('complementary', { name: /^Writing/ })).toBeVisible();
	const url = page.url();
	await page.goto('about:blank');
	const note = page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: /^Study guide/ });
	await expect(async () => {
		await page.goto(url);
		await expect(note).toBeVisible({ timeout: 2_000 });
	}).toPass({ timeout: 150_000, intervals: [3_000] });
	await note.click();
	await expect(page.locator('.prose sup.ref').first()).toBeVisible();
});

test('makes slides from the sources and exports them as PowerPoint', async ({ page }) => {
	await notebookWithArticle(page);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Slides' }).click();
	await page.getByText('Fewer (6)').click();
	await page.getByRole('button', { name: 'Make the slides' }).click();
	const stage = page.locator('.stage');
	await expect(stage.locator('.slide h2')).toBeVisible({ timeout: 150_000 });
	await expect(page.locator('.stage .apparatus .siglum').first()).toBeVisible();
	await page.keyboard.press('ArrowRight');
	await expect(stage).toHaveAttribute('aria-label', /^Slide 2 of \d+$/);
	const [download] = await Promise.all([
		page.waitForEvent('download'),
		page.getByRole('button', { name: 'Download PowerPoint' }).click()
	]);
	expect(download.suggestedFilename()).toMatch(/\.pptx$/);
});

test('draws an infographic with cited facts and exports it as PNG', async ({ page }) => {
	await notebookWithArticle(page);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Infographic' }).click();
	const poster = page.locator('.poster svg');
	await expect(poster.locator('text').first()).toBeVisible({ timeout: 150_000 });
	await expect(poster.locator('tspan').first()).toBeAttached();
	const [download] = await Promise.all([
		page.waitForEvent('download'),
		page.getByRole('button', { name: 'Download PNG' }).click()
	]);
	expect(download.suggestedFilename()).toMatch(/\.png$/);
});

test('records a brief audio overview and plays it from a chosen line', async ({ page }) => {
	// Writing the script, then reading it aloud on this machine.
	test.setTimeout(420_000);
	await notebookWithArticle(page);
	await page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: 'Audio overview', exact: true }).click();
	await page.getByText('Brief', { exact: true }).click();
	await page.getByRole('button', { name: 'Write and record it' }).click();
	const lines = page.locator('.pane .script li');
	await expect(lines.first()).toBeVisible({ timeout: 150_000 });
	await expect(page.locator('.pane .script .siglum-button').first()).toBeVisible();
	await expect(page.locator('.pane audio')).toBeVisible({ timeout: 240_000 });
	await lines.nth(2).locator('.time').click();
	await expect(lines.nth(2)).toHaveClass(/current/);
	await expect(page.locator('.pane .meta')).toContainText(/Brief, \d+ min/);
});

/** A zip with every file stored uncompressed, which is all a .docx or .epub needs to be. */
function zip(files: Record<string, string>): Buffer {
	const parts: Buffer[] = [];
	const directory: Buffer[] = [];
	let offset = 0;
	for (const [name, text] of Object.entries(files)) {
		const data = Buffer.from(text);
		const path = Buffer.from(name);
		const local = Buffer.alloc(30);
		local.writeUInt32LE(0x04034b50, 0);
		local.writeUInt16LE(20, 4);
		local.writeUInt16LE(33, 12); // 1980-01-01
		local.writeUInt32LE(crc32(data), 14);
		local.writeUInt32LE(data.length, 18);
		local.writeUInt32LE(data.length, 22);
		local.writeUInt16LE(path.length, 26);
		const entry = Buffer.alloc(46);
		entry.writeUInt32LE(0x02014b50, 0);
		entry.writeUInt16LE(20, 4);
		entry.writeUInt16LE(20, 6);
		entry.writeUInt16LE(33, 14);
		entry.writeUInt32LE(crc32(data), 16);
		entry.writeUInt32LE(data.length, 20);
		entry.writeUInt32LE(data.length, 24);
		entry.writeUInt16LE(path.length, 28);
		entry.writeUInt32LE(offset, 42);
		parts.push(local, path, data);
		directory.push(entry, path);
		offset += local.length + path.length + data.length;
	}
	const end = Buffer.alloc(22);
	end.writeUInt32LE(0x06054b50, 0);
	end.writeUInt16LE(directory.length / 2, 8);
	end.writeUInt16LE(directory.length / 2, 10);
	end.writeUInt32LE(directory.reduce((n, b) => n + b.length, 0), 12);
	end.writeUInt32LE(offset, 16);
	return Buffer.concat([...parts, ...directory, end]);
}

const W = 'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"';
const heading = (text: string) => `<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>${text}</w:t></w:r></w:p>`;
const para = (text: string) => `<w:p><w:r><w:t>${text}</w:t></w:r></w:p>`;
const DOCX = zip({
	'word/document.xml': `<w:document ${W}><w:body>${heading('Negotiations')}${para('Delegations met in Münster and Osnabrück.')}${heading('Terms')}${para('The treaties recognised the Dutch Republic.')}</w:body></w:document>`,
	'word/styles.xml': `<w:styles ${W}><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>`,
	'docProps/core.xml': '<cp:coreProperties xmlns:cp="cp" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Peace of Westphalia notes</dc:title></cp:coreProperties>'
});
const chapter = (title: string, text: string) =>
	`<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>${title}</h1><p>${text}</p></body></html>`;
const EPUB = zip({
	mimetype: 'application/epub+zip',
	'META-INF/container.xml':
		'<container><rootfiles><rootfile full-path="OEBPS/book.opf" media-type="application/oebps-package+xml"/></rootfiles></container>',
	'OEBPS/book.opf':
		'<package xmlns:dc="http://purl.org/dc/elements/1.1/"><metadata><dc:title>The Defenestration of Prague</dc:title></metadata>' +
		'<manifest><item id="a" href="window.xhtml" media-type="application/xhtml+xml"/><item id="b" href="revolt.xhtml" media-type="application/xhtml+xml"/></manifest>' +
		'<spine><itemref idref="a"/><itemref idref="b"/></spine></package>',
	'OEBPS/window.xhtml': chapter('The window', 'In May 1618 two regents were thrown from a window of Prague Castle.'),
	'OEBPS/revolt.xhtml': chapter('The revolt', 'The Bohemian estates rose against Ferdinand.')
});

test('adds a Word document and an EPUB and opens each at its headings', async ({ page }) => {
	await page.goto('/#token=e2e');
	await page.getByRole('button', { name: 'New notebook' }).click();
	await page.getByLabel('Notebook name').fill(`Books ${Date.now()}`);
	await page.getByRole('button', { name: 'Create' }).click();
	await page.locator('dialog input[type=file]').setInputFiles([
		{ name: 'westphalia.docx', mimeType: 'application/octet-stream', buffer: DOCX },
		{ name: 'prague.epub', mimeType: 'application/epub+zip', buffer: EPUB }
	]);
	const contents = page.getByRole('complementary', { name: 'Contents' });
	const word = contents.getByRole('button', { name: /^Peace of Westphalia notes, source/ });
	const book = contents.getByRole('button', { name: /^The Defenestration of Prague, source/ });
	await expect(word).toBeVisible({ timeout: 60_000 });
	await expect(book).toBeVisible();
	await expect(contents.getByText('Word document')).toBeVisible();
	await expect(contents.getByText('EPUB', { exact: true })).toBeVisible();
	await page.keyboard.press('Escape');
	await book.click();
	await expect(page.locator('.heading-link')).toHaveText(['The window', 'The revolt']);
	await expect(page.locator('.pane .passage')).toContainText('thrown from a window of Prague Castle');
	await expect(page.locator('.pane .passage .md-heading')).toHaveText('The window');
	await expect(page.locator('.pane .passage')).not.toContainText('#');
	await word.click();
	await expect(page.locator('.heading-link')).toHaveText(['Negotiations', 'Terms']);
});

test("adds the files it can and names each one it can't", async ({ page }) => {
	const dir = mkdtempSync(join(tmpdir(), 'bowerbird-batch-'));
	const big = join(dir, 'atlas.pdf');
	writeFileSync(big, '');
	truncateSync(big, 200 * 2 ** 20 + 1);
	writeFileSync(join(dir, 'letters.rtf'), '{\\rtf1 Letters from Münster}');
	writeFileSync(join(dir, 'treaty.md'), '# Treaty of Osnabrück\n\nSigned in October 1648 with the Swedish crown.\n');
	await page.goto('/#token=e2e');
	await page.getByRole('button', { name: 'New notebook' }).click();
	await page.getByLabel('Notebook name').fill(`Treaties ${Date.now()}`);
	await page.getByRole('button', { name: 'Create' }).click();
	await page.locator('dialog input[type=file]').setInputFiles(['atlas.pdf', 'letters.rtf', 'treaty.md'].map((f) => join(dir, f)));
	const contents = page.getByRole('complementary', { name: 'Contents' });
	await expect(contents.getByRole('button', { name: /^Treaty of Osnabrück, source/ })).toBeVisible({ timeout: 60_000 });
	const notice = page.getByRole('alert');
	await expect(notice).toContainText("Can't add atlas.pdf. A file can be up to 200 MB.");
	await expect(notice).toContainText("Can't add letters.rtf");
});

test('transcribes a recording and plays it from a passage', async ({ page }) => {
	// Transcribed on this machine; the first run also downloads the 190 MB model.
	test.setTimeout(300_000);
	await page.goto('/#token=e2e');
	await page.getByRole('button', { name: 'New notebook' }).click();
	await page.getByLabel('Notebook name').fill(`Lectures ${Date.now()}`);
	await page.getByRole('button', { name: 'Create' }).click();
	await page.locator('dialog input[type=file]').setInputFiles(fileURLToPath(new URL('lecture.ogg', import.meta.url)));
	const recording = page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: /^lecture, source/ });
	await expect(recording).toContainText(/Waiting to be transcribed|Transcribing|Downloading the transcriber/, {
		timeout: 30_000
	});
	await expect(recording).toContainText('Recording', { timeout: 240_000 });
	await page.keyboard.press('Escape');
	await recording.click();
	await expect(page.locator('.pane .passage')).toContainText(/defenestration of Prague/i);
	await page.getByRole('button', { name: 'Play from 0:00' }).click();
	await expect.poll(() => page.locator('.pane audio').evaluate((audio: HTMLAudioElement) => audio.paused)).toBe(false);
});

test('keeps the pictures of a web page and shows them beside their captions', async ({ page }) => {
	await page.setContent('<div style="width:300px;height:200px;background:#2f9a78"></div>');
	const plate = await page.locator('div').screenshot();
	const words = 'The Vogelkop bowerbird sorts its finds into piles by color in front of its hut. '.repeat(12);
	const article = `<!doctype html><html><head><title>Bowers of the Vogelkop</title></head><body><article>
<h1>Bowers of the Vogelkop</h1><p>${words}</p>
<figure><img src="/plate.png" width="300" height="200" alt=""><figcaption>A bower with its finds sorted by color</figcaption></figure>
<p>${words}</p></article></body></html>`;
	const site = createServer((request, response) => {
		const png = request.url === '/plate.png';
		response.writeHead(200, { 'content-type': png ? 'image/png' : 'text/html' });
		response.end(png ? plate : article);
	});
	await new Promise<void>((resolve) => site.listen(0, '127.0.0.2', resolve));
	const { port } = site.address() as AddressInfo;
	try {
		await page.goto('/#token=e2e');
		await page.getByRole('button', { name: 'New notebook' }).click();
		await page.getByLabel('Notebook name').fill(`Bowers ${Date.now()}`);
		await page.getByRole('button', { name: 'Create' }).click();
		await page.getByLabel('Web page, PDF, or YouTube link').fill(`http://127.0.0.2:${port}/bowers.html`);
		await page.getByRole('button', { name: 'Add link' }).click();
		const source = page.getByRole('complementary', { name: 'Contents' }).getByRole('button', { name: /^Bowers of the Vogelkop, source/ });
		await expect(source).toBeVisible({ timeout: 60_000 });
		await page.keyboard.press('Escape');
		await source.click();
		await expect(page.locator('.pane .pager')).toContainText('Passage 1 of 2');
		await page.locator('.pane .pager').getByRole('button', { name: 'Next' }).click();
		const picture = page.locator('.pane .md-figure img');
		await expect(picture).toHaveAttribute('alt', 'A bower with its finds sorted by color');
		await expect.poll(() => picture.evaluate((img: HTMLImageElement) => img.naturalWidth)).toBeGreaterThan(0);
		const next = await picture.evaluate((img) => img.closest('figure')?.nextElementSibling?.textContent ?? '');
		expect(next).toContain('A bower with its finds sorted by color');
	} finally {
		site.close();
	}
});
