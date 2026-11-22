import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { UpdaterCheckPayload, UpdaterDownloadProgressPayload } from '../tauri/types/updater';
import { useSettingsStore } from './settings';
import { cardAllowed } from '../helpers/updateCheck';
import { persistedRef, textCodec } from '../helpers/persistedRef';

const RECHECK_MS = 12 * 60 * 60 * 1000;
const RETRY_ON_FOCUS_MS = 60 * 60 * 1000;

export const useUpdaterStore = defineStore('updater', () => {
  const checkResult = ref<UpdaterCheckPayload | undefined>();
  const downloadProgress = ref<UpdaterDownloadProgressPayload>({ received: 0, total: 0 });
  const isUpdating = ref(false);
  const isIgnored = ref(false);
  const isNeedingRestart = ref(false);
  const lastError = ref<string | undefined>();
  const lastOperation = ref<'download' | 'install'>('download');
  const settingsStore = useSettingsStore();
  /** "Skip this version": kept on this computer; a newer version shows the card again. */
  const skippedVersion = persistedRef('nezzel.update.skippedVersion', textCodec, '');
  /** The latest check was manual (About): the user asked, so even a skipped version shows the card. Applies to that result only. */
  const manualCheck = ref(false);
  /** The install failed because the system package manager (Linux deb/rpm) could not get the password. */
  const needsPassword = computed(() => lastOperation.value === 'install' && lastError.value === 'needs_password');
  /** The download stopped receiving data (the engine ended it after a minute of silence). */
  const stalled = computed(() => lastOperation.value === 'download' && lastError.value === 'stalled');
  /** Automatic checks: every 12 hours, and once more on focus after a failed one (at most hourly). */
  let lastAutoCheckAt = 0;
  let lastAutoCheckFailed = false;
  const showCard = computed(() => !isIgnored.value && cardAllowed(checkResult.value, skippedVersion.value, manualCheck.value));

  /** Background check obeys the "update app" switch; a manual check (About) always runs and re-shows the card. */
  async function check(manual = false) {
    if (!manual && !settingsStore.settings.update.updateApp) return;
    checkResult.value = await invoke<UpdaterCheckPayload>('updater_check');
    manualCheck.value = manual;
    if (manual && checkResult.value?.available) isIgnored.value = false;
    return checkResult.value;
  }

  /** An automatic check that never throws. Skips while an update is downloading, installing or waiting on a failed install. */
  async function autoCheck() {
    if (isUpdating.value || isNeedingRestart.value || lastError.value) return;
    lastAutoCheckAt = Date.now();
    const before = checkResult.value?.version;
    try {
      await check();
      lastAutoCheckFailed = false;
      // Later hides one version only: a newer one shows the card again.
      const after = checkResult.value?.version;
      if (after && after !== before) isIgnored.value = false;
    } catch (e) {
      lastAutoCheckFailed = true;
      console.warn('Unable to check for updates:', e);
    }
  }

  /** The launch check, then a re-check every 12 hours while the app runs (it lives in the tray for days). Returns the cleanup. */
  function startAutoCheck(): () => void {
    void autoCheck();
    const timer = setInterval(() => void autoCheck(), RECHECK_MS);
    const onShown = () => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return;
      if (lastAutoCheckFailed && Date.now() - lastAutoCheckAt >= RETRY_ON_FOCUS_MS) void autoCheck();
    };
    window.addEventListener('focus', onShown);
    document.addEventListener('visibilitychange', onShown);
    return () => {
      clearInterval(timer);
      window.removeEventListener('focus', onShown);
      document.removeEventListener('visibilitychange', onShown);
    };
  }

  async function download() {
    if (isUpdating.value) return;
    lastError.value = undefined;
    lastOperation.value = 'download';
    isIgnored.value = false;
    downloadProgress.value = { received: 0, total: 0 };
    try {
      isUpdating.value = true;
      await invoke('updater_download');
      isNeedingRestart.value = true;
    } catch (e) {
      if (String(e) === 'cancelled') processUpdaterCancelled();
      else processUpdaterError(String(e));
    } finally {
      isUpdating.value = false;
    }
  }

  /** Stops the running download; the card goes back to the offer. */
  async function cancel() {
    if (!isUpdating.value || lastOperation.value !== 'download') return;
    try {
      await invoke('updater_cancel');
    } catch (e) {
      console.warn('Unable to cancel the update download:', e);
    }
  }

  async function install() {
    if (isUpdating.value) return;
    lastError.value = undefined;
    lastOperation.value = 'install';
    isUpdating.value = true;
    try {
      await invoke('updater_install');
      isNeedingRestart.value = false;
      processUpdaterFinished();
    } catch (e) {
      isNeedingRestart.value = true;
      processUpdaterError(String(e));
    } finally { isUpdating.value = false; }
  }

  async function retry() {
    const installing = lastOperation.value === 'install';
    // A refused password prompt installed nothing and the engine kept the download: just install again.
    if (needsPassword.value) return install();
    // Any other install failure consumed the cached bytes; fetch a fresh copy first.
    await download();
    if (installing && !lastError.value && isNeedingRestart.value) await install();
  }

  function ignore() {
    isIgnored.value = true;
  }

  function skip() {
    if (checkResult.value?.version) skippedVersion.value = checkResult.value.version;
    isIgnored.value = true;
  }

  function processUpdaterDownloadProgress(payload: UpdaterDownloadProgressPayload) {
    Object.assign(downloadProgress.value, payload);
  }

  function processUpdaterFinished() {
    isUpdating.value = false;
    isIgnored.value = true;
    downloadProgress.value = { received: 0, total: 0 };
  }

  function processUpdaterCancelled() {
    isUpdating.value = false;
    lastError.value = undefined;
    downloadProgress.value = { received: 0, total: 0 };
  }

  function processUpdaterError(payload: string) {
    isUpdating.value = false;
    isIgnored.value = false;
    downloadProgress.value = { received: 0, total: 0 };
    lastError.value = payload;
  }

  return { checkResult, showCard, needsPassword, stalled, cancel, startAutoCheck, skippedVersion, downloadProgress, isUpdating, isIgnored, isNeedingRestart, lastError, lastOperation, retry, ignore, skip, check, download, install, processUpdaterDownloadProgress, processUpdaterFinished, processUpdaterError };
});
