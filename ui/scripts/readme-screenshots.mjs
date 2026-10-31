// Builds a demo notebook from three Wikipedia articles against a running
// bowerbird-server with an empty library, asks it a question, draws a mind
// map, and saves the screenshots the README shows. Asks the configured model
// real questions, so it needs a working API key.
//
// usage: node scripts/readme-screenshots.mjs <base-url> <token> <out-dir>
import { mkdirSync } from 'node:fs';
import { chromium } from '@playwright/test';

const [base, token, out] = process.argv.slice(2);
if (!out) {
	console.error('usage: node scripts/readme-screenshots.mjs <base-url> <token> <out-dir>');
	process.exit(2);
}
mkdirSync(out, { recursive: true });

const SOURCES = [
	'https://en.wikipedia.org/wiki/Thirty_Years%27_War',
	'https://en.wikipedia.org/wiki/Peace_of_Westphalia',
	'https://en.wikipedia.org/wiki/Defenestrations_of_Prague'
];
const QUESTION = 'Why did the Peace of Westphalia weaken the Holy Roman Emperor?';
const MINUTE = 60_000;

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 900 }, colorScheme: 'light' });
const shot = async (name) => {
	await page.evaluate(() => window.scrollTo(0, 0));
	await page.waitForTimeout(500);
	await page.screenshot({ path: `${out}/${name}.png` });
};

await page.goto(`${base}/#token=${token}`);
await page.getByRole('button', { name: 'New notebook' }).click();
await page.getByLabel('Notebook name').fill("The Thirty Years' War");
await page.getByRole('button', { name: 'Create' }).click();
const contents = page.getByRole('complementary', { name: 'Contents' });
for (const [i, url] of SOURCES.entries()) {
	// Adding a link closes the dialog.
	if (i > 0) await contents.getByRole('button', { name: 'Add sources' }).click();
	await page.getByLabel('Web page, PDF, or YouTube link').fill(url);
	await page.getByRole('button', { name: 'Add link' }).click();
	await contents.locator('.siglum', { hasText: String.fromCharCode(65 + i) }).first().waitFor({ timeout: 2 * MINUTE });
}
await page.keyboard.press('Escape');
// Answers use search by meaning once every passage has its vector.
await page.locator('.index-note').waitFor({ state: 'detached', timeout: 15 * MINUTE });

await page.getByLabel('Question').fill(QUESTION);
await page.getByLabel('Question').press('Enter');
const mark = page.locator('.mark:not(.unverified)').first();
await mark.waitFor({ timeout: 3 * MINUTE });
await page.locator('[aria-busy=true]').waitFor({ state: 'detached', timeout: 3 * MINUTE });
await page.getByLabel('Question').blur();
await shot('answer');

await mark.click();
await page.locator('.pane mark').first().waitFor();
await shot('passage');
await page.keyboard.press('Escape');
await page.locator('.pane').waitFor({ state: 'detached' });
// Closing the passage returns focus to its citation; drop the focus ring.
await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());

await page.emulateMedia({ colorScheme: 'dark' });
await shot('answer-dark');
await page.emulateMedia({ colorScheme: 'light' });

await page.setViewportSize({ width: 390, height: 844 });
await shot('phone');
await page.setViewportSize({ width: 1440, height: 900 });

await contents.getByRole('button', { name: 'Mind map' }).click();
await page.locator('.node:not(.root)').first().waitFor({ timeout: 3 * MINUTE });
await page.getByRole('button', { name: 'Open every topic' }).click();
await shot('mind-map');

await browser.close();
console.log(`Saved 5 screenshots to ${out}`);
