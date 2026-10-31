// Locates a citation's quote inside its passage for highlighting. Compares
// letters and digits only, the same rule the core uses to verify quotes, so
// a quote that verified is also found here.

const LIGATURES: Record<string, string> = { ﬁ: 'fi', ﬂ: 'fl', ﬀ: 'ff', ﬃ: 'ffi', ﬄ: 'ffl' };
const WORD_CHAR = /[\p{L}\p{N}]/u;

/** [start, end) of the quote in `text`, spanning all its parts if it contains ellipses. */
export function quoteRange(text: string, quote: string): [number, number] | null {
	// Walks code points, not UTF-16 units: math italic letters in PDFs sit
	// outside the Basic Multilingual Plane and take two units each. `origin`
	// and `originEnd` map each UTF-16 unit of the normalized text back to the
	// character it came from.
	const norm: string[] = [];
	const origin: number[] = [];
	const originEnd: number[] = [];
	let i = 0;
	for (const c of text) {
		const expanded = LIGATURES[c] ?? (WORD_CHAR.test(c) ? c.toLowerCase() : '');
		norm.push(expanded);
		for (let unit = 0; unit < expanded.length; unit++) {
			origin.push(i);
			originEnd.push(i + c.length);
		}
		i += c.length;
	}
	const haystack = norm.join('');
	const parts = quote
		.split(/…|\.\.\./)
		.map((p) => [...p].flatMap((c) => LIGATURES[c] ?? (WORD_CHAR.test(c) ? c.toLowerCase() : '')).join(''))
		.filter(Boolean);
	if (!parts.length) return null;

	let from = 0;
	let start = -1;
	let end = -1;
	for (const part of parts) {
		const at = haystack.indexOf(part, from);
		if (at < 0) return null;
		if (start < 0) start = at;
		end = at + part.length;
		from = end;
	}
	return [origin[start], originEnd[end - 1]];
}
