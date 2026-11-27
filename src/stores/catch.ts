import { defineStore } from 'pinia';
import { computed, ref, shallowRef, watch } from 'vue';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { SEARCH_ENGINES, type SearchEngine } from '../tauri/types/preferences';
import { usePreferencesStore } from './preferences';
import { useToastStore } from './toast';
import { i18n as nzI18n } from '../i18n';
import type { CatchCandidate, CatchFileDownload, CatchFormat, CatchHome, CatchNavigation, CatchState, CatchTab } from '../tauri/types/catch';
import {
  catchAllowPopupOnce, catchBarReady, catchClose, catchDeepRetry, catchForgetSite, catchFormats, catchHome, catchNavigate, catchReport, catchSave, catchSearchEngineGet, catchSearchEngineSet,
  catchSigninOffGet, catchSigninOffSet,
  catchShellFit, catchShowDownload, catchTabClose, catchTabNew, catchTabSelect, catchTryMobile, catchAdblockSet, catchUseBrowserSignin, listenCatchFileDownload, listenCatchShortcut, listenCatchState, type CatchSearchChoice, type CatchShellFit,
} from '../tauri/listeners/catch';

/** Nothing played this long after the page loaded: the hint nudges ("Nothing found yet"). */
export const CATCH_NUDGE_MS = 20_000;

/** What the status strip shows. One per phase, plus `nudge` (looking, nothing played for a while). */
export type CatchViewKind = 'hint' | 'nudge' | 'signIn' | 'adPlaying' | 'found' | 'drm' | 'live' | 'notFound' | 'saving' | 'crashed' | 'geoBlocked' | 'loadFailed';
/** Strip colour: tinted hint, green only for a caught main video, amber "needs you", red only for protected. */
export type CatchTone = 'hint' | 'ok' | 'amber' | 'neutral' | 'red';
export type CatchAction = 'reload' | 'report' | 'close' | 'showDownload';
export interface CatchView { kind: CatchViewKind; tone: CatchTone; canSave: boolean; actions: CatchAction[] }

/** Phase → strip. Save exists only with a main candidate in `found`; never in drm, live or while saving. */
export function catchView(state: CatchState | null, nudge: boolean): CatchView {
  const view = (kind: CatchViewKind, tone: CatchTone, actions: CatchAction[] = []): CatchView => ({ kind, tone, canSave: false, actions });
  switch (state?.phase) {
    case undefined:
      return view('hint', 'hint');
    case 'looking':
      return nudge && !state.played ? view('nudge', 'neutral') : view('hint', 'hint');
    case 'signIn':
      return view('signIn', 'amber');
    case 'adPlaying':
      return view('adPlaying', 'neutral');
    case 'found':
      return { ...view('found', 'ok'), canSave: state.candidates.some(c => c.main) };
    case 'drm':
      return view('drm', 'red', ['close']);
    case 'live':
      return view('live', 'neutral');
    case 'notFound':
      return view('notFound', 'neutral', ['reload', 'report']);
    case 'saving':
      return view('saving', 'ok', state.savedDownloadId ? ['showDownload', 'close'] : ['close']);
    case 'crashed':
      return view('crashed', 'amber', ['reload']);
    case 'loadFailed':
      return view('loadFailed', 'amber', ['reload']);
    case 'geoBlocked':
      return view('geoBlocked', 'amber', ['reload']);
  }
}

/** The main video first (the first main candidate), then everything else for "Other videos on this page". */
export function splitCandidates(candidates: CatchCandidate[]): { main: CatchCandidate | null; others: CatchCandidate[] } {
  const main = candidates.find(c => c.main && !c.alreadySaved) ?? candidates.find(c => c.main) ?? null;
  return { main, others: candidates.filter(c => c !== main) };
}

/** The format Save uses until the user picks one: the one Rust marks best, else the first. */
export function defaultFormat(formats: CatchFormat[]): CatchFormat | null {
  return formats.find(f => f.best) ?? formats[0] ?? null;
}

/** Typed address → URL: a bare host gets https://. */
export function addressToUrl(text: string): string | null {
  const value = text.trim();
  if (!value || /\s/.test(value)) return null;
  const withScheme = /^[a-z][a-z\d+.-]*:/i.test(value) ? value : `https://${value}`;
  try {
    const url = new URL(withScheme);
    return url.hostname && (url.protocol === 'http:' || url.protocol === 'https:') ? url.href : null;
  } catch {
    return null;
  }
}

/** At most this many tabs per window (contract v3). */
export const CATCH_TAB_LIMIT = 6;
/** The side list is open by default from this window width, folded to icons below it. */
export const CATCH_SIDE_WIDE = 1100;
export const CATCH_SIDE_OPEN_PX = 240;
export const CATCH_SIDE_FOLDED_PX = 48;
const SIDE_KEY = 'nz-catch-side';
/** The shell's old local list of sites where the browser sign-in is off: moved into Rust once
 *  (`catch_signin_off_get`), so the hidden page of automatic catch respects it too. */
const SIGNIN_OFF_KEY = 'nz-catch-signin-off';
const MAX_SIGNIN_OFF = 200;

/** The user's fold choice wins; without one, folded under 1100 px wide. */
export function sideOpenDefault(width: number, remembered: boolean | null): boolean {
  return remembered ?? width >= CATCH_SIDE_WIDE;
}

/** Words typed in the address or home field search with the chosen engine (no query is logged or counted). */
export function searchUrl(words: string, engine: SearchEngine = 'duckduckgo'): string {
  return (SEARCH_ENGINES[engine] ?? SEARCH_ENGINES.duckduckgo).url.replace('{q}', () => encodeURIComponent(words));
}

/** Typed text → where to go: an address opens, words search the web; other schemes go nowhere. */
export function homeTarget(text: string, engine?: SearchEngine): { url: string; search: boolean } | null {
  const value = text.trim();
  if (!value) return null;
  const hostPort = /^[^\s:/]+:\d+(?:[/?#]|$)/.test(value);
  if (/^[a-z][a-z\d+.-]*:/i.test(value) && !hostPort) {
    const url = /\s/.test(value) ? null : addressToUrl(value);
    return url ? { url, search: false } : null;
  }
  const looksLikeHost = !/\s/.test(value) && (/^[^\s/?#.]+(?:\.[^\s/?#.]+)*\.[^\s/?#.\d]{2,}(?::\d+)?(?:[/?#]|$)/u.test(value) || /^localhost(?::\d+)?(?:[/?#]|$)/i.test(value) || hostPort);
  if (!looksLikeHost) return { url: searchUrl(value, engine), search: true };
  const url = addressToUrl(hostPort ? `https://${value}` : value);
  return url ? { url, search: false } : null;
}

export const CATCH_PICKER_PX = 420;
/** The quality picker's start edge: under its button's start edge, pulled back so the whole sheet stays in the page area. */
export function pickerStart(buttonStart: number, areaWidth: number): number {
  const width = Math.min(CATCH_PICKER_PX, areaWidth - 16);
  return Math.round(Math.max(8, Math.min(buttonStart, areaWidth - width - 8)));
}

/**
 * Direction of the address field's text. Empty: the placeholder follows the app language. An address
 * (typed or loaded) reads left to right in every language; typed search words follow their own script.
 */
export function addressDir(text: string, editing: boolean): 'ltr' | 'auto' | undefined {
  if (!text) return undefined;
  return editing && homeTarget(text)?.search ? 'auto' : 'ltr';
}

/** What a tab row (open list) or icon (folded strip) shows. Linked tabs sit right under their opener. */
export interface CatchTabRow { tab: CatchTab; active: boolean; linked: boolean; letter: string; home: boolean }

export function tabLetter(tab: Pick<CatchTab, 'host'>): string {
  const host = tab.host.replace(/^www\./i, '');
  return ([...host][0] ?? '').toLocaleUpperCase();
}

export function tabRows(tabs: CatchTab[], activeId: string | null): CatchTabRow[] {
  const ids = new Set(tabs.map(t => t.id));
  const row = (tab: CatchTab, linked: boolean): CatchTabRow => ({ tab, active: tab.id === activeId, linked, letter: tabLetter(tab), home: !tab.url });
  const rows: CatchTabRow[] = [];
  const seen = new Set<string>();
  // A tab's row, then every tab it opened (and those they opened) under it, one indent deep.
  const add = (tab: CatchTab, linked: boolean) => {
    if (seen.has(tab.id)) return;
    seen.add(tab.id);
    rows.push(row(tab, linked));
    for (const child of tabs) if (child.openerId === tab.id) add(child, true);
  };
  for (const tab of tabs) if (!tab.openerId || !ids.has(tab.openerId) || tab.openerId === tab.id) add(tab, false);
  // Tabs that name each other as opener have no top row: each still gets one.
  for (const tab of tabs) add(tab, false);
  return rows;
}

/** Site-tool errors → the plain message key under `catch.tools`. */
export function signinErrorKey(error: unknown): 'signinNone' | 'signinDenied' | 'signinUnsupported' | 'signinAlready' | 'signinFailed' | null {
  const code = typeof error === 'string' ? error : error instanceof Error ? error.message : '';
  if (code.includes('catchLeftSite') || code.includes('catchWindowClosed')) return null;
  if (code.includes('catchSigninNone')) return 'signinNone';
  if (code.includes('catchSigninDenied')) return 'signinDenied';
  if (code.includes('catchSigninUnsupported')) return 'signinUnsupported';
  if (code.includes('catchSigninBusy')) return 'signinAlready';
  return 'signinFailed';
}

/** A tab change the user made, shown at once and dropped by the first state after Rust answered. */
/**
 * A tab change shown before the engine confirms it. `settled`: the command answered. `landed`: this state already
 * shows the change (or the engine chose otherwise), so the op is dropped; a state sent before the change does not undo it.
 */
type TabOp = { settled: boolean; landed: (next: CatchState) => boolean } & (
  { kind: 'select'; tabId: string } | { kind: 'close'; tabId: string }
  | { kind: 'new'; tab: CatchTab; done: Promise<string | null>; /** Stands for the home page after the last tab closed. */ placeholder?: boolean }
  | { kind: 'mobile'; tabId: string; on: boolean });

/** Rust keeps each switched-off site as its registrable domain (`example.com`; an address stays itself): a host is off when it is that site or one of its hosts. */
export function signinOffFor(sites: readonly string[], host: string): boolean {
  const h = host.toLowerCase();
  return !!h && sites.some(site => h === site || h.endsWith(`.${site}`) || site.endsWith(`.${h}`));
}

function readSigninOff(): string[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(SIGNIN_OFF_KEY) ?? '[]');
    return Array.isArray(value) ? value.filter((host): host is string => typeof host === 'string').slice(-MAX_SIGNIN_OFF) : [];
  } catch {
    return [];
  }
}

function readSide(): boolean | null {
  try {
    const value = localStorage.getItem(SIDE_KEY);
    return value === 'open' ? true : value === 'folded' ? false : null;
  } catch {
    return null;
  }
}

/** The bar's state: fed by `catch://state`, acting through the contract commands. One store per bar webview. */
export const useCatchStore = defineStore('catch', () => {
  const state = shallowRef<CatchState | null>(null);
  const formats = ref<Record<string, CatchFormat[] | 'loading' | 'failed'>>({});
  const pickedCandidateId = ref<string | null>(null);
  const pickedFormatId = ref<string | null>(null);
  const pickerOpen = ref(false);
  const busy = ref(false);
  const saveFailed = ref(false);
  const nudge = ref(false);
  let nudgeTimer: ReturnType<typeof setTimeout> | undefined;
  let nudgeUrl = '';
  let unlisten: UnlistenFn | undefined;
  let unlistenShortcut: UnlistenFn | undefined;
  let unlistenFileDownload: UnlistenFn | undefined;
  const fileDownload = shallowRef<CatchFileDownload | null>(null);
  let fileDownloadTimer: ReturnType<typeof setTimeout> | undefined;
  /** Counts "focus the address field" asks from a page; the bar watches it (it owns the fields). */
  const addressAsks = ref(0);
  let delivery = 0;
  /** Counts page changes: a format answer for an earlier page is dropped. */
  let pages = 0;

  const windowId = computed(() => state.value?.windowId ?? null);
  /** The strip follows the tab in front; for a tab picked a moment ago only its phase is known until its state comes. */
  const shown = computed<CatchState | null>(() => {
    const s = state.value;
    const tab = activeTab.value;
    if (!s || stateIsActive.value || !tab) return s;
    return { ...s, url: tab.url, host: tab.host, title: tab.title, loading: tab.loading, phase: tab.phase === 'found' ? 'looking' : tab.phase,
      candidates: [], canBack: false, canForward: false, popupsBlocked: 0, adsBlocked: 0, played: false, savedDownloadId: undefined, signinHint: undefined };
  });
  const view = computed(() => catchView(shown.value, nudge.value));
  const candidates = computed(() => splitCandidates(state.value?.candidates ?? []));
  const candidate = computed<CatchCandidate | null>(() =>
    state.value?.candidates.find(c => c.id === pickedCandidateId.value) ?? candidates.value.main);
  const candidateFormats = computed<CatchFormat[]>(() => {
    const list = candidate.value ? formats.value[candidate.value.id] : undefined;
    return Array.isArray(list) ? list : [];
  });
  const formatsLoading = computed(() => !!candidate.value && formats.value[candidate.value.id] === 'loading');
  const format = computed<CatchFormat | null>(() =>
    candidateFormats.value.find(f => f.id === pickedFormatId.value) ?? defaultFormat(candidateFormats.value));
  const canSave = computed(() => view.value.canSave && !!candidate.value && !candidate.value.alreadySaved && !busy.value);

  // ---- v3: tabs, side list, home, site tools ----
  const ops = shallowRef<TabOp[]>([]);
  let pendingIds = 0;
  /** Tabs as Rust sent them; a v1 engine has none: the window is its one tab. */
  const sentTabs = computed<CatchTab[]>(() => {
    const s = state.value;
    if (!s) return [];
    if (s.tabs) return s.tabs;
    return [{ id: '', url: s.url, host: s.host, title: s.title, loading: s.loading, phase: s.phase, found: s.phase === 'found', private: false, asleep: false, mobile: false }];
  });
  const tabs = computed<CatchTab[]>(() => {
    let list = sentTabs.value;
    for (const op of ops.value) {
      if (op.kind === 'close') list = list.filter(t => t.id !== op.tabId);
      else if (op.kind === 'new' && !list.some(t => t.id === op.tab.id)) list = [...list, op.tab];
      else if (op.kind === 'mobile') list = list.map(t => (t.id === op.tabId ? { ...t, mobile: op.on } : t));
    }
    return list;
  });
  const activeTabId = computed<string | null>(() => {
    let id = state.value?.activeTabId ?? sentTabs.value[0]?.id ?? null;
    for (const op of ops.value) {
      if (op.kind === 'select') id = op.tabId;
      else if (op.kind === 'new') id = op.tab.id;
    }
    if (id !== null && tabs.value.some(t => t.id === id)) return id;
    return tabs.value.at(-1)?.id ?? null;
  });
  const activeTab = computed(() => tabs.value.find(t => t.id === activeTabId.value) ?? null);
  const rows = computed(() => tabRows(tabs.value, activeTabId.value));
  /** The state belongs to the tab in front (not a tab picked a moment ago that Rust has not shown yet). */
  const stateIsActive = computed(() => !state.value?.tabs || state.value.activeTabId === activeTabId.value);
  const isHome = computed(() => !!state.value && (activeTab.value ? !activeTab.value.url : !state.value.url));
  const atLimit = computed(() => tabs.value.length >= CATCH_TAB_LIMIT);
  /** "New tab" at six: asks which tab to close; holds what the new tab was for. */
  const limitAsk = ref<{ url?: string; private?: boolean } | null>(null);
  /** Tab id Rust knows (a just-opened tab has none yet): commands for it go to the active tab. */
  const realTabId = () => (state.value?.tabs && sentTabs.value.some(t => t.id === activeTabId.value) ? activeTabId.value ?? undefined : undefined);

  function settle(op: TabOp, ok: boolean) {
    if (!ok) {
      ops.value = ops.value.filter(o => o !== op);
      return;
    }
    op.settled = true;
    // The state that shows it may have come before the call returned: no later state would drop the op then
    // (closing the last tab left its stand-in beside the real home tab). Only a state newer than the op counts.
    const shown = state.value;
    if (shown && (pushedAt.get(op) ?? statesSeen) < statesSeen && op.landed(shown)) ops.value = ops.value.filter(o => o !== op);
  }
  /** States applied so far, and the count when each op began. */
  let statesSeen = 0;
  const pushedAt = new WeakMap<TabOp, number>();
  function push<T extends TabOp>(op: T): T {
    pushedAt.set(op, statesSeen);
    ops.value = [...ops.value, op];
    return op;
  }

  async function selectTab(tabId: string) {
    const id = windowId.value;
    if (!id || tabId === activeTabId.value || tabId.startsWith('pending-')) return;
    const from = state.value?.activeTabId;
    const op = push({
      kind: 'select', tabId, settled: false,
      landed: next => !next.tabs || next.activeTabId !== from || !next.tabs.some(t => t.id === tabId),
    });
    menuOpen.value = false;
    pickerOpen.value = false;
    limitAsk.value = null;
    settle(op, await catchTabSelect(id, tabId).then(() => true, () => false));
  }

  async function closeTab(tabId: string) {
    const id = windowId.value;
    if (!id) return;
    const closing = tabs.value.find(t => t.id === tabId);
    if (!closing) return;
    limitAsk.value = null;
    const index = tabs.value.indexOf(closing);
    const wasActive = tabId === activeTabId.value;
    const rest = tabs.value.filter(t => t.id !== tabId);
    /** A tab that is still opening has no real id yet. */
    const opening = ops.value.find((o): o is Extract<TabOp, { kind: 'new' }> => o.kind === 'new' && o.tab.id === tabId && !o.settled && !o.placeholder);
    const close: Extract<TabOp, { kind: 'close' }> = push({ kind: 'close', tabId, settled: false, landed: (next: CatchState) => !next.tabs?.some(t => t.id === close.tabId) });
    // Only this close's own ops are settled with it: other tab changes in flight keep their own answer.
    const mine: TabOp[] = [close];
    if (wasActive && rest.length) {
      // Next in front: a linked tab's opener, else the tab after it, else the one before.
      const next = rest.find(t => t.id === closing.openerId) ?? rest[index] ?? rest[index - 1];
      if (next) mine.push(push({ kind: 'select', tabId: next.id, settled: false, landed: close.landed }));
    }
    if (!rest.length) mine.push(homePlaceholder(close.landed));
    if (opening) {
      const real = await opening.done;
      if (real === null) {
        // It never opened: nothing to close.
        ops.value = ops.value.filter(o => !mine.includes(o));
        return;
      }
      close.tabId = real;
      ops.value = [...ops.value];
    }
    const ok = await catchTabClose(id, close.tabId).then(() => true, () => false);
    for (const o of mine) settle(o, ok);
    // A tab closed before any state listed it leaves no "new" op behind to bring it back.
    if (ok) ops.value = ops.value.filter(o => !(o.kind === 'new' && o.tab.id === close.tabId));
  }

  function homePlaceholder(landed: TabOp['landed']) {
    const tab: CatchTab = { id: `pending-${++pendingIds}`, url: '', host: '', loading: false, phase: 'looking', found: false, private: false, asleep: false, mobile: false };
    return push({ kind: 'new', tab, done: Promise.resolve(null), settled: false, placeholder: true, landed });
  }

  /** The proxy is on in Settings with an address the browser cannot take: it loads nothing, and says so. */
  function tellRefusal(error: unknown) {
    if (String(error).includes('catchProxyUnusable')) useToastStore().showToast(nzI18n.global.t('catch.proxyUnusable'), { style: 'error', duration: 8000 });
  }

  async function newTab(opts: { url?: string; private?: boolean } = {}) {
    const id = windowId.value;
    if (!id) return null;
    if (atLimit.value) {
      limitAsk.value = opts;
      return null;
    }
    limitAsk.value = null;
    menuOpen.value = false;
    pickerOpen.value = false;
    const tab: CatchTab = {
      id: `pending-${++pendingIds}`, url: opts.url ?? '', host: opts.url ? new URL(opts.url).hostname : '', loading: !!opts.url, phase: 'looking',
      found: false, private: !!opts.private, asleep: false, mobile: false,
    };
    let resolve!: (value: string | null) => void;
    const listed = (list?: CatchTab[]) => (list ?? []).map(t => t.id).join('\n');
    const before = listed(state.value?.tabs);
    const op: Extract<TabOp, { kind: 'new' }> = push({
      kind: 'new', tab, settled: false, done: new Promise(r => (resolve = r)),
      // Listed, or the engine's tabs changed some other way: its list is newer than this op and wins.
      landed: (next: CatchState) => !next.tabs || next.tabs.some(t => t.id === op.tab.id) || listed(next.tabs) !== before,
    });
    try {
      const { tabId } = await catchTabNew(id, opts.url || opts.private ? { ...(opts.url ? { url: opts.url } : {}), ...(opts.private ? { private: true } : {}) } : {});
      // Until the state lists it, the placeholder stands for the real tab.
      op.tab = { ...tab, id: tabId };
      ops.value = [...ops.value];
      settle(op, true);
      resolve(tabId);
      return tabId;
    } catch (error) {
      settle(op, false);
      resolve(null);
      if (String(error).includes('catchTabLimit')) limitAsk.value = opts;
      tellRefusal(error);
      return null;
    }
  }

  /** The limit question's answer: close that tab, then open the new one. */
  async function closeForNew(tabId: string) {
    const opts = limitAsk.value ?? {};
    limitAsk.value = null;
    await closeTab(tabId);
    return newTab(opts);
  }

  function nextTab(step: 1 | -1) {
    const list = rows.value;
    const index = list.findIndex(r => r.active);
    const next = list[(index + step + list.length) % list.length];
    if (next) void selectTab(next.tab.id);
  }

  // Side list: open or folded to icons; the toggle's choice is remembered.
  const sideOpen = ref(false);
  function initSide(width: number) {
    sideOpen.value = sideOpenDefault(width, readSide());
  }
  function toggleSide() {
    sideOpen.value = !sideOpen.value;
    try {
      localStorage.setItem(SIDE_KEY, sideOpen.value ? 'open' : 'folded');
    } catch {
      // Not stored: the choice holds for this window.
    }
  }

  // Search engine: a saved preference, changed from either field (or Settings in the main window).
  const preferencesStore = usePreferencesStore();
  const engine = computed<SearchEngine>(() => {
    const chosen = preferencesStore.preferences.searchEngine;
    return chosen && chosen in SEARCH_ENGINES ? chosen : 'duckduckgo';
  });
  const engineName = computed(() => SEARCH_ENGINES[engine.value].name);
  /** Which field's engine list is open. */
  const engineMenu = ref<'home' | 'address' | null>(null);
  /** The one-time "Searching with …" line. */
  const searchHint = ref(false);
  function applySearchChoice(saved: CatchSearchChoice | null) {
    if (!saved) return;
    preferencesStore.preferences.searchEngine = saved.engine;
    preferencesStore.preferences.searchHintShown = saved.hintShown;
  }
  /** Settings may have changed it since this window loaded; unreadable keeps what is known. */
  async function loadPreferences() {
    try {
      applySearchChoice(await catchSearchEngineGet());
    } catch {
      // Search still works with the engine already known.
    }
  }
  /** The field shows the new engine now; a failed save puts the old one back and says so. */
  async function setEngine(next: SearchEngine) {
    const before = engine.value;
    if (next === before) return;
    preferencesStore.preferences.searchEngine = next;
    try {
      applySearchChoice(await catchSearchEngineSet({ engine: next }));
    } catch {
      if (preferencesStore.preferences.searchEngine === next) preferencesStore.preferences.searchEngine = before;
      useToastStore().showToast(nzI18n.global.t('feedback.settingFailed'), { style: 'error' });
    }
  }
  function showSearchHint() {
    if (preferencesStore.preferences.searchHintShown) return;
    searchHint.value = true;
    preferencesStore.preferences.searchHintShown = true;
    void catchSearchEngineSet({ hintShown: true }).catch(() => {
      // Not saved: it may show once more after a restart.
    });
  }

  // Home page.
  const home = ref<CatchHome | null>(null);
  async function loadHome() {
    const id = windowId.value;
    if (!id) return;
    try {
      home.value = await catchHome(id);
    } catch {
      home.value ??= { recentSites: [], saved: [] };
    }
  }
  /** Home field and address field: an address opens, words search (in the tab in front). */
  async function open(text: string) {
    const id = windowId.value;
    if (!homeTarget(text) || !id) return false;
    if (homeTarget(text)?.search) await loadPreferences();
    const target = homeTarget(text, engine.value)!;
    searchHint.value = false;
    const pending = ops.value.find((o): o is Extract<TabOp, { kind: 'new' }> => o.kind === 'new' && o.tab.id === activeTabId.value && !o.settled && !o.placeholder);
    const opened = pending ? await pending.done : undefined;
    // The tab it was typed in failed to open: the address must not land in another tab.
    if (opened === null) return false;
    const tabId = opened ?? realTabId();
    try {
      await catchNavigate(id, { url: target.url }, tabId);
      if (target.search) showSearchHint();
      return true;
    } catch (error) {
      console.warn('catch command failed', error);
      tellRefusal(error);
      return false;
    }
  }

  // Site tools.
  const signin = ref<{ host: string; status: 'busy' | 'done' | 'error'; error?: ReturnType<typeof signinErrorKey>; browser?: string } | null>(null);
  const signinOff = ref<string[]>([]);
  /** The list is read from Rust before any sign-in is copied by itself: never on a guess. */
  const signinOffReady = ref(false);
  let signinOffLoading: Promise<void> | null = null;
  const offEdits = new Map<string, boolean>();
  let offEdit = 0;
  function loadSigninOff() {
    signinOffLoading ??= catchSigninOffGet(readSigninOff()).then((sites) => {
      // Switch changes made while the list was on its way stay shown (their saves follow this read).
      signinOff.value = [...offEdits].reduce((list, [site, off]) => [...list.filter(h => !signinOffFor([h], site)), ...(off ? [site] : [])],
        Array.isArray(sites) ? sites : []);
      offEdits.clear();
      signinOffReady.value = true;
      try {
        localStorage.removeItem(SIGNIN_OFF_KEY);
      } catch { /* storage unavailable: Rust ignores the old list once it has its own */ }
    }, () => {
      signinOffLoading = null; // tried again with the next change
    });
    return signinOffLoading;
  }
  void loadSigninOff();
  /** Sites this window already imported for by itself: once per site per window. */
  const autoTried = new Set<string>();
  const host = computed(() => (stateIsActive.value ? state.value?.host : activeTab.value?.host) ?? '');
  /** The switch means "use my browser sign-in for this site": on unless the user switched this site off; a private tab never takes it. */
  /** A page that did not come over https never takes a sign-in either: its cookies would travel in the clear. */
  const plainHttp = computed(() => !!((stateIsActive.value ? state.value?.url : activeTab.value?.url) ?? '').match(/^(?!https:)\w/i));
  const signinLocked = computed(() => !!activeTab.value?.private || plainHttp.value);
  const signinOn = computed(() => !signinLocked.value && signinOffReady.value && !!host.value && !signinOffFor(signinOff.value, host.value));
  /** A sign-in was imported into this site's page in this window (the tooltip names the browser). */
  const signinDone = computed(() => signin.value?.host === host.value && signin.value.status === 'done');
  const signinError = computed(() => (signin.value?.host === host.value && signin.value.status === 'error' ? signin.value.error ?? null : null));
  function setSigninOff(site: string, off: boolean) {
    const rest = signinOff.value.filter(h => !signinOffFor([h], site));
    signinOff.value = (off ? [...rest, site] : rest).slice(-MAX_SIGNIN_OFF);
    if (!signinOffReady.value) offEdits.set(site, off);
    // After the old list moved in, so the first save never drops it; a failed save lasts for this window.
    const edit = ++offEdit;
    // Resolves to whether Rust saved it.
    return loadSigninOff()
      .then(() => catchSigninOffSet(site, off))
      .then((sites) => {
        // Only the latest change's answer is shown: an earlier one never flips the switch back.
        if (Array.isArray(sites) && edit === offEdit) signinOff.value = sites;
        return true;
      })
      .catch(() => false);
  }
  /** On by default: the page asks to sign in and a browser is signed in to this site. Never in a private tab, never where the user switched it off. */
  const autoSigninDue = computed(() => stateIsActive.value && state.value?.signinHint === 'offer' && !isHome.value
    && !signinLocked.value && signinOffReady.value && !signinOffFor(signinOff.value, host.value));
  /** The offer's site as Rust names it (every host of a site is one), else the host. */
  const offeredSite = () => state.value?.signinSite ?? host.value;
  const autoSigninSite = computed(() => (autoSigninDue.value && offeredSite()) || '');
  /** `offered`, `tabId`: the offer a caller already read (the page may change while it waits). */
  async function useBrowserSignin(offered = offeredSite(), tabId = realTabId()) {
    const id = windowId.value;
    const site = host.value;
    if (!id || !site || signin.value?.status === 'busy') return false;
    signin.value = { host: site, status: 'busy' };
    try {
      const result = await catchUseBrowserSignin(id, offered, tabId);
      signin.value = { host: site, status: 'done', browser: result.browser };
      return true;
    } catch (error) {
      const key = signinErrorKey(error);
      signin.value = key ? { host: site, status: 'error', error: key } : null;
      return false;
    }
  }
  watch(autoSigninSite, (site) => {
    if (!site || autoTried.has(site)) return;
    autoTried.add(site);
    void useBrowserSignin();
  }, { flush: 'sync' });
  /** The switch: off forgets this site's sign-ins and stays off for this site; on again imports now when the page asks for a sign-in. */
  async function toggleSignin() {
    const site = host.value;
    // Until the list is read the switch's state is not known: a click would guess.
    if (signinLocked.value || !site || !signinOffReady.value) return false;
    if (!signinOn.value) {
      // Only this tab's own offer counts: after a tab switch the state is still the other tab's.
      const [offered, tabId] = [offeredSite(), realTabId()];
      const asked = () => stateIsActive.value && state.value?.signinHint === 'offer' && offeredSite() === offered && realTabId() === tabId;
      const due = asked() && !autoTried.has(offered);
      // Claimed before the switch flips, so the offer's own watcher does not import ahead of the save.
      if (due) autoTried.add(offered);
      // Saved first: Rust reads its own list before it exports, and refuses a site still off there.
      const saved = await setSigninOff(site, false);
      if (!due) return true;
      if (!saved || host.value !== site || !asked()) {
        autoTried.delete(offered);
        return true;
      }
      return useBrowserSignin(offered, tabId);
    }
    void setSigninOff(site, true);
    signin.value = null;
    return run(id => catchForgetSite(id, realTabId()));
  }
  async function toggleMobile() {
    const id = windowId.value;
    const tab = activeTab.value;
    if (!id || !tab || !tab.url) return;
    const on = !tab.mobile;
    const op = push({ kind: 'mobile', tabId: tab.id, on, settled: false, landed: next => next.tabs?.find(t => t.id === tab.id)?.mobile !== !on });
    settle(op, await catchTryMobile(id, !tab.mobile, realTabId()).then(() => true, () => false));
  }

  // "Block ads" for the site in front: shown switched at once, until the engine's state agrees.
  const adblockAsk = shallowRef<{ host: string; on: boolean } | null>(null);
  const adblockOn = computed(() => {
    const ask = adblockAsk.value;
    if (ask && ask.host === host.value) return ask.on;
    return shown.value?.adblock ?? true;
  });
  const adsBlocked = computed(() => (adblockOn.value ? shown.value?.adsBlocked ?? 0 : 0));
  watch(state, (next) => {
    const ask = adblockAsk.value;
    if (ask && next && next.host === ask.host && (next.adblock ?? true) === ask.on) adblockAsk.value = null;
  });
  async function toggleAdblock() {
    const id = windowId.value;
    const site = host.value;
    if (!id || !site || activeTab.value?.private) return;
    const on = !adblockOn.value;
    const ask = { host: site, on };
    adblockAsk.value = ask;
    const ok = await catchAdblockSet(id, on, realTabId()).then(() => true, () => false);
    if (!ok && adblockAsk.value === ask) adblockAsk.value = null;
  }

  // Shell geometry: where the page goes, and whether the shell draws over it.
  const menuOpen = ref(false);
  let lastFit = '';
  let lastSent: CatchShellFit | null = null;
  function fitShell(fit: { top: number; side: number; overlay: boolean; rtl: boolean }) {
    const id = windowId.value;
    if (!id) return;
    const sent = { ...fit, overlay: fit.overlay || pickerOpen.value || menuOpen.value || !!engineMenu.value || !!limitAsk.value };
    const key = JSON.stringify(sent);
    if (key === lastFit) return;
    lastFit = key;
    lastSent = sent;
    void catchShellFit(id, sent).catch(() => {
      // Not applied (the window was closing): the next change sends it again.
      lastFit = '';
    });
  }

  function armNudge(next: CatchState) {
    if (next.url !== nudgeUrl) {
      nudgeUrl = next.url;
      nudge.value = false;
      clearTimeout(nudgeTimer);
      nudgeTimer = undefined;
    }
    // A blank page has nothing to play (a closed window kept for reuse sits on one).
    if (!next.url || next.phase !== 'looking' || next.played) {
      clearTimeout(nudgeTimer);
      nudgeTimer = undefined;
      return;
    }
    if (!nudgeTimer && !nudge.value) {
      nudgeTimer = setTimeout(() => {
        nudgeTimer = undefined;
        nudge.value = true;
      }, CATCH_NUDGE_MS);
    }
  }

  function apply(next: CatchState) {
    // Every bar belongs to one window: a stray state for another window is not ours.
    if (state.value && next.windowId !== state.value.windowId) return;
    const navigated = !!state.value && (state.value.url !== next.url || state.value.activeTabId !== next.activeTabId);
    // The page went blank (a closed window is kept, shell loaded, for the next open): its menus close.
    // A home tab is blank all along, so states arriving for it leave an open menu alone.
    if (!next.url && state.value?.url) {
      menuOpen.value = false;
      if (engineMenu.value === 'address') engineMenu.value = null;
    }
    state.value = next;
    statesSeen += 1;
    if (ops.value.some(o => o.settled && o.landed(next))) ops.value = ops.value.filter(o => !(o.settled && o.landed(next)));
    armNudge(next);
    if (navigated) {
      // Candidate ids belong to one page: drop the old page's lists.
      pages++;
      formats.value = {};
      pickedCandidateId.value = null;
      pickedFormatId.value = null;
      pickerOpen.value = false;
      saveFailed.value = false;
    }
    if (pickedCandidateId.value && !next.candidates.some(c => c.id === pickedCandidateId.value)) pickedCandidateId.value = null;
    if (next.phase === 'found' && candidate.value && !candidate.value.alreadySaved) void loadFormats(candidate.value.id);
    if (next.phase !== 'found') pickerOpen.value = false;
  }

  async function loadFormats(candidateId: string) {
    const id = windowId.value;
    if (!id || formats.value[candidateId] || state.value?.candidates.find(c => c.id === candidateId)?.alreadySaved) return;
    formats.value[candidateId] = 'loading';
    const asked = pages;
    let list: CatchFormat[] | 'failed';
    try {
      list = await catchFormats(id, candidateId);
    } catch {
      // Save still works without a list: Rust picks the best.
      list = 'failed';
    }
    if (asked === pages) formats.value[candidateId] = list;
  }

  /** Run a command for this window; the bar never throws at the user, the next state tells. */
  async function run(command: (id: string) => Promise<unknown>): Promise<boolean> {
    const id = windowId.value;
    if (!id) return false;
    try {
      await command(id);
      return true;
    } catch (error) {
      console.warn('catch command failed', error);
      return false;
    }
  }

  const navigate = (action: CatchNavigation) => run(id => catchNavigate(id, action, realTabId()));

  function pickCandidate(candidateId: string) {
    if (pickedCandidateId.value === candidateId) return;
    pickedCandidateId.value = candidateId;
    pickedFormatId.value = null;
    void loadFormats(candidateId);
  }

  function pickFormat(formatId: string) {
    pickedFormatId.value = formatId;
  }

  async function save() {
    const id = windowId.value;
    const chosen = candidate.value;
    if (!id || !chosen || !canSave.value) return false;
    busy.value = true;
    saveFailed.value = false;
    try {
      await catchSave(id, chosen.id, format.value?.id);
      pickerOpen.value = false;
      return true;
    } catch (error) {
      console.warn('catch_save failed', error);
      saveFailed.value = true;
      return false;
    } finally {
      busy.value = false;
    }
  }

  function openPicker() {
    if (!view.value.canSave) return;
    pickerOpen.value = true;
    if (candidate.value) void loadFormats(candidate.value.id);
  }

  async function start(id: string) {
    const attempt = ++delivery;
    unlisten?.();
    const unsubscribe = await listenCatchState((next) => {
      if (next.windowId === id) apply(next);
    });
    if (attempt !== delivery) {
      unsubscribe();
      return;
    }
    unlisten = unsubscribe;
    unlistenShortcut?.();
    const unsubscribeShortcut = await listenCatchShortcut((what) => {
      if (what === 'address') addressAsks.value++;
      else if (what === 'tabLimit') limitAsk.value = {};
      else if (typeof what === 'object' && typeof what?.tabLimit === 'string') limitAsk.value = { url: what.tabLimit };
    });
    if (attempt !== delivery) {
      unsubscribeShortcut();
      return;
    }
    unlistenShortcut = unsubscribeShortcut;
    unlistenFileDownload?.();
    const unsubscribeFileDownload = await listenCatchFileDownload((notice) => {
      fileDownload.value = notice;
      clearTimeout(fileDownloadTimer);
      fileDownloadTimer = setTimeout(() => {
        fileDownload.value = null;
      }, 5000);
    });
    if (attempt !== delivery) {
      unsubscribeFileDownload();
      return;
    }
    unlistenFileDownload = unsubscribeFileDownload;
    await catchBarReady(id);
  }

  function stop() {
    delivery++;
    unlisten?.();
    unlisten = undefined;
    unlistenShortcut?.();
    unlistenShortcut = undefined;
    unlistenFileDownload?.();
    unlistenFileDownload = undefined;
    clearTimeout(fileDownloadTimer);
    fileDownload.value = null;
    clearTimeout(nudgeTimer);
    nudgeTimer = undefined;
    // Nothing is drawn over the page any more: the page gets its full area back.
    pickerOpen.value = false;
    menuOpen.value = false;
    engineMenu.value = null;
    limitAsk.value = null;
    const id = windowId.value;
    if (id && lastSent?.overlay) void catchShellFit(id, { ...lastSent, overlay: false }).catch(() => {});
    lastSent = null;
    lastFit = '';
  }

  return {
    state, shown, formats, pickerOpen, menuOpen, busy, saveFailed, nudge, windowId, view, candidates, candidate, candidateFormats, formatsLoading, format, canSave,
    tabs, rows, activeTab, activeTabId, isHome, atLimit, limitAsk, addressAsks, sideOpen, home, signin, signinOn, signinDone, signinLocked, signinOffReady, signinError, host,
    apply, start, stop, fileDownload, loadFormats, pickCandidate, pickFormat, save, openPicker,
    selectTab, closeTab, newTab, closeForNew, nextTab, initSide, engine, engineName, engineMenu, searchHint, loadPreferences, setEngine, toggleSide, loadHome, useBrowserSignin, toggleSignin, toggleMobile, adblockOn, adsBlocked, toggleAdblock, fitShell,
    cancelLimit: () => {
      limitAsk.value = null;
    },
    closePicker: () => {
      pickerOpen.value = false;
    },
    back: () => navigate('back'),
    forward: () => navigate('forward'),
    reload: () => navigate('reload'),
    go: open,
    allowPopupOnce: () => run(id => catchAllowPopupOnce(id, realTabId())),
    forgetSite: () => {
      signin.value = null;
      return run(id => catchForgetSite(id, realTabId()));
    },
    deepRetry: () => run(id => catchDeepRetry(id, realTabId())),
    showSaved: (downloadId: string) => run(id => catchShowDownload(id, downloadId)),
    close: () => run(catchClose),
    showDownload: () => run(catchShowDownload),
    report: () => run(catchReport),
  };
});
