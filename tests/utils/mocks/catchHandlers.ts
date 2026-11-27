import { mockWindows } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import { IPCHandler, installTauriMock } from '../tauriMock';
import { configHandlers } from './configHandlers';
import type { SearchEngine } from '../../../src/tauri/types/preferences';
import { CATCH_STATE_EVENT, type CatchFormat, type CatchHome, type CatchState } from '../../../src/tauri/types/catch';

export type CatchMockData = {
  /** catch_formats answer (default: 1080p best, 720p, audio). */
  formats?: CatchFormat[];
  /** catch_save: 'pending' never settles, 'fail' rejects. */
  save?: 'pending' | 'fail';
  /** catch_bar_fit: 'fail' rejects, as for a window that is closing. */
  fit?: 'fail';
  /** catch_home answer (default: empty lists). */
  home?: CatchHome;
  /** catch_use_browser_signin: an error code to reject with (default: 3 cookies from Chrome). */
  signin?: 'catchSigninNone' | 'catchSigninDenied' | 'catchSigninUnsupported';
  /** catch_tab_new: 'limit' rejects with catchTabLimit. */
  tabNew?: 'limit';
};

let tabCount = 1;
let signinOff: string[] = [];

/** Calls made to the catch commands, for e2e assertions (`window.__catchCalls`). */
export type CatchCall = { cmd: string; args?: unknown };
const record = (cmd: string, args?: unknown) => {
  const target = window as unknown as { __catchCalls?: CatchCall[] };
  (target.__catchCalls ??= []).push({ cmd, args });
};

const MB = 1024 * 1024;
const FORMATS: CatchFormat[] = [
  { id: 'v1080', label: 'video', height: 1080, ext: 'mp4', sizeBytes: 1240 * MB, best: true },
  { id: 'v720', label: 'video', height: 720, ext: 'mp4', sizeBytes: 640 * MB, best: false },
  { id: 'a', label: 'audio', ext: 'm4a', sizeBytes: 60 * MB, best: false },
];

const recorded: IPCHandler = (cmd, args) => {
  record(cmd, args);
  return null;
};

export const catchHandlers: Record<string, IPCHandler> = {
  catch_open: (cmd, args) => {
    record(cmd, args);
    return { windowId: 'catch-1' };
  },
  catch_formats: (cmd, args) => {
    record(cmd, args);
    return window.E2E?.catch?.formats ?? FORMATS;
  },
  catch_save: (cmd, args) => {
    record(cmd, args);
    if (window.E2E?.catch?.save === 'pending') return new Promise(() => {});
    if (window.E2E?.catch?.save === 'fail') return Promise.reject(new Error('catch_save failed'));
    return { downloadId: 'download-1' };
  },
  catch_bar_fit: (cmd, args) => {
    record(cmd, args);
    return window.E2E?.catch?.fit === 'fail' ? Promise.reject(new Error('catchWindowClosed')) : null;
  },
  catch_tab_new: (cmd, args) => {
    record(cmd, args);
    return window.E2E?.catch?.tabNew === 'limit' ? Promise.reject(new Error('catchTabLimit')) : { tabId: `tab-${++tabCount}` };
  },
  catch_home: (cmd, args) => {
    record(cmd, args);
    return window.E2E?.catch?.home ?? { recentSites: [], saved: [] };
  },
  catch_use_browser_signin: (cmd, args) => {
    record(cmd, args);
    const error = window.E2E?.catch?.signin;
    return error ? Promise.reject(error) : { imported: 3, browser: 'Chrome' };
  },
  // The browser sign-in switch's off list, kept by Rust (here: for this page only).
  catch_signin_off_get: (cmd, args) => {
    record(cmd, args);
    return [...signinOff];
  },
  catch_signin_off_set: (cmd, args) => {
    record(cmd, args);
    const { site, off } = (args ?? {}) as { site: string; off: boolean };
    signinOff = [...signinOff.filter(h => h !== site), ...(off ? [site] : [])];
    return [...signinOff];
  },
  // The shell's own two search values (it cannot call preferences_get/_set); seeded by `E2E.preferences`.
  catch_search_engine_get: () => ({ engine: window.E2E?.preferences?.searchEngine ?? 'duckduckgo', hintShown: window.E2E?.preferences?.searchHintShown ?? false }),
  catch_search_engine_set: (cmd, args) => {
    record(cmd, args);
    const change = (args ?? {}) as { engine?: SearchEngine; hintShown?: boolean };
    const saved = { engine: change.engine ?? window.E2E?.preferences?.searchEngine ?? 'duckduckgo', hintShown: change.hintShown ?? window.E2E?.preferences?.searchHintShown ?? false };
    if (window.E2E?.preferences) Object.assign(window.E2E.preferences, { searchEngine: saved.engine, searchHintShown: saved.hintShown });
    return saved;
  },
  ...Object.fromEntries(['catch_tab_select', 'catch_tab_close', 'catch_shell_fit', 'catch_clear_recent', 'catch_try_mobile', 'catch_navigate', 'catch_allow_popup_once', 'catch_forget_site', 'catch_deep_retry', 'catch_close',
    'catch_show_download', 'catch_report', 'catch_bar_ready'].map(cmd => [cmd, recorded])),
};

/** E2E only: the bar webview believes it is `label`; `window.__catchPush(state)` plays Rust's `catch://state`. */
export function installCatchBarMock(label: string): void {
  installTauriMock({ ...configHandlers, ...catchHandlers });
  mockWindows(label);
  (window as unknown as { __catchPush: (state: CatchState) => Promise<void> }).__catchPush = state => emit(CATCH_STATE_EVENT, state);
}
