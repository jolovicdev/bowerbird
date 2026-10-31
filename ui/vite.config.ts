import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [sveltekit({ adapter: adapter({ fallback: '200.html' }) })],
	server: {
		// `npm run dev` talks to a bowerbird-server on its default port.
		proxy: { '/api': 'http://127.0.0.1:7477' }
	}
});
