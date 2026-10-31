import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';

// End-to-end tests run against a real bowerbird-server and a throwaway library.
// Asking calls the model, so DEEPSEEK_API_KEY must be set.
const port = 7491;
// Not 127.0.0.1: under WSL's mirrored networking a connection to a closed
// port there hangs instead of being refused, which stalls the check
// Playwright makes before starting the server for about two minutes.
const host = '127.0.0.2';
const library = mkdtempSync(join(tmpdir(), 'bowerbird-e2e-'));

export default defineConfig({
	testDir: 'tests',
	timeout: 180_000,
	workers: 1,
	use: { baseURL: `http://${host}:${port}` },
	webServer: {
		command: `../target/debug/bowerbird-server --addr ${host}:${port} --ui build --library ${library}`,
		url: `http://${host}:${port}/`,
		env: { ...process.env, BOWERBIRD_TOKEN: 'e2e' } as Record<string, string>,
		reuseExistingServer: false
	}
});
