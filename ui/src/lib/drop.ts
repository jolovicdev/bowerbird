// Reads dropped files and folders recursively via the non-standard
// FileSystemEntry API supported by Chromium, WebKit, and Firefox, falling back
// to `DataTransfer.files` when directory entries aren't available.

// A reader hands a folder over in batches, so it is read until a batch is empty.
function readAllEntries(folder: FileSystemDirectoryEntry): Promise<FileSystemEntry[]> {
	return new Promise((resolve) => {
		const reader = folder.createReader();
		const all: FileSystemEntry[] = [];
		const next = () => {
			reader.readEntries(
				(batch) => {
					if (!batch.length) return resolve(all);
					all.push(...batch);
					next();
				},
				() => resolve(all)
			);
		};
		next();
	});
}

// A dropped file carries only its own name, so its folder is kept here.
const droppedIn = new WeakMap<File, string>();

const parent = (path: string) => path.split('/').filter(Boolean).slice(0, -1).join('/');

/** The folder a file was dropped or picked inside, such as `Papers/2024`, or '' for a file by itself. */
export function folderOf(file: File): string {
	return droppedIn.get(file) ?? parent(file.webkitRelativePath);
}

async function walk(entry: FileSystemEntry, out: File[]) {
	if (entry.name.startsWith('.') || entry.name === '__MACOSX') return;
	if (entry.isFile) {
		const file = await new Promise<File | null>((resolve) =>
			(entry as FileSystemFileEntry).file(resolve, () => resolve(null))
		);
		if (!file) return;
		droppedIn.set(file, parent(entry.fullPath));
		out.push(file);
	} else if (entry.isDirectory) {
		for (const child of await readAllEntries(entry as FileSystemDirectoryEntry)) {
			await walk(child, out);
		}
	}
}

export async function droppedFiles(dt: DataTransfer | null | undefined): Promise<File[]> {
	if (!dt) return [];
	// Entries are only readable during the drop event, so all are taken before the first await.
	const entries = [...dt.items].map((item) => item.webkitGetAsEntry()).filter((entry) => entry !== null);
	if (entries.some((e) => e.isDirectory)) {
		const out: File[] = [];
		for (const entry of entries) await walk(entry, out);
		return out;
	}
	return [...dt.files].filter((f) => !f.name.startsWith('.'));
}

/** Triggers a browser download for a Blob. */
export function downloadBlob(blob: Blob, filename: string) {
	const url = URL.createObjectURL(blob);
	const link = document.createElement('a');
	link.href = url;
	link.download = filename;
	document.body.append(link);
	link.click();
	link.remove();
	setTimeout(() => URL.revokeObjectURL(url), 1000);
}
