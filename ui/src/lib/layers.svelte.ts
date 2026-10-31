// Overlays (dialogs, the contents drawer) take a history entry while open, so
// Back, from the browser or a phone, closes them before leaving the page.

import { pushState } from '$app/navigation';
import { page } from '$app/state';

type Layer = 'dialog' | 'contents';

export function pushLayer(layer: Layer) {
	if (!page.state[layer]) pushState('', { ...page.state, [layer]: true });
}

const leaving = new Map<Layer, Promise<void>>();

/** Removes the overlay's history entry. Resolves once Back has been handled,
 * so whatever opens next takes its own entry after it. Callers closing the
 * same overlay share one Back. */
export function popLayer(layer: Layer): Promise<void> {
	if (!page.state[layer]) return Promise.resolve();
	let done = leaving.get(layer);
	if (!done) {
		done = new Promise<void>((resolve) => {
			addEventListener('popstate', () => setTimeout(resolve), { once: true });
			history.back();
		}).finally(() => leaving.delete(layer));
		leaving.set(layer, done);
	}
	return done;
}

/** Calls `close` when Back removes the overlay's entry while it is open. */
export function closeOnBack(layer: Layer, isOpen: () => boolean, close: () => void) {
	let inHistory = false;
	$effect(() => {
		const shown = page.state[layer] === true;
		if (inHistory && !shown && isOpen()) close();
		inHistory = shown;
	});
}
