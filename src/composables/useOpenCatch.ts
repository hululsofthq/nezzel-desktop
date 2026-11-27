import { useI18n } from 'vue-i18n';
import { catchOpen } from '../tauri/listeners/catch';
import { useToastStore } from '../stores/toast';

/** Main window: open the Nezzel browser on a page (or blank), with a toast when it can't. */
export function useOpenCatch() {
  const { t } = useI18n();
  return async function openCatch(url?: string | null) {
    try {
      await catchOpen(url ?? undefined);
      return true;
    } catch (error) {
      // Six tabs: the Nezzel browser is in front, asking which tab to close.
      if (String(error).includes('catchTabLimit')) {
        useToastStore().showToast(t('catch.tabsFull'), { style: 'info' });
        return false;
      }
      // The proxy is on in Settings with an address the browser cannot take: no page loads around it.
      if (String(error).includes('catchProxyUnusable')) {
        useToastStore().showToast(t('catch.proxyUnusable'), { style: 'error', duration: 8000 });
        return false;
      }
      console.warn('catch_open failed', error);
      useToastStore().showToast(t('catch.openFailed'), { style: 'error' });
      return false;
    }
  };
}
