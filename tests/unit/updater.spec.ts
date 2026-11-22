import { flushPromises, mount } from '@vue/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import TheUpdater from '../../src/components/TheUpdater.vue';
import { useUpdaterStore } from '../../src/stores/updater';
import { useSettingsStore } from '../../src/stores/settings';
import { i18n } from '../../src/i18n';
import { installTauriMock } from '../utils/tauriMock';
import { cardAllowed, MAX_NOTE_CHARS, summaryLines } from '../../src/helpers/updateCheck';
import { afterUpdateLine, compareVersions } from '../../src/helpers/releaseNotes';
import type { UpdaterCheckPayload } from '../../src/tauri/types/updater';

const card = '[data-testid="updater-alert"]';

async function mountAfterCheck(available: boolean) {
  installTauriMock({ updater_check: () => ({ available, currentVersion: '1.2.2', version: available ? '1.2.3' : undefined, manual: false }) });
  const wrapper = mount(TheUpdater, { global: { plugins: [i18n] } });
  await useUpdaterStore().check();
  await flushPromises();
  return wrapper;
}

describe('update card', () => {
  it('shows an available update until Later is clicked', async () => {
    const wrapper = await mountAfterCheck(true);
    expect(wrapper.find(card).exists()).toBe(true);
    await wrapper.findAll('button').find(b => b.text() === 'Later')!.trigger('click');
    expect(wrapper.find(card).exists()).toBe(false);
    wrapper.unmount();
  });

  it.each([
    ['there is no update', false, true],
    ['the background check is switched off', true, false],
  ])('shows nothing when %s', async (_case, available, updateApp) => {
    useSettingsStore().settings.update.updateApp = updateApp;
    const wrapper = await mountAfterCheck(available);
    expect(wrapper.find(card).exists()).toBe(false);
    wrapper.unmount();
  });
});

describe('builds that cannot update in-app', () => {
  it('offer the download page, never an in-app Download', async () => {
    useSettingsStore().settings.update.updateApp = true;
    const calls: string[] = [];
    installTauriMock({
      updater_check: () => ({ available: true, currentVersion: '0.1.4', version: '0.1.5', manual: true }),
      updater_download: () => { calls.push('download'); },
    });
    const wrapper = mount(TheUpdater, { global: { plugins: [i18n] } });
    await useUpdaterStore().check();
    await flushPromises();
    expect(wrapper.find('[data-testid="updater-open-page"]').exists()).toBe(true);
    expect(wrapper.findAll('button').map(b => b.text())).toEqual(['Later']);
    expect(calls).toEqual([]);
    wrapper.unmount();
  });
});

describe('update card notes', () => {
  const notes = JSON.stringify({ en: ['One', 'Two', 'Three', 'Four'], ar: ['واحد'], fr: [] });
  it.each([
    ['the user language', notes, 'ar', ['واحد']],
    ['English when the language is missing', notes, 'de', ['One', 'Two', 'Three']],
    ['English when the language is empty', notes, 'fr', ['One', 'Two', 'Three']],
    ['nothing for the old plain-sentence notes', 'See the assets to download this version', 'en', []],
    ['nothing for JSON without English', JSON.stringify({ ar: [] }), 'fr', []],
    ['nothing for a JSON array', '["One"]', 'en', []],
    ['nothing without notes', undefined, 'en', []],
    ['only text entries', JSON.stringify({ en: [42, null, { a: 1 }, ' ', 'Real line'] }), 'en', ['Real line']],
    ['a very long line cut with an ellipsis', JSON.stringify({ en: ['A'.repeat(100_000)] }), 'en', [`${'A'.repeat(MAX_NOTE_CHARS - 1)}…`]],
    ['no emoji split in half when cut', JSON.stringify({ en: ['👍🏽'.repeat(200)] }), 'en', [`${'👍🏽'.repeat(MAX_NOTE_CHARS - 1)}…`]],
  ])('shows %s', (_case, raw, locale, lines) => {
    expect(summaryLines(raw, locale)).toEqual(lines);
  });
});

describe('skip this version', () => {
  const found = (version: string, extra: Partial<UpdaterCheckPayload> = {}): UpdaterCheckPayload => ({ available: true, currentVersion: '0.1.4', version, manual: false, ...extra });
  it.each([
    ['shows a version nobody skipped', found('0.1.5'), '', false, true],
    ['hides the skipped version on the background check', found('0.1.5'), '0.1.5', false, false],
    ['shows a newer version than the skipped one', found('0.1.6'), '0.1.5', false, true],
    ['shows the skipped version when checked by hand', found('0.1.5'), '0.1.5', true, true],
    ['never shows for store builds', found('0.1.5', { available: false, channel: 'store' }), '', true, false],
    ['shows nothing without an update', { available: false, currentVersion: '0.1.5', manual: false }, '', true, false],
  ] as const)('%s', (_case, info, skipped, manual, shown) => {
    expect(cardAllowed(info, skipped, manual)).toBe(shown);
  });

  beforeEach(() => localStorage.clear());

  it('keeps the skip on this computer and shows the card again on a manual check', async () => {
    useSettingsStore().settings.update.updateApp = true;
    const wrapper = await mountAfterCheck(true);
    await wrapper.get('[data-testid="updater-skip"]').trigger('click');
    expect(wrapper.find(card).exists()).toBe(false);
    expect(localStorage.getItem('nezzel.update.skippedVersion')).toBe('1.2.3');
    await useUpdaterStore().check(true);
    await flushPromises();
    expect(wrapper.find(card).exists()).toBe(true);
    // The manual override covers that one result: the next automatic check of the same version stays quiet.
    await useUpdaterStore().check();
    await flushPromises();
    expect(wrapper.find(card).exists()).toBe(false);
    wrapper.unmount();
  });
});

describe('after-update line', () => {
  it.each([
    ['fresh install (welcome not completed): remembers the version, shows nothing', '0.1.5', '', false, { show: false, record: '0.1.5' }],
    ['update from a version that recorded nothing (welcome completed before): shows the line', '0.1.5', '', true, { show: true }],
    ['preferences not loaded yet: decides nothing', '0.1.5', '', undefined, { show: false }],
    ['same version as last seen', '0.1.5', '0.1.5', true, { show: false }],
    ['updated since the notes were last seen', '0.1.6', '0.1.5', true, { show: true }],
    ['no version known (dev build)', '', '0.1.5', true, { show: false }],
  ] as const)('%s', (_case, current, lastSeen, welcomeDone, result) => {
    expect(afterUpdateLine(current, lastSeen, welcomeDone)).toEqual(result);
  });

  it.each([
    ['shown in an earlier run and never dismissed: gone, version remembered', '0.1.6', '0.1.5', '0.1.6', { show: false, record: '0.1.6' }],
    ['same, updating from a version that recorded nothing', '0.1.5', '', '0.1.5', { show: false, record: '0.1.5' }],
    ['an older version was the one shown: the new line shows', '0.1.7', '0.1.5', '0.1.6', { show: true }],
  ] as const)('one run only: %s', (_case, current, lastSeen, shownBefore, result) => {
    expect(afterUpdateLine(current, lastSeen, true, shownBefore)).toEqual(result);
  });

  it('orders versions by number, not text', () => {
    expect(compareVersions('0.1.10', '0.1.9')).toBeGreaterThan(0);
    expect(compareVersions('0.2.0', '0.2.0-beta.1')).toBe(0);
  });
});

describe('automatic re-checks', () => {
  const found = { available: true, currentVersion: '1.2.2', version: '1.2.3', manual: false };
  const HOUR = 60 * 60 * 1000;
  let calls = 0;
  let fail = false;
  let stop: () => void;
  function start(result: UpdaterCheckPayload = found) {
    calls = 0;
    installTauriMock({
      updater_check: () => {
        calls++;
        if (fail) throw new Error('offline');
        return result;
      },
    });
    useSettingsStore().settings.update.updateApp = true;
    stop = useUpdaterStore().startAutoCheck();
  }
  beforeEach(() => {
    localStorage.clear();
    fail = false;
    vi.useFakeTimers();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => {
    stop();
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('checks at launch, then every 12 hours, and stops when cleaned up', async () => {
    start();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(1);
    await vi.advanceTimersByTimeAsync(12 * HOUR);
    expect(calls).toBe(2);
    stop();
    await vi.advanceTimersByTimeAsync(24 * HOUR);
    expect(calls).toBe(2);
  });

  it('retries once on focus after a failed launch check, at most hourly', async () => {
    fail = true;
    start();
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(1);
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(1); // under an hour since the failure
    await vi.advanceTimersByTimeAsync(HOUR);
    fail = false;
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(2);
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(HOUR);
    window.dispatchEvent(new Event('focus'));
    await vi.advanceTimersByTimeAsync(0);
    expect(calls).toBe(2); // succeeded: focus does not check again
  });

  it('obeys the update switch and keeps a skipped version quiet', async () => {
    start();
    await vi.advanceTimersByTimeAsync(0);
    const store = useUpdaterStore();
    expect(store.showCard).toBe(true);
    store.skip();
    await vi.advanceTimersByTimeAsync(12 * HOUR);
    expect(calls).toBe(2);
    expect(store.showCard).toBe(false);
    useSettingsStore().settings.update.updateApp = false;
    await vi.advanceTimersByTimeAsync(12 * HOUR);
    expect(calls).toBe(2);
  });
});

describe('installing a Linux deb/rpm update', () => {
  const pkg = (systemPackage: boolean) => ({ available: true, currentVersion: '1.2.2', version: '1.2.3', manual: false, systemPackage });
  async function mountInstall(systemPackage: boolean, install: () => unknown) {
    useSettingsStore().settings.update.updateApp = true;
    const calls: string[] = [];
    installTauriMock({
      updater_check: () => pkg(systemPackage),
      updater_download: () => void calls.push('download'),
      updater_install: () => {
        calls.push('install');
        return install();
      },
    });
    const wrapper = mount(TheUpdater, { global: { plugins: [i18n] } });
    const store = useUpdaterStore();
    await store.check();
    await flushPromises();
    return { wrapper, store, calls };
  }
  beforeEach(() => localStorage.clear());

  it.each([
    ['deb/rpm', true, 'Your system may ask for your password to install the update'],
    ['macOS/Windows/AppImage', false, 'Please wait…'],
  ])('says what to expect while installing (%s)', async (_c, systemPackage, text) => {
    let finish!: () => void;
    const { wrapper, store } = await mountInstall(systemPackage, () => new Promise<void>((resolve) => {
      finish = resolve;
    }));
    const pending = store.install();
    await flushPromises();
    expect(wrapper.get(card).text()).toContain(text);
    finish();
    await pending;
    wrapper.unmount();
  });

  it('a refused password prompt says so with both actions, and Retry installs without downloading again', async () => {
    let fail = true;
    const { wrapper, store, calls } = await mountInstall(true, () => {
      if (fail) throw 'needs_password';
    });
    await store.install();
    await flushPromises();
    expect(wrapper.get(card).text()).toContain('The update needs your system password');
    expect(wrapper.get(card).text()).not.toContain('needs_password');
    expect(wrapper.find('[data-testid="updater-open-page"]').exists()).toBe(true);
    fail = false;
    await wrapper.findAll('button').find(b => b.text() === 'Retry')!.trigger('click');
    await flushPromises();
    expect(calls).toEqual(['install', 'install']);
    wrapper.unmount();
  });

  it('any other install failure keeps the generic message and downloads again on Retry', async () => {
    const { wrapper, store, calls } = await mountInstall(false, () => {
      throw 'boom';
    });
    await store.install();
    await flushPromises();
    expect(wrapper.get(card).text()).toContain('Couldn\'t install the update');
    expect(wrapper.find('[data-testid="updater-open-page"]').exists()).toBe(false);
    await store.retry();
    expect(calls[1]).toBe('download');
    wrapper.unmount();
  });
});

describe('a stalled or cancelled update download', () => {
  const offer = { available: true, currentVersion: '1.2.2', version: '1.2.3', manual: false };
  async function mountDownload(download: () => unknown, extra: Record<string, () => unknown> = {}) {
    useSettingsStore().settings.update.updateApp = true;
    installTauriMock({ updater_check: () => offer, updater_download: download, ...extra });
    const wrapper = mount(TheUpdater, { global: { plugins: [i18n] } });
    const store = useUpdaterStore();
    await store.check();
    await flushPromises();
    return { wrapper, store };
  }
  beforeEach(() => localStorage.clear());

  it('says the download stopped with Retry, never the raw code, and the re-check keeps the card', async () => {
    let stall = true;
    const calls: string[] = [];
    const { wrapper, store } = await mountDownload(() => {
      calls.push('download');
      if (stall) throw 'stalled';
    });
    await store.download();
    await flushPromises();
    const text = wrapper.get(card).text();
    expect(text).toContain('The download stopped. Check your connection and try again');
    expect(text).not.toContain('stalled');
    expect(store.isUpdating).toBe(false);
    await store.check();
    expect(wrapper.get(card).text()).toContain('The download stopped');
    stall = false;
    await wrapper.findAll('button').find(b => b.text() === 'Retry')!.trigger('click');
    await flushPromises();
    expect(calls).toEqual(['download', 'download']);
    expect(store.isNeedingRestart).toBe(true);
    wrapper.unmount();
  });

  it('Cancel stops the download and returns the card to the offer', async () => {
    let stop!: () => void;
    const { wrapper, store } = await mountDownload(
      () => new Promise((_resolve, reject) => {
        stop = () => reject('cancelled');
      }),
      { updater_cancel: () => stop() },
    );
    const pending = store.download();
    await flushPromises();
    expect(wrapper.find('[data-testid="updater-cancel"]').exists()).toBe(true);
    await wrapper.get('[data-testid="updater-cancel"]').trigger('click');
    await pending;
    await flushPromises();
    expect(store.lastError).toBeUndefined();
    expect(wrapper.get(card).text()).toContain('Download');
    expect(wrapper.find('[data-testid="updater-cancel"]').exists()).toBe(false);
    wrapper.unmount();
  });
});
