import { flushPromises, mount } from '@vue/test-utils';
import { beforeEach, describe, expect, it } from 'vitest';
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

  it('orders versions by number, not text', () => {
    expect(compareVersions('0.1.10', '0.1.9')).toBeGreaterThan(0);
    expect(compareVersions('0.2.0', '0.2.0-beta.1')).toBe(0);
  });
});
