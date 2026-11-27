import type { InjectionKey } from 'vue';

/** The layout's "put the copied link into the toolbar field" (the same as ⌘/Ctrl+V outside a text field). */
export const PASTE_LINK: InjectionKey<() => Promise<void>> = Symbol('pasteLink');
