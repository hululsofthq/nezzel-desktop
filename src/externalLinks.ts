import { openUrl } from '@tauri-apps/plugin-opener';

const OPENABLE = new Set(['http:', 'https:', 'mailto:', 'tel:']);

/**
 * Opens the app's own `target="_blank"` (or Ctrl/Shift-clicked) links in the default browser.
 * The opener plugin's built-in click script does the same but runs in every webview, Catch
 * browser tabs included, where it swallowed new-tab links (#111); it is off in lib.rs.
 */
export function installExternalLinks(target: EventTarget = window, open: (url: string) => unknown = openUrl) {
  target.addEventListener('click', (raw) => {
    const event = raw as MouseEvent;
    if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.altKey) return;
    const link = event.composedPath().find((node): node is HTMLAnchorElement => node instanceof Node && node.nodeName.toUpperCase() === 'A');
    if (!link?.href || (link.target !== '_blank' && !event.ctrlKey && !event.shiftKey)) return;
    const url = new URL(link.href);
    if (!OPENABLE.has(url.protocol)) return;
    event.preventDefault();
    void Promise.resolve(open(url.href)).catch(() => {});
  });
}
