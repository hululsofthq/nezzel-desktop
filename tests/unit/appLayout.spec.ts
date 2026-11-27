import { flushPromises, mount } from '@vue/test-utils';
import { defineComponent } from 'vue';
import { createMemoryHistory, createRouter } from 'vue-router';
import { describe, expect, it, vi } from 'vitest';
import { emit } from '@tauri-apps/api/event';
import { installTauriMock } from '../utils/tauriMock';
import { resetPlatformForTests } from '../../src/composables/usePlatform';
import { Platform } from '../../src/tauri/types/app';
import AppLayout from '../../src/layouts/AppLayout.vue';
import { i18n } from '../../src/i18n';
import { useMediaStore } from '../../src/stores/media/media';
import { useDownloadSheetStore } from '../../src/stores/downloadSheet';

const pasteField = vi.fn();
const Toolbar = defineComponent({
  setup(_, { expose }) {
    expose({ pasteField, focusField: vi.fn() });
    return () => null;
  },
});

describe('app-wide paste shortcut', () => {
  it('returns to the list and closes an inspector when a pending link is copied again', async () => {
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: '/', name: 'home', component: { template: '<div />' } },
      { path: '/settings', name: 'settings', component: { template: '<div />' } },
    ] });
    await router.push('/settings');
    const wrapper = mount(AppLayout, { global: { plugins: [router, i18n], stubs: { AppToolbar: Toolbar, AppSidebar: true, AppStatusBar: true, TheUpdater: true, DownloadInspector: true } } });
    try {
      useDownloadSheetStore().show('existing');
      useMediaStore().duplicateFocus = { id: 'existing' };
      await flushPromises();
      expect(router.currentRoute.value.name).toBe('home');
      expect(useDownloadSheetStore().open).toBe(false);
    } finally { wrapper.unmount(); }
  });

  it('reads the clipboard explicitly outside editors and leaves native paste inside editors', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/', name: 'home', component: { template: '<div />' } }],
    });
    await router.push('/');
    const wrapper = mount(AppLayout, {
      attachTo: document.body,
      global: {
        plugins: [router, i18n],
        stubs: { AppToolbar: Toolbar, AppSidebar: true, AppStatusBar: true, TheUpdater: true, DownloadInspector: true },
      },
    });
    try {
      for (const modifier of ['metaKey', 'ctrlKey']) {
        pasteField.mockClear();
        const event = new KeyboardEvent('keydown', { key: 'v', [modifier]: true, bubbles: true, cancelable: true });
        document.body.dispatchEvent(event);
        expect(event.defaultPrevented).toBe(true);
        expect(pasteField).toHaveBeenCalledOnce();
      }
      const editor = document.createElement('textarea');
      document.body.append(editor);
      pasteField.mockClear();
      const event = new KeyboardEvent('keydown', { key: 'v', ctrlKey: true, bubbles: true, cancelable: true });
      editor.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(false);
      expect(pasteField).not.toHaveBeenCalled();
      editor.remove();
    } finally {
      wrapper.unmount();
    }
  });
});

describe('sidebar toggle', () => {
  it('stays in one spot on the list and in Settings, expanded and collapsed', async () => {
    const router = createRouter({ history: createMemoryHistory(), routes: [
      { path: '/', name: 'home', component: { template: '<div />' } },
      { path: '/settings', name: 'settings.general', component: { template: '<div />' } },
    ] });
    await router.push('/');
    const wrapper = mount(AppLayout, { global: { plugins: [router, i18n], stubs: { AppToolbar: Toolbar, AppSidebar: true, AppStatusBar: true, TheUpdater: true, DownloadInspector: true } } });
    const spots = () => wrapper.findAll('button[aria-keyshortcuts]').map(b => b.element.parentElement?.getAttribute('style'));
    try {
      const seen = new Set<string | null | undefined>();
      for (const path of ['/', '/settings']) {
        await router.push(path);
        for (let i = 0; i < 2; i++) {
          await flushPromises();
          expect(spots()).toHaveLength(1);
          seen.add(spots()[0]);
          await wrapper.get('button[aria-keyshortcuts]').trigger('click');
        }
      }
      expect([...seen]).toEqual(['inset-inline-start: 14px;']);
    } finally { wrapper.unmount(); }
  });
});

describe('Nezzel browser key (⌘/Ctrl+⇧+B)', () => {
  async function layout(platform: Platform) {
    const catchOpen = vi.fn(() => ({ windowId: 'catch-1' }));
    installTauriMock({ catch_open: catchOpen });
    resetPlatformForTests(platform);
    const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/', name: 'home', component: { template: '<div />' } }] });
    await router.push('/');
    const wrapper = mount(AppLayout, { attachTo: document.body, global: { plugins: [router, i18n], stubs: { AppToolbar: Toolbar, AppSidebar: true, AppStatusBar: true, TheUpdater: true, DownloadInspector: true } } });
    await flushPromises();
    const press = async (target: Element, init: KeyboardEventInit = {}) => {
      target.dispatchEvent(new KeyboardEvent('keydown', { key: 'B', code: 'KeyB', shiftKey: true, [platform === Platform.MacOS ? 'metaKey' : 'ctrlKey']: true, bubbles: true, cancelable: true, ...init }));
      await flushPromises();
    };
    const done = () => {
      wrapper.unmount();
      resetPlatformForTests();
    };
    return { catchOpen, press, done };
  }
  /** A modal screen over the app: the welcome sheet and Report a problem both mount outside the layout. */
  function modal() {
    const sheet = document.createElement('div');
    sheet.setAttribute('role', 'dialog');
    sheet.setAttribute('aria-modal', 'true');
    document.body.append(sheet);
    return sheet;
  }

  it('opens once per press, also from the link field and on an Arabic keyboard, never over a modal screen', async () => {
    const { catchOpen, press, done } = await layout(Platform.Windows);
    const field = document.createElement('input');
    document.body.append(field);
    try {
      await press(document.body);
      expect(catchOpen).toHaveBeenCalledTimes(1);
      // Like ⌘B and ⌘, it is an app key, not a text key: the link field usually has the focus.
      await press(field);
      expect(catchOpen).toHaveBeenCalledTimes(2);
      await press(document.body, { key: 'لآ' });
      expect(catchOpen).toHaveBeenCalledTimes(3);
      const sheet = modal();
      await press(document.body);
      await press(sheet);
      expect(catchOpen).toHaveBeenCalledTimes(3);
      sheet.remove();
    } finally {
      field.remove();
      done();
    }
  });

  it('on macOS the File menu owns the key: the press opens nothing by itself, the menu opens once and never over a modal screen', async () => {
    const { catchOpen, press, done } = await layout(Platform.MacOS);
    try {
      await press(document.body);
      expect(catchOpen).not.toHaveBeenCalled();
      await emit('menu-open-browser');
      await flushPromises();
      expect(catchOpen).toHaveBeenCalledTimes(1);
      const sheet = modal();
      await emit('menu-open-browser');
      await flushPromises();
      expect(catchOpen).toHaveBeenCalledTimes(1);
      sheet.remove();
      // Owner build t4: with a shortcut recorder taking keys, the menu's key still opened a browser window.
      const recorder = document.createElement('button');
      recorder.setAttribute('data-shortcut-recording', '');
      document.body.append(recorder);
      await emit('menu-open-browser');
      await flushPromises();
      expect(catchOpen).toHaveBeenCalledTimes(1);
      recorder.remove();
      await emit('menu-open-browser');
      await flushPromises();
      expect(catchOpen).toHaveBeenCalledTimes(2);
    } finally { done(); }
  });
});
