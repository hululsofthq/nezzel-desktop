import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { flushPromises, mount } from '@vue/test-utils';
import { clearMocks, mockWindows } from '@tauri-apps/api/mocks';
import { installTauriMock } from '../utils/tauriMock';
import { i18n } from '../../src/i18n';
import CatchHome from '../../src/components/catch/CatchHome.vue';
import { useCatchStore } from '../../src/stores/catch';

describe('search engine switcher', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    mockWindows('catch-bar-1');
    installTauriMock({
      catch_search_engine_get: () => ({ engine: 'duckduckgo', hintShown: true }),
      catch_search_engine_set: () => null,
      catch_home: () => ({ recentSites: [], saved: [] }),
    });
  });
  afterEach(() => {
    clearMocks();
    document.body.innerHTML = '';
  });

  // Owner test t1: the focus stayed on the switcher, so the next typed letter ("b") picked another engine.
  it('a pick hands the keyboard to the field with its text selected', async () => {
    const wrapper = mount(CatchHome, { attachTo: document.body, global: { plugins: [i18n] } });
    const store = useCatchStore();
    await flushPromises();
    const field = wrapper.get<HTMLInputElement>('[data-testid=catch-home-field]');
    await field.setValue('salt road');
    const trigger = wrapper.get<HTMLElement>('[data-testid=catch-engine-home]');
    trigger.element.focus();
    await trigger.trigger('keydown', { key: 'ArrowDown' });
    await flushPromises();
    const google = [...document.body.querySelectorAll<HTMLElement>('[role=menuitemcheckbox]')].find(item => item.textContent?.includes('Google'));
    expect(google).toBeTruthy();
    google!.click();
    await expect.poll(() => document.body.querySelector('[role=menu]')).toBeNull();
    await new Promise(resolve => setTimeout(resolve, 0)); // reka hands the focus back on a 0 ms timer
    expect(store.engine).toBe('google');
    expect(document.activeElement).toBe(field.element);
    expect([field.element.selectionStart, field.element.selectionEnd]).toEqual([0, 'salt road'.length]);
    wrapper.unmount();
  });

  it('the empty home field takes the page direction, so an Arabic placeholder reads right to left', async () => {
    const wrapper = mount(CatchHome, { global: { plugins: [i18n] } });
    await flushPromises();
    const field = wrapper.get<HTMLInputElement>('[data-testid=catch-home-field]');
    expect(field.attributes('dir')).toBeUndefined();
    await field.setValue('salt road');
    expect(field.attributes('dir')).toBe('auto');
    wrapper.unmount();
  });
});
