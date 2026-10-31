export type ThemeChoice = 'system' | 'light' | 'dark';

const KEY = 'bowerbird-theme';

export function loadTheme(): ThemeChoice {
	try {
		const saved = localStorage.getItem(KEY);
		if (saved === 'light' || saved === 'dark') return saved;
	} catch {
		// Storage blocked: follow the system.
	}
	return 'system';
}

export function applyTheme(choice: ThemeChoice): void {
	const root = document.documentElement;
	if (choice === 'system') delete root.dataset.theme;
	else root.dataset.theme = choice;
	try {
		if (choice === 'system') localStorage.removeItem(KEY);
		else localStorage.setItem(KEY, choice);
	} catch {
		// Storage blocked: the choice lasts until reload.
	}
}
