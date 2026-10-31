declare global {
	namespace App {
		interface PageState {
			/** A sheet is open beside or over the page, so Back closes it. */
			sheet?: boolean;
			/** A dialog is open. */
			dialog?: boolean;
			/** The contents are open over the page. */
			contents?: boolean;
		}
	}
}

export {};
