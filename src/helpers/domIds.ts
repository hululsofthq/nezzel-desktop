import { nextTick } from 'vue';

/** DOM hooks several components reach by id or selector: one name each, so a rename can't half-apply. */
export const MAIN_PANE_ID = 'nz-main';
export const LIST_SEARCH_ID = 'list-search';
/** Every download row (`<li class="row">` in DownloadRow). */
export const ROW_SELECTOR = 'li.row';
/** The "Show more" button under a paged list: ArrowDown on the last rendered row presses it. */
export const SHOW_MORE_ATTR = 'data-show-more';

/** ↑/↓ between rows. Reveal playlist folds before leaving a group, or the next list page outside groups. */
export function focusSiblingRow(from: HTMLElement, step: 1 | -1) {
  const rows = Array.from(document.querySelectorAll<HTMLElement>(ROW_SELECTOR));
  const at = rows.indexOf(from);
  if (at < 0) return;
  const next = rows[at + step];
  const playlistId = from.dataset.playlistId;
  if (step === 1 && playlistId && next?.dataset.playlistId !== playlistId) {
    const folds = Array.from(document.querySelectorAll<HTMLElement>('[data-playlist-fold]'))
      .filter(button => button.dataset.playlistFold === playlistId);
    // Waiting folds follow the children; saved folds can precede them.
    const fold = folds.find(button => from.compareDocumentPosition(button) & Node.DOCUMENT_POSITION_FOLLOWING) ?? folds.at(-1);
    if (fold) {
      const visible = new Set(rows);
      fold.click();
      void nextTick(() => Array.from(document.querySelectorAll<HTMLElement>(ROW_SELECTOR))
        .find(row => row.dataset.playlistId === playlistId && !visible.has(row))?.focus());
      return;
    }
  }
  if (next) {
    next.focus();
    return;
  }
  const more = step === 1 && !playlistId && at === rows.length - 1 ? document.querySelector<HTMLElement>(`[${SHOW_MORE_ATTR}]`) : null;
  if (!more) return;
  more.click();
  void nextTick(() => Array.from(document.querySelectorAll<HTMLElement>(ROW_SELECTOR))[at + 1]?.focus());
}
