import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { emit } from '@tauri-apps/api/event';
import { clearMocks, mockWindows } from '@tauri-apps/api/mocks';
import { installTauriMock } from '../utils/tauriMock';
import { addressDir, addressToUrl, CATCH_NUDGE_MS, catchView, homeTarget, pickerStart, searchUrl, sideOpenDefault, signinErrorKey, signinOffFor, tabRows, useCatchStore } from '../../src/stores/catch';
import { usePreferencesStore } from '../../src/stores/preferences';
import { useToastStore } from '../../src/stores/toast';
import en from '../../src/locales/en.json';
import { catchShellFit, catchShowDownload } from '../../src/tauri/listeners/catch';
import { CATCH_SHORTCUT_EVENT, CATCH_STATE_EVENT, type CatchCandidate, type CatchPhase, type CatchState, type CatchTab } from '../../src/tauri/types/catch';

const MAIN: CatchCandidate = { id: 'c-main', kind: 'hls', title: 'Coastal drive', durationSec: 768, main: true };
const AD: CatchCandidate = { id: 'c-ad', kind: 'direct', durationSec: 15, main: false };

function catchState(over: Partial<CatchState> = {}): CatchState {
  return {
    windowId: 'catch-1', url: 'https://example.com/watch/1', host: 'example.com', canBack: false, canForward: false, loading: false,
    phase: 'looking', popupsBlocked: 0, played: false, playedSec: 0, candidates: [], firstRun: false, ...over,
  };
}

describe('phase → strip', () => {
  it.each<[CatchPhase, Partial<CatchState>, string, string, boolean, string[]]>([
    ['looking', {}, 'hint', 'hint', false, []],
    ['signIn', {}, 'signIn', 'amber', false, []],
    ['adPlaying', { candidates: [AD] }, 'adPlaying', 'neutral', false, []], // short clips never turn the bar green
    ['found', { candidates: [MAIN, AD] }, 'found', 'ok', true, []],
    ['drm', { candidates: [MAIN] }, 'drm', 'red', false, ['close']],
    ['live', { candidates: [MAIN] }, 'live', 'neutral', false, []],
    ['notFound', { played: true, playedSec: 12 }, 'notFound', 'neutral', false, ['reload', 'report']],
    ['saving', { candidates: [MAIN], savedDownloadId: 'd1' }, 'saving', 'ok', false, ['showDownload', 'close']],
    ['crashed', { candidates: [MAIN] }, 'crashed', 'amber', false, ['reload']],
    ['geoBlocked', {}, 'geoBlocked', 'amber', false, ['reload']],
  ])('%s', (phase, over, kind, tone, canSave, actions) => {
    expect(catchView(catchState({ phase, ...over }), false)).toEqual({ kind, tone, canSave, actions });
  });

  it('found without a main candidate cannot save', () => {
    expect(catchView(catchState({ phase: 'found', candidates: [AD] }), false).canSave).toBe(false);
  });

  it('before the first state the hint shows; the nudge only while nothing played', () => {
    expect(catchView(null, false).kind).toBe('hint');
    expect(catchView(catchState(), true).kind).toBe('nudge');
    expect(catchView(catchState({ played: true }), true).kind).toBe('hint');
  });

  it.each([
    ['example.com/watch?v=1', 'https://example.com/watch?v=1'],
    ['http://example.com', 'http://example.com/'],
    ['  https://x.org/a  ', 'https://x.org/a'],
    ['javascript:alert(1)', null],
    ['file:///etc/passwd', null],
    ['two words', null],
    ['', null],
  ])('address %j → %j', (typed, url) => {
    expect(addressToUrl(typed)).toBe(url);
  });
});

describe('catch store', () => {
  let calls: { cmd: string; args: unknown }[];
  let formats: unknown;
  let saveFails: boolean;
  let formatsReply: Promise<unknown> | undefined;

  beforeEach(() => {
    setActivePinia(createPinia());
    calls = [];
    formats = [
      { id: 'v1080', label: 'video', height: 1080, ext: 'mp4', best: true },
      { id: 'a', label: 'audio', ext: 'm4a', best: false },
    ];
    saveFails = false;
    formatsReply = undefined;
    const record = (cmd: string, args: unknown) => {
      calls.push({ cmd, args });
      return null;
    };
    mockWindows('catch-bar-1');
    installTauriMock({
      ...Object.fromEntries(['catch_navigate', 'catch_allow_popup_once', 'catch_forget_site', 'catch_deep_retry', 'catch_close', 'catch_show_download', 'catch_report', 'catch_bar_ready']
        .map(cmd => [cmd, record])),
      catch_formats: (cmd, args) => {
        record(cmd, args);
        return formatsReply ?? formats;
      },
      catch_save: (cmd, args) => {
        record(cmd, args);
        return saveFails ? Promise.reject(new Error('disk full')) : { downloadId: 'd1' };
      },
    });
  });
  afterEach(() => {
    clearMocks();
    vi.useRealTimers();
  });
  const sent = (cmd: string) => calls.filter(c => c.cmd === cmd).map(c => c.args);

  it('follows catch://state and ignores another window', async () => {
    const store = useCatchStore();
    await store.start('catch-1');
    expect(sent('catch_bar_ready')).toEqual([{ windowId: 'catch-1' }]);
    await emit(CATCH_STATE_EVENT, catchState({ popupsBlocked: 2 }));
    expect(store.state?.popupsBlocked).toBe(2);
    await emit(CATCH_STATE_EVENT, catchState({ windowId: 'catch-2', popupsBlocked: 9 }));
    expect(store.state?.popupsBlocked).toBe(2);
    store.stop();
  });

  it('catch://shortcut "address" asks the bar to focus its field, and stops after stop()', async () => {
    const store = useCatchStore();
    await store.start('catch-1');
    await emit(CATCH_SHORTCUT_EVENT, 'address');
    expect([store.addressAsks, store.limitAsk]).toEqual([1, null]);
    store.stop();
    await emit(CATCH_SHORTCUT_EVENT, 'address');
    expect(store.addressAsks).toBe(1);
  });

  it('catch://shortcut "tabLimit" shows the six-tab question', async () => {
    const store = useCatchStore();
    await store.start('catch-1');
    await emit(CATCH_SHORTCUT_EVENT, 'tabLimit');
    expect([store.addressAsks, store.limitAsk]).toEqual([0, {}]);
    store.stop();
  });

  it('a link from the main window that finds six tabs asks which tab to close, and keeps the link for the new tab', async () => {
    const store = useCatchStore();
    await store.start('catch-1');
    await emit(CATCH_SHORTCUT_EVENT, { tabLimit: 'https://example.com/watch' });
    expect(store.limitAsk).toEqual({ url: 'https://example.com/watch' });
    store.stop();
  });

  it('sends every bar command with its window id', async () => {
    const store = useCatchStore();
    store.apply(catchState());
    await store.back();
    await store.forward();
    await store.reload();
    await store.go('example.org/v/2');
    await store.allowPopupOnce();
    await store.forgetSite();
    await store.deepRetry();
    await store.showDownload();
    await store.report();
    await store.close();
    expect(sent('catch_navigate')).toEqual([
      { windowId: 'catch-1', action: 'back' },
      { windowId: 'catch-1', action: 'forward' },
      { windowId: 'catch-1', action: 'reload' },
      { windowId: 'catch-1', action: { url: 'https://example.org/v/2' } },
    ]);
    for (const cmd of ['catch_allow_popup_once', 'catch_forget_site', 'catch_deep_retry', 'catch_show_download', 'catch_report', 'catch_close']) {
      expect(sent(cmd), cmd).toEqual([{ windowId: 'catch-1' }]);
    }
  });

  it('marks saved candidates without fetching formats or saving them again', async () => {
    const store = useCatchStore();
    store.apply(catchState({ phase: 'found', candidates: [{ ...MAIN, alreadySaved: true }] }));
    expect(store.canSave).toBe(false);
    expect(await store.save()).toBe(false);
    expect(sent('catch_formats')).toEqual([]);
    expect(sent('catch_save')).toEqual([]);
  });

  it('a typed address that is not a web page is never sent', async () => {
    const store = useCatchStore();
    store.apply(catchState());
    expect(await store.go('javascript:alert(1)')).toBe(false);
    expect(sent('catch_navigate')).toEqual([]);
  });

  it('found: reads the formats of the main video once, saves the best, or the picked one', async () => {
    const store = useCatchStore();
    store.apply(catchState({ phase: 'found', candidates: [AD, MAIN] }));
    await vi.waitFor(() => expect(store.candidateFormats).toHaveLength(2));
    store.apply(catchState({ phase: 'found', candidates: [AD, MAIN], played: true }));
    expect(sent('catch_formats')).toEqual([{ windowId: 'catch-1', candidateId: 'c-main' }]);
    expect(store.format?.id).toBe('v1080');

    await store.save();
    store.pickFormat('a');
    await store.save();
    expect(sent('catch_save')).toEqual([
      { windowId: 'catch-1', candidateId: 'c-main', formatId: 'v1080' },
      { windowId: 'catch-1', candidateId: 'c-main', formatId: 'a' },
    ]);
  });

  it('a picked other video saves that video', async () => {
    const store = useCatchStore();
    store.apply(catchState({ phase: 'found', candidates: [MAIN, AD] }));
    formats = [];
    store.pickCandidate('c-ad');
    await vi.waitFor(() => expect(sent('catch_formats')).toHaveLength(2));
    await store.save();
    expect(sent('catch_save').at(-1)).toEqual({ windowId: 'catch-1', candidateId: 'c-ad', formatId: null });
  });

  it.each<CatchPhase>(['drm', 'live', 'saving', 'adPlaying', 'looking'])('Save does nothing in %s', async (phase) => {
    const store = useCatchStore();
    store.apply(catchState({ phase, candidates: [MAIN] }));
    expect(store.canSave).toBe(false);
    store.openPicker();
    expect(store.pickerOpen).toBe(false);
    expect(await store.save()).toBe(false);
    expect(sent('catch_save')).toEqual([]);
  });

  it('a refused save says so and keeps the picker open', async () => {
    const store = useCatchStore();
    store.apply(catchState({ phase: 'found', candidates: [MAIN] }));
    store.openPicker();
    saveFails = true;
    expect(await store.save()).toBe(false);
    expect(store.saveFailed).toBe(true);
    expect(store.pickerOpen).toBe(true);
  });

  it('navigating drops the format lists of the previous page', async () => {
    const store = useCatchStore();
    store.apply(catchState({ phase: 'found', candidates: [MAIN] }));
    await vi.waitFor(() => expect(store.candidateFormats).toHaveLength(2));
    store.apply(catchState({ url: 'https://example.com/watch/2' }));
    expect(store.formats).toEqual({});
    // A reused window: the same candidate id on the next page asks again.
    store.apply(catchState({ url: 'https://example.com/watch/2', phase: 'found', candidates: [MAIN] }));
    await vi.waitFor(() => expect(sent('catch_formats')).toHaveLength(2));

    // The answer for a page the bar has left arrives late: it is not the next page's list.
    let refuse!: (reason: Error) => void;
    formatsReply = new Promise((_, reject) => {
      refuse = reject;
    });
    store.apply(catchState({ url: 'https://example.com/watch/3', phase: 'found', candidates: [MAIN] }));
    await vi.waitFor(() => expect(sent('catch_formats')).toHaveLength(3));
    formatsReply = undefined;
    store.apply(catchState({ url: 'https://example.com/watch/4' }));
    refuse(new Error('catchCandidateUnavailable'));
    await new Promise(resolve => setTimeout(resolve));
    expect(store.formats).toEqual({});
    store.apply(catchState({ url: 'https://example.com/watch/4', phase: 'found', candidates: [MAIN] }));
    await vi.waitFor(() => expect(store.candidateFormats).toHaveLength(2));
  });

  it('leaving found closes the picker; navigating forgets the picks', () => {
    const store = useCatchStore();
    store.apply(catchState({ phase: 'found', candidates: [MAIN, AD] }));
    store.openPicker();
    store.pickCandidate('c-ad');
    store.apply(catchState({ phase: 'drm', candidates: [MAIN, AD] }));
    expect(store.pickerOpen).toBe(false);
    expect(store.candidate?.id).toBe('c-ad');
    store.apply(catchState({ url: 'https://example.com/watch/2', phase: 'found', candidates: [MAIN, AD] }));
    expect(store.candidate?.id).toBe('c-main');
  });

  it('nudges after 20 s of nothing played, not after playing, and starts over on a new page', () => {
    vi.useFakeTimers();
    const store = useCatchStore();
    store.apply(catchState());
    vi.advanceTimersByTime(CATCH_NUDGE_MS - 1);
    expect(store.view.kind).toBe('hint');
    vi.advanceTimersByTime(1);
    expect(store.view.kind).toBe('nudge');

    store.apply(catchState({ url: 'https://example.com/watch/2' }));
    expect(store.view.kind).toBe('hint');
    store.apply(catchState({ url: 'https://example.com/watch/2', played: true, playedSec: 3 }));
    vi.advanceTimersByTime(CATCH_NUDGE_MS * 2);
    expect(store.view.kind).toBe('hint');

    // A blank page has nothing to play: no nudge, also not while a closed window waits for its next use.
    store.apply(catchState({ url: '', host: '' }));
    vi.advanceTimersByTime(CATCH_NUDGE_MS * 2);
    expect(store.view.kind).toBe('hint');
  });
});

const tab = (id: string, over: Partial<CatchTab> = {}): CatchTab => ({
  id, url: `https://${id}.example/watch`, host: `${id}.example`, loading: false, phase: 'looking', found: false, private: false, asleep: false, mobile: false, ...over,
});
const withTabs = (tabs: CatchTab[], activeTabId: string, over: Partial<CatchState> = {}) => {
  const active = tabs.find(t => t.id === activeTabId)!;
  return catchState({ url: active.url, host: active.host, tabs, activeTabId, ...over });
};

describe('tab list', () => {
  it('lists linked tabs under their opener and marks active, found, private and asleep', () => {
    const tabs = [tab('a', { found: true }), tab('b', { private: true, asleep: true }), tab('login', { openerId: 'a' }), tab('orphan', { openerId: 'gone' }), tab('home', { url: '', host: '' })];
    expect(tabRows(tabs, 'login').map(r => [r.tab.id, r.active, r.linked, r.letter, r.home, r.tab.found, r.tab.private, r.tab.asleep])).toEqual([
      ['a', false, false, 'A', false, true, false, false],
      ['login', true, true, 'L', false, false, false, false],
      ['b', false, false, 'B', false, false, true, true],
      ['orphan', false, false, 'O', false, false, false, false],
      ['home', false, false, '', true, false, false, false],
    ]);
  });

  it.each([
    [880, null, false], [1100, null, true], [1280, null, true], [880, true, true], [1400, false, false],
  ])('side list at %i px, remembered %s → open %s', (width, remembered, open) => {
    expect(sideOpenDefault(width, remembered)).toBe(open);
  });

  it.each([
    ['example.com/watch?v=1', 'https://example.com/watch?v=1', false],
    ['https://x.org/a', 'https://x.org/a', false],
    ['localhost:3000/v', 'https://localhost:3000/v', false],
    ['  https://example.com/v  ', 'https://example.com/v', false],
    ['salt road documentary', searchUrl('salt road documentary'), true],
    ['طريق الملح', searchUrl('طريق الملح'), true],
    ['cats', searchUrl('cats'), true],
    ['v1.2', searchUrl('v1.2'), true],
  ])('home field %j → %j', (typed, url, search) => {
    expect(homeTarget(typed)).toEqual({ url, search });
  });

  it.each([
    ['duckduckgo', 'salt & road', 'https://duckduckgo.com/?q=salt%20%26%20road'],
    ['google', 'salt & road', 'https://www.google.com/search?q=salt%20%26%20road'],
    ['bing', 'طريق', `https://www.bing.com/search?q=${encodeURIComponent('طريق')}`],
    ['brave', 'cats $& $x', `https://search.brave.com/search?q=${encodeURIComponent('cats $& $x')}`],
  ] as const)('%s: words search there, a domain stays a URL', (engine, words, url) => {
    expect(homeTarget(words, engine)).toEqual({ url, search: true });
    expect(homeTarget('example.org/v', engine)).toEqual({ url: 'https://example.org/v', search: false });
  });

  it('a tab opened by a linked tab, and tabs that name each other as opener, still get a row', () => {
    const chain = [tab('a'), tab('b', { openerId: 'a' }), tab('c', { openerId: 'b' }), tab('d')];
    expect(tabRows(chain, 'c').map(r => [r.tab.id, r.linked])).toEqual([['a', false], ['b', true], ['c', true], ['d', false]]);
    const cycle = [tab('x', { openerId: 'y' }), tab('y', { openerId: 'x' }), tab('z')];
    expect(tabRows(cycle, 'z').map(r => r.tab.id).sort()).toEqual(['x', 'y', 'z']);
  });

  // The picker starts under the quality button, and the whole sheet stays in the page area.
  it.each([
    [300, 860, 300],
    [700, 860, 432],
    [2, 860, 8],
    [250, 327, 8],
  ])('picker under a button at %i in a %i px area starts at %i', (buttonStart, areaWidth, start) => {
    expect(pickerStart(buttonStart, areaWidth)).toBe(start);
  });

  // Arabic UI: an empty field keeps the page direction (placeholder reads RTL), an address always reads LTR.
  it.each([
    ['', false, undefined],
    ['', true, undefined],
    ['example.com/watch/2', false, 'ltr'],
    ['مثال.example/فيديو', false, 'ltr'],
    ['https://example.com/watch/2', true, 'ltr'],
    ['example.org/v', true, 'ltr'],
    ['طريق الملح', true, 'auto'],
    ['salt road', true, 'auto'],
  ] as const)('address field %j (editing: %j) reads %j', (text, editing, dir) => {
    expect(addressDir(text, editing)).toBe(dir);
  });

  it.each(['', '   ', 'javascript:alert(1)', 'file:///etc/passwd'])('home field %j goes nowhere', (typed) => {
    expect(homeTarget(typed)).toBeNull();
  });

  it.each([
    ['catchSigninNone', 'signinNone'], [new Error('catchSigninDenied'), 'signinDenied'], ['catchSigninUnsupported', 'signinUnsupported'], ['catchSigninBusy', 'signinAlready'], ['boom', 'signinFailed'], [undefined, 'signinFailed'],
  ])('sign-in error %s → %s', (error, key) => {
    expect(signinErrorKey(error)).toBe(key);
  });
});

describe('catch store: tabs and shell', () => {
  let calls: { cmd: string; args: unknown }[];
  let reply: Record<string, (args: unknown) => unknown>;
  beforeEach(() => {
    setActivePinia(createPinia());
    calls = [];
    // Rust's off list (one per test): the old local list moves in on first read.
    let offSites: string[] | null = null;
    reply = {
      catch_signin_off_get: args => (offSites ??= [...(args as { legacy: string[] }).legacy]),
      catch_signin_off_set: (args) => {
        const { site, off } = args as { site: string; off: boolean };
        offSites = [...(offSites ?? []).filter(h => h !== site), ...(off ? [site] : [])];
        return offSites;
      },
    };
    localStorage.clear();
    mockWindows('catch-bar-1');
    installTauriMock(Object.fromEntries(['catch_search_engine_get', 'catch_search_engine_set', 'catch_tab_new', 'catch_tab_select', 'catch_tab_close', 'catch_shell_fit', 'catch_home', 'catch_use_browser_signin',
      'catch_try_mobile', 'catch_adblock_set', 'catch_navigate', 'catch_forget_site', 'catch_formats', 'catch_show_download', 'catch_signin_off_get', 'catch_signin_off_set'].map(cmd => [cmd, (c: string, args: unknown) => {
      calls.push({ cmd: c, args });
      return reply[c] ? reply[c](args) : null;
    }])));
  });
  afterEach(() => clearMocks());
  const sent = (cmd: string) => calls.filter(c => c.cmd === cmd).map(c => c.args);
  const ids = (store: ReturnType<typeof useCatchStore>) => store.rows.map(r => `${r.tab.id}${r.active ? '*' : ''}`);

  it('select and close show at once, then the next state decides', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a'), tab('b'), tab('c')], 'a'));
    const selecting = store.selectTab('b');
    expect(ids(store)).toEqual(['a', 'b*', 'c']);
    expect(store.view.kind).toBe('hint'); // b's own state is not here yet: no Save from a's candidates
    await selecting;
    expect(sent('catch_tab_select')).toEqual([{ windowId: 'catch-1', tabId: 'b' }]);
    store.apply(withTabs([tab('a'), tab('b'), tab('c')], 'b'));
    expect(ids(store)).toEqual(['a', 'b*', 'c']);

    const closing = store.closeTab('b');
    expect(ids(store)).toEqual(['a', 'c*']); // the tab after it comes to the front
    await closing;
    store.apply(withTabs([tab('a'), tab('c')], 'a')); // Rust chose another: it wins
    expect(ids(store)).toEqual(['a*', 'c']);
  });

  it('a failed select or close rolls back', async () => {
    const store = useCatchStore();
    reply.catch_tab_select = () => Promise.reject(new Error('catchWindowClosed'));
    reply.catch_tab_close = () => Promise.reject(new Error('catchWindowClosed'));
    store.apply(withTabs([tab('a'), tab('b')], 'a'));
    await store.selectTab('b');
    expect(ids(store)).toEqual(['a*', 'b']);
    await store.closeTab('a');
    expect(ids(store)).toEqual(['a*', 'b']);
  });

  it('closing a linked tab brings its opener; closing the last shows the home page', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a'), tab('b'), tab('login', { openerId: 'a' })], 'login'));
    void store.closeTab('login');
    expect(ids(store)).toEqual(['a*', 'b']);
    setActivePinia(createPinia());
    const single = useCatchStore();
    single.apply(withTabs([tab('a')], 'a'));
    void single.closeTab('a');
    expect(single.isHome).toBe(true);
    expect(single.rows).toHaveLength(1);
  });

  it('closing the tab in front brings the one after it, not the last', () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a'), tab('b'), tab('c')], 'a'));
    void store.closeTab('a');
    expect(ids(store)).toEqual(['b*', 'c']);
  });

  it('a close leaves a tab that is still opening alone, whether the close worked or not', async () => {
    const store = useCatchStore();
    let opened!: (value: { tabId: string }) => void;
    reply.catch_tab_new = () => new Promise(r => (opened = r));
    store.apply(withTabs([tab('a'), tab('b'), tab('c')], 'a'));
    const opening = store.newTab();
    await store.closeTab('b');
    store.apply(withTabs([tab('a'), tab('c')], 'a'));
    expect(store.rows.map(r => r.tab.id.replace(/\d+$/, ''))).toEqual(['a', 'c', 'pending-']);
    expect(store.isHome).toBe(true);
    reply.catch_tab_close = () => Promise.reject(new Error('catchWindowClosed'));
    await store.closeTab('c');
    expect(store.rows).toHaveLength(3);
    expect(store.isHome).toBe(true);
    opened({ tabId: 'n' });
    expect(await opening).toBe('n');
  });

  it('closing a tab that is still opening closes the real tab once it exists', async () => {
    const store = useCatchStore();
    let opened!: (value: { tabId: string }) => void;
    reply.catch_tab_new = () => new Promise(r => (opened = r));
    store.apply(withTabs([tab('a')], 'a'));
    void store.newTab();
    const closing = store.closeTab(store.activeTabId!);
    expect(ids(store)).toEqual(['a*']);
    opened({ tabId: 'n' });
    await closing;
    expect(sent('catch_tab_close')).toEqual([{ windowId: 'catch-1', tabId: 'n' }]);
    store.apply(withTabs([tab('a'), tab('n', { url: '', host: '' })], 'n')); // sent before the close landed
    expect(ids(store)).toEqual(['a*']);
    store.apply(withTabs([tab('a')], 'a'));
    expect(ids(store)).toEqual(['a*']);
  });

  it('after the last tab closes the home page stays through a state sent before the close', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a')], 'a'));
    await store.closeTab('a');
    store.apply(withTabs([tab('a')], 'a'));
    expect(store.isHome).toBe(true);
    expect(store.rows).toHaveLength(1);
    store.apply(withTabs([tab('h', { url: '', host: '' })], 'h'));
    expect(ids(store)).toEqual(['h*']);
  });

  it('the last tab closes into ONE home tab when the new state arrives before the close call returns', async () => {
    // Owner on t5: closing the only tab sometimes left two "New tab" rows (the stand-in and the engine's real home tab).
    const store = useCatchStore();
    store.apply(withTabs([tab('a')], 'a'));
    let returned!: () => void;
    reply.catch_tab_close = () => new Promise<void>((resolve) => {
      returned = resolve;
    });
    const closing = store.closeTab('a');
    await Promise.resolve();
    store.apply(withTabs([tab('h', { url: '', host: '' })], 'h'));
    returned();
    await closing;
    expect(ids(store)).toEqual(['h*']);
    // Selecting works on it at once: no stale op holds the old order.
    store.apply(withTabs([tab('h', { url: '', host: '' })], 'h', { loading: true }));
    expect(ids(store)).toEqual(['h*']);
  });

  it('a state sent before a select, close or new tab landed does not undo it', async () => {
    const store = useCatchStore();
    reply.catch_tab_new = () => ({ tabId: 'n' });
    const three = [tab('a'), tab('b'), tab('c')];
    store.apply(withTabs(three, 'a'));
    await store.selectTab('b');
    store.apply(withTabs(three, 'a', { loading: true }));
    expect(ids(store)).toEqual(['a', 'b*', 'c']);
    store.apply(withTabs(three, 'b'));
    await store.closeTab('c');
    store.apply(withTabs(three, 'b'));
    expect(ids(store)).toEqual(['a', 'b*']);
    store.apply(withTabs([tab('a'), tab('b')], 'b'));
    await store.newTab();
    store.apply(withTabs([tab('a'), tab('b')], 'b', { loading: true }));
    expect(ids(store)).toEqual(['a', 'b', 'n*']);
    // Once the engine's state shows something else, it wins.
    store.apply(withTabs([tab('a'), tab('b'), tab('n', { url: '', host: '' })], 'a'));
    expect(ids(store)).toEqual(['a*', 'b', 'n']);
  });

  it('typing an address in a tab that failed to open goes nowhere', async () => {
    const store = useCatchStore();
    let refuse!: (error: Error) => void;
    reply.catch_tab_new = () => new Promise((_, reject) => (refuse = reject));
    store.apply(withTabs([tab('a')], 'a'));
    void store.newTab();
    const going = store.go('example.org/v');
    refuse(new Error('catchWindowClosed'));
    expect(await going).toBe(false);
    expect(sent('catch_navigate')).toEqual([]);
  });

  it('picking or closing a tab answers the six-tab question; stopping clears what was drawn over the page', async () => {
    const store = useCatchStore();
    const six = ['a', 'b', 'c', 'd', 'e', 'f'].map(id => tab(id));
    store.apply(withTabs(six, 'a'));
    await store.newTab();
    expect(store.limitAsk).not.toBeNull();
    await store.selectTab('b');
    expect(store.limitAsk).toBeNull();
    await store.newTab();
    await store.closeTab('c');
    expect(store.limitAsk).toBeNull();

    store.menuOpen = true;
    store.engineMenu = 'address';
    store.fitShell({ top: 88, side: 48, overlay: false, rtl: false });
    store.stop();
    await Promise.resolve();
    expect([store.menuOpen, store.engineMenu, store.pickerOpen, store.limitAsk]).toEqual([false, null, false, null]);
    expect(sent('catch_shell_fit').at(-1)).toEqual({ windowId: 'catch-1', fit: { top: 88, side: 48, overlay: false, rtl: false } });
  });

  it('the ⋯ menu stays open on a home tab while states arrive, and closes when the window goes blank', () => {
    const store = useCatchStore();
    const tabs = [tab('h', { url: '', host: '' }), tab('b')];
    store.apply(withTabs(tabs, 'h'));
    store.menuOpen = true;
    store.apply(withTabs([tabs[0], tab('b', { loading: true })], 'h'));
    expect(store.menuOpen).toBe(true);
    setActivePinia(createPinia());
    const page = useCatchStore();
    page.apply(catchState());
    page.menuOpen = true;
    page.engineMenu = 'address';
    page.apply(catchState({ url: '', host: '' }));
    expect([page.menuOpen, page.engineMenu]).toEqual([false, null]);
  });

  it('show-download and shell-fit keep their payload shapes', async () => {
    await catchShowDownload('catch-1');
    await catchShowDownload('catch-1', 'download-9');
    await catchShellFit('catch-1', { top: 88, side: 240, overlay: true, rtl: true });
    expect(sent('catch_show_download')).toEqual([{ windowId: 'catch-1' }, { windowId: 'catch-1', downloadId: 'download-9' }]);
    expect(sent('catch_shell_fit')).toEqual([{ windowId: 'catch-1', fit: { top: 88, side: 240, overlay: true, rtl: true } }]);
  });

  it('new tab opens the home page at once and becomes the real tab', async () => {
    const store = useCatchStore();
    reply.catch_tab_new = () => ({ tabId: 't2' });
    store.apply(withTabs([tab('a')], 'a'));
    const opening = store.newTab({ private: true });
    expect(store.isHome).toBe(true);
    expect(store.rows.at(-1)).toMatchObject({ active: true, home: true, tab: { private: true } });
    expect(await opening).toBe('t2');
    expect(sent('catch_tab_new')).toEqual([{ windowId: 'catch-1', opts: { private: true } }]);
    store.apply(withTabs([tab('a'), tab('t2', { url: '', host: '', private: true })], 't2'));
    expect(ids(store)).toEqual(['a', 't2*']);
  });

  it('at six tabs New tab asks which one to close, then opens the new tab', async () => {
    const store = useCatchStore();
    reply.catch_tab_new = () => ({ tabId: 'n' });
    const six = ['a', 'b', 'c', 'd', 'e', 'f'].map(id => tab(id));
    store.apply(withTabs(six, 'a'));
    expect(await store.newTab({ url: 'https://x.example/' })).toBeNull();
    expect(store.limitAsk).toEqual({ url: 'https://x.example/' });
    expect(sent('catch_tab_new')).toEqual([]);
    expect(await store.closeForNew('c')).toBe('n');
    expect(sent('catch_tab_close')).toEqual([{ windowId: 'catch-1', tabId: 'c' }]);
    expect(sent('catch_tab_new')).toEqual([{ windowId: 'catch-1', opts: { url: 'https://x.example/' } }]);
    expect(store.limitAsk).toBeNull();
  });

  it('the engine refusing at the limit asks the same question', async () => {
    const store = useCatchStore();
    reply.catch_tab_new = () => Promise.reject(new Error('catchTabLimit'));
    store.apply(withTabs([tab('a')], 'a'));
    expect(await store.newTab()).toBeNull();
    expect(store.limitAsk).toEqual({});
    expect(ids(store)).toEqual(['a*']);
  });

  it('the fold follows the window width until the user picks, then remembers', () => {
    const store = useCatchStore();
    store.initSide(880);
    expect(store.sideOpen).toBe(false);
    store.initSide(1280);
    expect(store.sideOpen).toBe(true);
    store.toggleSide();
    setActivePinia(createPinia());
    const next = useCatchStore();
    next.initSide(1600);
    expect(next.sideOpen).toBe(false);
  });

  it('home field: words search, an address opens, in the tab in front', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a'), tab('h', { url: '', host: '' })], 'h', { url: '', host: '' }));
    expect(await store.go('salt road')).toBe(true);
    expect(await store.go('example.org/v/2')).toBe(true);
    expect(await store.go('javascript:alert(1)')).toBe(false);
    expect(sent('catch_navigate')).toEqual([
      { windowId: 'catch-1', action: { url: 'https://duckduckgo.com/?q=salt%20road' }, tabId: 'h' },
      { windowId: 'catch-1', action: { url: 'https://example.org/v/2' }, tabId: 'h' },
    ]);
  });

  it('an unusable Settings proxy: no page loads, and a toast says why', async () => {
    const store = useCatchStore();
    const toasts = useToastStore();
    store.apply(withTabs([tab('h', { url: '', host: '' })], 'h', { url: '', host: '' }));
    reply.catch_navigate = () => Promise.reject(new Error('catchOpenFailed'));
    expect(await store.go('example.org')).toBe(false);
    expect(toasts.toasts).toEqual([]);
    reply.catch_navigate = () => Promise.reject(new Error('catchProxyUnusable'));
    reply.catch_tab_new = () => Promise.reject(new Error('catchProxyUnusable'));
    expect(await store.go('example.org')).toBe(false);
    expect(await store.newTab({ url: 'https://example.org/' })).toBeNull();
    expect(toasts.toasts.map(toast => [toast.message, toast.style])).toEqual(Array(2).fill([en.catch.proxyUnusable, 'error']));
  });

  it('the first search ever shows the engine line once and saves that it did', async () => {
    const store = useCatchStore();
    // The default preferences object is shared between pinia instances: start unseen.
    usePreferencesStore().preferences.searchHintShown = false;
    store.apply(withTabs([tab('h', { url: '', host: '' })], 'h', { url: '', host: '' }));
    await store.go('example.org');
    expect(store.searchHint).toBe(false);
    await store.go('salt road');
    expect(store.searchHint).toBe(true);
    await vi.waitFor(() => expect(sent('catch_search_engine_set')).toEqual([{ hintShown: true }]));
    await store.go('salt road again');
    expect(store.searchHint).toBe(false);
    expect(sent('catch_search_engine_set')).toHaveLength(1);
  });

  it('the shell reads and saves its engine through its own two commands; a failed save puts the old engine back', async () => {
    const store = useCatchStore();
    const prefs = usePreferencesStore().preferences;
    prefs.searchEngine = 'duckduckgo';
    reply.catch_search_engine_get = () => ({ engine: 'google', hintShown: true });
    await store.loadPreferences();
    expect([store.engine, prefs.searchHintShown]).toEqual(['google', true]);
    reply.catch_search_engine_set = () => ({ engine: 'brave', hintShown: true });
    await store.setEngine('brave');
    expect(store.engine).toBe('brave');
    reply.catch_search_engine_set = () => Promise.reject(new Error('catchPreferencesFailed'));
    await store.setEngine('bing');
    expect(store.engine).toBe('brave');
    expect(sent('catch_search_engine_set')).toEqual([{ engine: 'brave' }, { engine: 'bing' }]);
    expect(calls.some(c => c.cmd.startsWith('preferences_'))).toBe(false);
  });

  it.each([
    ['catchSigninNone', 'signinNone'], ['catchSigninDenied', 'signinDenied'], ['catchSigninUnsupported', 'signinUnsupported'],
  ])('browser sign-in error %s shows its message', async (code, key) => {
    const store = useCatchStore();
    reply.catch_use_browser_signin = () => Promise.reject(code);
    store.apply(withTabs([tab('a')], 'a'));
    expect(await store.useBrowserSignin()).toBe(false);
    expect(store.signinError).toBe(key);
  });

  it.each(['catchLeftSite', 'catchWindowClosed'])('browser sign-in ends quietly on %s', async (code) => {
    const store = useCatchStore();
    reply.catch_use_browser_signin = () => Promise.reject(code);
    store.apply(withTabs([tab('a')], 'a'));
    expect(await store.useBrowserSignin()).toBe(false);
    expect(store.signinError).toBeNull();
  });

  it('browser sign-in is on by default: the offer imports once per site, never in a private tab or where it was switched off', async () => {
    const store = useCatchStore();
    reply.catch_use_browser_signin = () => ({ imported: 4, browser: 'Chrome' });
    const offer = (id: string) => store.apply(withTabs([tab(id)], id, { signinHint: 'offer' }));
    // Offer → import, with no question; the same offer again imports nothing more.
    offer('a');
    await vi.waitFor(() => expect(store.signinDone).toBe(true));
    offer('a');
    expect(sent('catch_use_browser_signin')).toEqual([{ windowId: 'catch-1', tabId: 'a', site: tab('a').host }]);
    // Switched off: forgotten, remembered by Rust (the hidden page reads it), and a later offer imports nothing.
    await store.toggleSignin();
    expect(sent('catch_forget_site')).toEqual([{ windowId: 'catch-1', tabId: 'a' }]);
    await vi.waitFor(() => expect(sent('catch_signin_off_set')).toEqual([{ site: tab('a').host, off: true }]));
    setActivePinia(createPinia());
    const next = useCatchStore();
    // Until the list is read, nothing is imported by itself.
    next.apply(withTabs([tab('a')], 'a', { signinHint: 'offer' }));
    await vi.waitFor(() => expect(sent('catch_signin_off_get')).toHaveLength(2));
    next.apply(withTabs([tab('a')], 'a', { signinHint: 'offer' }));
    await vi.waitFor(() => expect(next.signinOffReady).toBe(true));
    expect(next.signinOn).toBe(false);
    // A private tab never gets the browser's sign-in.
    next.apply(withTabs([tab('b', { private: true })], 'b', { signinHint: 'offer' }));
    expect(sent('catch_use_browser_signin')).toHaveLength(1);
    // Switched on again on the sign-in wall: imports, and the site is no longer remembered as off.
    next.apply(withTabs([tab('a')], 'a', { signinHint: 'offer' }));
    await next.toggleSignin();
    await vi.waitFor(() => expect(sent('catch_use_browser_signin')).toHaveLength(2));
    await vi.waitFor(() => expect(sent('catch_signin_off_set')).toHaveLength(2));
    expect(sent('catch_signin_off_set')[1]).toEqual({ site: tab('a').host, off: false });
  });

  it('the import names the offered site, runs once per site (not per host), and switching on saves before it imports', async () => {
    const store = useCatchStore();
    reply.catch_use_browser_signin = () => ({ imported: 4, browser: 'Chrome' });
    const offer = (id: string, host: string) => store.apply(withTabs([tab(id, { host })], id, { host, signinHint: 'offer', signinSite: 'example.com' }));
    await vi.waitFor(() => expect(store.signinOffReady).toBe(true));
    offer('a', 'www.example.com');
    await vi.waitFor(() => expect(store.signinDone).toBe(true));
    // Rust refuses the import when the page is no longer on this site.
    expect(sent('catch_use_browser_signin')).toEqual([{ windowId: 'catch-1', tabId: 'a', site: 'example.com' }]);
    // Another host of the same site in this window: no second import.
    offer('b', 'm.example.com');
    expect(sent('catch_use_browser_signin')).toHaveLength(1);

    // Switched off, then on again at a wall: the off list is saved before the import starts.
    reply.catch_signin_off_get = () => [tab('a').host];
    setActivePinia(createPinia());
    const next = useCatchStore();
    await vi.waitFor(() => expect(next.signinOffReady).toBe(true));
    next.apply(withTabs([tab('a')], 'a', { signinHint: 'offer', signinSite: tab('a').host }));
    let saved = (_: string[]) => {};
    reply.catch_signin_off_set = () => new Promise<string[]>(resolve => saved = resolve);
    calls.length = 0;
    const toggled = next.toggleSignin();
    await vi.waitFor(() => expect(sent('catch_signin_off_set')).toEqual([{ site: tab('a').host, off: false }]));
    expect(sent('catch_use_browser_signin')).toEqual([]);
    saved([]);
    await toggled;
    expect(sent('catch_use_browser_signin')).toEqual([{ windowId: 'catch-1', tabId: 'a', site: tab('a').host }]);
  });

  it('switching on right after a tab switch never imports on the other tab\'s offer', async () => {
    reply.catch_use_browser_signin = () => ({ imported: 4, browser: 'Chrome' });
    reply.catch_signin_off_get = () => ['a.example'];
    const store = useCatchStore();
    await vi.waitFor(() => expect(store.signinOffReady).toBe(true));
    // Tab a has an offer (its site is off); tab b is on the same site and shows no wall.
    store.apply(withTabs([tab('a'), tab('b', { host: 'www.a.example' })], 'a', { signinHint: 'offer', signinSite: 'a.example' }));
    void store.selectTab('b');
    expect(store.signinOn).toBe(false);
    await store.toggleSignin();
    expect(store.signinOn).toBe(true);
    expect(sent('catch_use_browser_signin')).toEqual([]);
  });

  it('a plain-http page never takes the browser sign-in: no import on its offer, and the switch reads off', async () => {
    reply.catch_use_browser_signin = () => ({ imported: 4, browser: 'Chrome' });
    const store = useCatchStore();
    await vi.waitFor(() => expect(store.signinOffReady).toBe(true));
    const plain = tab('a', { url: 'http://a.example/login' });
    store.apply(withTabs([plain], 'a', { signinHint: 'offer', signinSite: 'a.example' }));
    expect(store.signinOn).toBe(false);
    expect(await store.toggleSignin()).toBe(false);
    expect(sent('catch_use_browser_signin')).toEqual([]);
    expect(sent('catch_signin_off_set')).toEqual([]);
    // The same site over https still does.
    store.apply(withTabs([tab('a')], 'a', { signinHint: 'offer', signinSite: 'a.example' }));
    await vi.waitFor(() => expect(sent('catch_use_browser_signin')).toHaveLength(1));
  });

  it('the old local off list moves into Rust once, and its sites stay off', async () => {
    localStorage.setItem('nz-catch-signin-off', JSON.stringify([tab('a').host, 42]));
    reply.catch_use_browser_signin = () => ({ imported: 4, browser: 'Chrome' });
    const store = useCatchStore();
    await vi.waitFor(() => expect(localStorage.getItem('nz-catch-signin-off')).toBeNull());
    expect(sent('catch_signin_off_get')).toEqual([{ legacy: [tab('a').host] }]);
    store.apply(withTabs([tab('a')], 'a', { signinHint: 'offer' }));
    expect(sent('catch_use_browser_signin')).toEqual([]);
  });

  it('browser sign-in switch: on by default before any import; off forgets the site; private tab is off and locked', async () => {
    const store = useCatchStore();
    reply.catch_use_browser_signin = () => ({ imported: 4, browser: 'Chrome' });
    store.apply(withTabs([tab('a'), tab('b')], 'a'));
    // Nothing imported yet (the page did not ask): the switch still reads on.
    await vi.waitFor(() => expect(store.signinOn).toBe(true));
    await store.toggleSignin();
    expect(store.signinOn).toBe(false);
    expect(sent('catch_forget_site')).toEqual([{ windowId: 'catch-1', tabId: 'a' }]);
    // On again: no import while the page asks for nothing; with a sign-in wall it imports now.
    await store.toggleSignin();
    expect(store.signinOn).toBe(true);
    expect(sent('catch_use_browser_signin')).toEqual([]);
    await store.toggleSignin();
    store.apply(withTabs([tab('a'), tab('b')], 'a', { signinHint: 'offer' }));
    await store.toggleSignin();
    expect(sent('catch_use_browser_signin')).toEqual([{ windowId: 'catch-1', tabId: 'a', site: tab('a').host }]);
    store.apply(withTabs([tab('p', { private: true })], 'p'));
    expect([store.signinOn, store.signinLocked]).toEqual([false, true]);
    expect(await store.toggleSignin()).toBe(false);
  });

  it('Mobile version flips at once and sends the tab', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a')], 'a'));
    await store.toggleMobile();
    expect(store.activeTab?.mobile).toBe(true);
    expect(sent('catch_try_mobile')).toEqual([{ windowId: 'catch-1', tabId: 'a', on: true }]);
  });

  it('Block ads flips at once, hides the count while off, and fails back', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a')], 'a', { adblock: true, adsBlocked: 7 }));
    expect([store.adblockOn, store.adsBlocked]).toEqual([true, 7]);
    await store.toggleAdblock();
    expect([store.adblockOn, store.adsBlocked]).toEqual([false, 0]);
    expect(sent('catch_adblock_set')).toEqual([{ windowId: 'catch-1', tabId: 'a', on: false }]);
    store.apply(withTabs([tab('a')], 'a', { adblock: false, adsBlocked: 0 }));
    reply.catch_adblock_set = () => Promise.reject(new Error('catchNavigationFailed'));
    const failing = store.toggleAdblock();
    expect(store.adblockOn).toBe(true);
    await failing;
    expect(store.adblockOn).toBe(false);
  });

  it('Block ads does nothing in a private tab (nothing is kept for it)', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('p', { private: true })], 'p', { adblock: true }));
    await store.toggleAdblock();
    expect(store.adblockOn).toBe(true);
    expect(sent('catch_adblock_set')).toEqual([]);
  });

  it('shell fit: overlay while the picker or a menu is open, sent once per change', async () => {
    const store = useCatchStore();
    store.apply(withTabs([tab('a', { found: true, phase: 'found' })], 'a', { phase: 'found', candidates: [MAIN] }));
    const fit = { top: 88, side: 48, overlay: false, rtl: false };
    store.fitShell(fit);
    store.fitShell(fit);
    store.openPicker();
    store.fitShell(fit);
    store.closePicker();
    store.menuOpen = true;
    store.fitShell(fit);
    store.menuOpen = false;
    store.fitShell({ ...fit, side: 240 });
    await Promise.resolve();
    expect(sent('catch_shell_fit').map(a => (a as { fit: { overlay: boolean; side: number } }).fit)).toEqual([
      { ...fit }, { ...fit, overlay: true }, { ...fit, side: 240 },
    ]);
  });
});

describe('signinOffFor', () => {
  it('a switched-off site covers every host of it, never a look-alike', () => {
    // Rust keeps the registrable domain; an address stays itself.
    for (const [sites, host, off] of [
      [['example.com'], 'www.example.com', true],
      [['example.com'], 'm.example.com', true],
      [['example.com'], 'example.com', true],
      [['example.com'], 'notexample.com', false],
      [['example.com'], 'example.com.evil.net', false],
      [['[::1]'], '[::1]', true],
      [['example.com'], '', false],
    ] as const) expect(signinOffFor(sites, host), `${sites} ${host}`).toBe(off);
  });
});
