import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { flushPromises, mount } from '@vue/test-utils';
import { emit } from '@tauri-apps/api/event';
import { clearMocks, mockWindows } from '@tauri-apps/api/mocks';
import { installTauriMock } from '../utils/tauriMock';
import { i18n } from '../../src/i18n';
import CatchBar from '../../src/components/catch/CatchBar.vue';
import CatchHome from '../../src/components/catch/CatchHome.vue';
import CatchPicker from '../../src/components/catch/CatchPicker.vue';
import { mountCatchBar } from '../../src/catchBar';
import { applyLanguage } from '../../src/appearance';
import { useCatchStore } from '../../src/stores/catch';
import type { CatchState } from '../../src/tauri/types/catch';

const STATE: CatchState = {
  windowId: 'catch-1', url: 'https://example.com/watch/1', host: 'example.com', canBack: false, canForward: false, loading: false,
  phase: 'looking', popupsBlocked: 0, played: false, playedSec: 0, candidates: [], firstRun: false,
};

describe('shell geometry', () => {
  const fits: unknown[] = [];
  beforeEach(() => {
    fits.length = 0;
    setActivePinia(createPinia());
    mockWindows('catch-bar-1');
    installTauriMock({ catch_shell_fit: (_cmd, args) => fits.push((args as { fit: unknown }).fit) });
    // A window behind another app paints no frames: WebKit runs no animation-frame or resize-observer callbacks there.
    vi.stubGlobal('requestAnimationFrame', () => 0);
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    clearMocks();
    document.body.innerHTML = '';
  });

  // GUI test t3: the first browser window opened behind another app; its page covered the tab list for good.
  it('reaches Rust without an animation frame, and again when the tab list folds', async () => {
    const wrapper = mount(CatchBar, { props: { windowId: 'catch-1' }, attachTo: document.body, global: { plugins: [i18n] } });
    const store = useCatchStore();
    store.apply(STATE);
    await flushPromises();
    const side = store.sideOpen ? 240 : 48;
    expect(fits).toContainEqual(expect.objectContaining({ side, overlay: false, rtl: false }));
    store.toggleSide();
    await flushPromises();
    expect(fits.at(-1)).toEqual(expect.objectContaining({ side: side === 240 ? 48 : 240 }));
    wrapper.unmount();
  });

  it('marks a plain-http page as not secure in words, and only that page', async () => {
    const wrapper = mount(CatchBar, { props: { windowId: 'catch-1' }, attachTo: document.body, global: { plugins: [i18n] } });
    const store = useCatchStore();
    store.apply(STATE);
    await flushPromises();
    expect(wrapper.find('[data-testid="catch-not-secure"]').exists()).toBe(false);
    store.apply({ ...STATE, url: 'http://www.dopebox.to/', host: 'www.dopebox.to' });
    await flushPromises();
    expect(wrapper.get('[data-testid="catch-not-secure"]').text()).toBe(i18n.global.t('catch.notSecure'));
    wrapper.unmount();
  });

  it('explains a hostless address without leaving the current page', async () => {
    const navigate = vi.fn();
    installTauriMock({ catch_navigate: navigate });
    const wrapper = mount(CatchBar, { props: { windowId: 'catch-1' }, global: { plugins: [i18n] } });
    try {
      const store = useCatchStore();
      store.apply(STATE);
      const address = wrapper.get('[data-testid="catch-address"]');
      await address.trigger('focus');
      await address.setValue('https://');
      await wrapper.get('form').trigger('submit');
      expect(navigate).not.toHaveBeenCalled();
      expect(store.shown?.url).toBe(STATE.url);
      expect(address.attributes('aria-invalid')).toBe('true');
      expect(wrapper.get('#catch-address-error').text()).toBe('Enter a valid web address');
      await address.setValue('https://example.org');
      expect(wrapper.find('#catch-address-error').exists()).toBe(false);
    } finally {
      wrapper.unmount();
    }
  });

  it('shows the failed site and retries the current tab', async () => {
    const navigate = vi.fn();
    installTauriMock({ catch_navigate: navigate });
    const wrapper = mount(CatchBar, { props: { windowId: 'catch-1' }, global: { plugins: [i18n] } });
    try {
      const store = useCatchStore();
      const tab = { id: 't1', url: 'https://sub.nonexistent.invalid/path', host: 'nonexistent.invalid',
        loading: false, phase: 'loadFailed', found: false, private: false, asleep: false, mobile: false } as const;
      store.apply({ ...STATE, url: tab.url, host: tab.host, phase: tab.phase, tabs: [tab], activeTabId: tab.id });
      await flushPromises();
      expect(wrapper.get('[data-testid="catch-title"]').text()).toBe('Couldn\'t open nonexistent.invalid');
      expect(wrapper.get('[data-testid="catch-title"] bdi').attributes('dir')).toBe('ltr');
      const retry = wrapper.get('[data-view="loadFailed"] button');
      expect(retry.text()).toBe('Try again');
      await retry.trigger('click');
      expect(navigate).toHaveBeenCalledWith('catch_navigate', { windowId: 'catch-1', tabId: 't1', action: 'reload' });
      await applyLanguage('ar');
      await flushPromises();
      expect(wrapper.get('[data-testid="catch-title"]').text()).toBe('تعذّر فتح nonexistent.invalid');
    } finally {
      wrapper.unmount();
      await applyLanguage('en');
    }
  });

  it('shows the file hand-off outcome briefly in the browser bar', async () => {
    vi.useFakeTimers();
    const wrapper = mount(CatchBar, { props: { windowId: 'catch-1' }, global: { plugins: [i18n] } });
    try {
      useCatchStore().apply(STATE);
      await flushPromises();
      for (const [notice, message] of [
        [{ kind: 'saving', name: 'ملف.zip' }, 'Saving ملف.zip in Nezzel'],
        [{ kind: 'saving', name: null }, 'Saving the file in Nezzel'],
        [{ kind: 'opened', name: 'ملف.zip' }, 'Opened ملف.zip in your browser'],
        [{ kind: 'opened', name: null }, 'Opened the file in your browser'],
        [{ kind: 'failed', name: 'manual.pdf' }, 'Couldn\'t save manual.pdf'],
        [{ kind: 'failed', name: null }, 'Couldn\'t save the file'],
        [{ kind: 'unsupported' }, 'This file can\'t be saved from this page'],
        [{ kind: 'private' }, 'Files can\'t be opened from a private tab'],
        [{ kind: 'proxied' }, 'Files can\'t be opened while the proxy is on'],
      ] as const) {
        await emit('catch://file-download', notice);
        await wrapper.vm.$nextTick();
        expect(wrapper.get('[data-testid="catch-file-download"]').text()).toBe(message);
      }
      await applyLanguage('ar');
      await emit('catch://file-download', { kind: 'saving', name: 'Archive-1.zip' });
      await wrapper.vm.$nextTick();
      const status = wrapper.get('[data-testid="catch-file-download"]');
      expect(status.text()).toBe('جارٍ حفظ Archive-1.zip في Nezzel');
      expect(status.get('bdi').attributes('dir')).toBe('auto');
      await vi.advanceTimersByTimeAsync(5000);
      expect(wrapper.find('[data-testid="catch-file-download"]').exists()).toBe(false);
    } finally {
      wrapper.unmount();
      vi.useRealTimers();
      await applyLanguage('en');
    }
  });

  it.each([
    [false, '[data-testid="catch-tab"] .truncate'],
    [true, '[data-testid="catch-tab-limit"] li .truncate'],
  ])('uses the page title direction in the tab list (limit prompt: %s)', async (limit, selector) => {
    const wrapper = mount(CatchBar, { props: { windowId: 'catch-1' }, attachTo: document.body, global: { plugins: [i18n] } });
    const store = useCatchStore();
    try {
      store.sideOpen = true;
      store.apply({ ...STATE, title: 'من طوكيو إلى إسلام آباد؟' });
      store.limitAsk = limit ? {} : null;
      await flushPromises();
      expect(wrapper.get(selector).attributes('dir')).toBe('auto');
    } finally {
      wrapper.unmount();
    }
  });

  it.each(['sites', 'saved'])('uses each title direction on the home %s list', async (list) => {
    const store = useCatchStore();
    store.home = {
      recentSites: [{ host: 'example.com', title: 'من طوكيو إلى إسلام آباد؟' }],
      saved: [{ downloadId: '1', host: 'example.com', title: 'The report from Tokyo' }],
    };
    const wrapper = mount(CatchHome, { global: { plugins: [i18n] } });
    try {
      expect(wrapper.get(`[data-testid="catch-home-${list}"] .truncate`).attributes('dir')).toBe('auto');
    } finally {
      wrapper.unmount();
    }
  });

  it('uses the page title direction when the quality picker has no media title', () => {
    const title = 'من طوكيو إلى إسلام آباد؟';
    useCatchStore().apply({
      ...STATE, title, phase: 'found', candidates: [{ id: 'video', kind: 'hls', main: true }],
    });
    const wrapper = mount(CatchPicker, { global: { plugins: [i18n] } });
    try {
      const heading = wrapper.get('.line-clamp-2');
      expect(heading.text()).toBe(title);
      expect(heading.attributes('dir')).toBe('auto');
    } finally {
      wrapper.unmount();
    }
  });

  it('applies a saved language change to the open shell and its document', async () => {
    installTauriMock({ config_get: () => ({ appearance: { language: 'en', theme: 'system' } }) });
    document.body.innerHTML = '<div id="app"></div>';
    const app = await mountCatchBar('catch-bar-1');
    try {
      expect(i18n.global.locale.value).toBe('en');
      expect(document.documentElement.dir).toBe('ltr');
      await emit('appearance://language', 'ar');
      await flushPromises();
      expect(i18n.global.locale.value).toBe('ar');
      expect(document.documentElement.dir).toBe('rtl');
      expect(document.querySelector('[data-testid="catch-back"]')?.getAttribute('aria-label')).toBe('رجوع');
      await emit('appearance://language', 'en');
      await flushPromises();
      expect(document.documentElement.dir).toBe('ltr');
    } finally {
      app.unmount();
      await applyLanguage('en');
    }
  });

  it('follows saved theme changes in the open browser shell', async () => {
    installTauriMock({ config_get: () => ({ appearance: { language: 'en', theme: 'light' } }) });
    document.body.innerHTML = '<div id="app"></div>';
    const app = await mountCatchBar('catch-bar-1');
    try {
      expect(document.body.dataset.theme).toBe('light');
      await emit('appearance://theme', 'dark');
      expect(document.body.dataset.theme).toBe('dark');
      await emit('appearance://theme', 'system');
      expect(document.body.dataset.theme).toBeUndefined();
    } finally {
      app.unmount();
      document.body.removeAttribute('data-theme');
    }
  });
});
