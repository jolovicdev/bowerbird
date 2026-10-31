// Pictures from sources, fetched with the API token and shown from blob URLs.

import { api } from '#lib/api.ts';

// Kept for the whole visit: a notebook's pictures come to a few megabytes.
const loaded = new Map<string, Promise<string>>();

export function figureUrl(slug: string, sourceId: string, file: string): Promise<string> {
	const key = `${slug}/${sourceId}/${file}`;
	let url = loaded.get(key);
	if (!url) {
		url = api.sourceImage(slug, sourceId, file).then((blob) => URL.createObjectURL(blob));
		url.catch(() => loaded.delete(key));
		loaded.set(key, url);
	}
	return url;
}
