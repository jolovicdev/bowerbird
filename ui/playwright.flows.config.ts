import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig } from '@playwright/test';
import { APP_PORT, HOST, MODEL_PORT, TOKEN } from './tests/flows-env.ts';

// Browser tests that need no API key. The app answers from a stand-in model
// server (tests/model-server.mjs), reached the way a user reaches Ollama on
// another machine, and it keeps its library and its settings in a throwaway
// folder, so a run leaves the real settings alone.
const scratch = mkdtempSync(join(tmpdir(), 'bowerbird-flows-'));

export default defineConfig({
	testDir: 'tests',
	testMatch: 'flows.spec.ts',
	timeout: 60_000,
	workers: 1,
	use: { baseURL: `http://${HOST}:${APP_PORT}` },
	webServer: [
		{
			command: 'node tests/model-server.mjs',
			url: `http://${HOST}:${MODEL_PORT}/v1/models`,
			env: { MODEL_SERVER_HOST: HOST, MODEL_SERVER_PORT: String(MODEL_PORT) },
			reuseExistingServer: false
		},
		{
			command: `../target/debug/bowerbird-server --addr ${HOST}:${APP_PORT} --ui build --library ${join(scratch, 'library')}`,
			url: `http://${HOST}:${APP_PORT}/`,
			env: {
				...process.env,
				BOWERBIRD_TOKEN: TOKEN,
				BOWERBIRD_CONFIG: join(scratch, 'settings.toml')
			} as Record<string, string>,
			reuseExistingServer: false
		}
	]
});
