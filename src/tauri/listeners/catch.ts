import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import type { SearchEngine } from '../types/preferences';
import { CATCH_AUTO_EVENT, CATCH_FILE_DOWNLOAD_EVENT, CATCH_SHORTCUT_EVENT, CATCH_STATE_EVENT, type CatchAutoState, type CatchFileDownload, type CatchFormat, type CatchHome, type CatchNavigation, type CatchState } from '../types/catch';

/** The optional trailing tab of a v3 command; absent means the active tab (and keeps v1 calls unchanged). */
const tab = (tabId?: string) => (tabId ? { tabId } : {});

/** Opens a Nezzel browser window (a blank address bar without `url`). Failed rows, the quick bar and the link menu call it. */
export function catchOpen(url?: string): Promise<{ windowId: string }> {
  return invoke<{ windowId: string }>('catch_open', { url: url ?? null });
}

export function catchNavigate(windowId: string, action: CatchNavigation, tabId?: string): Promise<void> {
  return invoke('catch_navigate', { windowId, action, ...tab(tabId) });
}

export function catchSave(windowId: string, candidateId: string, formatId?: string): Promise<{ downloadId: string }> {
  return invoke<{ downloadId: string }>('catch_save', { windowId, candidateId, formatId: formatId ?? null });
}

export function catchFormats(windowId: string, candidateId: string): Promise<CatchFormat[]> {
  return invoke<CatchFormat[]>('catch_formats', { windowId, candidateId });
}

export function catchAllowPopupOnce(windowId: string, tabId?: string): Promise<void> {
  return invoke('catch_allow_popup_once', { windowId, ...tab(tabId) });
}

export function catchForgetSite(windowId: string, tabId?: string): Promise<void> {
  return invoke('catch_forget_site', { windowId, ...tab(tabId) });
}

export function catchDeepRetry(windowId: string, tabId?: string): Promise<void> {
  return invoke('catch_deep_retry', { windowId, ...tab(tabId) });
}

/** Stops the page's media and drops its state; never cancels a download. */
export function catchClose(windowId: string): Promise<void> {
  return invoke('catch_close', { windowId });
}

export function catchBarReady(windowId: string): Promise<void> {
  return invoke('catch_bar_ready', { windowId });
}

/** Keep the page below the bar body while the bar's overlays extend over it. */
export function catchBarFit(windowId: string, height: number, bodyHeight: number): Promise<void> {
  return invoke('catch_bar_fit', { windowId, height, bodyHeight });
}

/** Shows the window's saved download, or `downloadId` (a home page row) when given. */
export function catchShowDownload(windowId: string, downloadId?: string): Promise<void> {
  return invoke('catch_show_download', downloadId ? { windowId, downloadId } : { windowId });
}

export function catchTabNew(windowId: string, opts: { url?: string; private?: boolean } = {}): Promise<{ tabId: string }> {
  return invoke<{ tabId: string }>('catch_tab_new', { windowId, opts });
}

export function catchTabSelect(windowId: string, tabId: string): Promise<void> {
  return invoke('catch_tab_select', { windowId, tabId });
}

export function catchTabClose(windowId: string, tabId: string): Promise<void> {
  return invoke('catch_tab_close', { windowId, tabId });
}

/** Where the page goes: below `top`, beside the `side` list (on the start side); `overlay` while the shell draws over it. */
export interface CatchShellFit { top: number; side: number; overlay: boolean; rtl: boolean }

export function catchShellFit(windowId: string, fit: CatchShellFit): Promise<void> {
  return invoke('catch_shell_fit', { windowId, fit });
}

export function catchHome(windowId: string): Promise<CatchHome> {
  return invoke<CatchHome>('catch_home', { windowId });
}

export function catchClearRecent(windowId: string): Promise<void> {
  return invoke('catch_clear_recent', { windowId });
}

/** `site`: the site the offer was for; Rust refuses when the page is no longer on it. */
export function catchUseBrowserSignin(windowId: string, site: string, tabId?: string): Promise<{ imported: number; browser?: string }> {
  return invoke<{ imported: number; browser?: string }>('catch_use_browser_signin', { windowId, site, ...tab(tabId) });
}

export function catchTryMobile(windowId: string, on: boolean, tabId?: string): Promise<void> {
  return invoke('catch_try_mobile', { windowId, ...tab(tabId), on });
}

export function catchAdblockSet(windowId: string, on: boolean, tabId?: string): Promise<void> {
  return invoke('catch_adblock_set', { windowId, on, ...tab(tabId) });
}

/** The shell's two saved search values; it has no access to the other preferences. */
export type CatchSearchChoice = { engine: SearchEngine; hintShown: boolean };

export function catchSearchEngineGet(): Promise<CatchSearchChoice> {
  return invoke<CatchSearchChoice>('catch_search_engine_get');
}

export function catchSearchEngineSet(change: Partial<CatchSearchChoice>): Promise<CatchSearchChoice> {
  return invoke<CatchSearchChoice>('catch_search_engine_set', change);
}

/** Sites where the browser sign-in is switched off (Rust keeps them; the hidden page reads them too).
 *  `legacy`: the shell's old local list, moved in the first time. */
export function catchSigninOffGet(legacy: string[]): Promise<string[]> {
  return invoke<string[]>('catch_signin_off_get', { legacy });
}

export function catchSigninOffSet(site: string, off: boolean): Promise<string[]> {
  return invoke<string[]>('catch_signin_off_set', { site, off });
}

export function catchReport(windowId: string): Promise<void> {
  return invoke('catch_report', { windowId });
}

/** A shortcut pressed while the keyboard was in a page: the shell focuses the address field or asks about the six-tab limit.
 *  `{ tabLimit: link }`: a link opened from the main window found six tabs; the tab that replaces the closed one opens it. */
export type CatchShortcut = 'address' | 'tabLimit' | { tabLimit: string };

export function listenCatchShortcut(handler: (what: CatchShortcut) => void): Promise<UnlistenFn> {
  return getCurrentWebview().listen<CatchShortcut>(CATCH_SHORTCUT_EVENT, event => handler(event.payload));
}

export function listenCatchState(handler: (state: CatchState) => void): Promise<UnlistenFn> {
  return getCurrentWebview().listen<CatchState>(CATCH_STATE_EVENT, event => handler(event.payload));
}

export function listenCatchFileDownload(handler: (notice: CatchFileDownload) => void): Promise<UnlistenFn> {
  return getCurrentWebview().listen<CatchFileDownload>(CATCH_FILE_DOWNLOAD_EVENT, event => handler(event.payload));
}

/** Stops a waiting or running automatic catch; the row goes back to what it was. */
export function catchAutoCancel(groupId: string): Promise<void> {
  return invoke('catch_auto_cancel', { groupId });
}

/** Downloads what the automatic catch found, from Rust's held snapshot (never `media_download` with the page link). */
export function catchAutoDownload(groupId: string, formatId?: string): Promise<{ downloadId: string }> {
  return invoke<{ downloadId: string }>('catch_auto_download', { groupId, formatId: formatId ?? null });
}

/** Main window: every automatic-catch state, keyed by its download group. */
export function listenCatchAuto(handler: (state: CatchAutoState) => void): Promise<UnlistenFn> {
  return listen<CatchAutoState>(CATCH_AUTO_EVENT, event => handler(event.payload));
}
