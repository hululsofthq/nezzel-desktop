import { createApp } from 'vue';
import { createPinia } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import CatchBar from './components/catch/CatchBar.vue';
import { i18n } from './i18n';
import { applyAppearance, applyLanguage, applyTheme, removeTheme } from './appearance';
import type { Settings } from './tauri/types/config';

/** Native label prefix of the bar webview above a Nezzel browser page (`catch-bar-<n>`, route `/catch-bar`). */
export const CATCH_BAR_PREFIX = 'catch-bar-';

/** The "Catch from browser" bar (#106): its own small app, IPC only to the catch commands and the config read. */
export async function mountCatchBar(label: string) {
  if (typeof __E2E__ !== 'undefined' && __E2E__) {
    const { installCatchBarMock } = await import('../tests/utils/mocks/catchHandlers');
    installCatchBarMock(label);
  }
  document.documentElement.classList.add('nz-catch');
  let latestLanguage: string | undefined;
  let latestTheme: string | undefined;
  const unlisten = await listen<string>('appearance://language', ({ payload }) => {
    latestLanguage = payload;
    void applyLanguage(payload);
  }).catch((error) => {
    console.warn('Could not subscribe to language changes', error);
  });
  const unlistenTheme = await listen<string>('appearance://theme', ({ payload }) => {
    latestTheme = payload;
    if (payload === 'light' || payload === 'dark') applyTheme(payload);
    else removeTheme();
  }).catch((error) => {
    console.warn('Could not subscribe to theme changes', error);
  });
  try {
    const settings = await invoke<Settings>('config_get');
    await applyAppearance({
      language: latestLanguage ?? settings.appearance.language,
      theme: latestTheme ?? settings.appearance.theme,
    });
  } catch {
    await applyLanguage(latestLanguage ?? 'system');
  }
  const app = createApp(CatchBar, { windowId: `catch-${label.slice(CATCH_BAR_PREFIX.length)}` }).use(createPinia()).use(i18n);
  if (unlisten) app.onUnmount(unlisten);
  if (unlistenTheme) app.onUnmount(unlistenTheme);
  app.mount('#app');
  return app;
}
