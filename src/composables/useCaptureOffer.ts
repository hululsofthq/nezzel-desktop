import { computed, nextTick, ref, watch, onBeforeUnmount } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useI18n } from 'vue-i18n';
import { emptyDetails, type CaptureOffer } from '../captureTypes';
import { canUseAutomaticLogin, failReason, type FailAction } from '../helpers/captureFailure';
import { autoCatchView, embedCatchPage, readCanCatch, type AutoCatchAction } from '../helpers/catchOffer';
import { catchAutoCancel, catchAutoDownload, catchOpen } from '../tauri/listeners/catch';
import { isKnownAudioSite } from '../helpers/captureChoices';
import { fileCategory } from '../helpers/fileCategory';
import { siteForHost, siteLogo } from '../helpers/siteLogo';
import { formatDuration } from '../helpers/units';
import { useLocaleNumber } from './useLocaleNumber';
import type { Settings } from '../tauri/types/config';
import { applyAppearance } from '../appearance';
import type { CaptureChoice, CaptureWindowAction } from '@/generated/contract';

const SLOW_MS = 5000;
const SNAPSHOT_MS = 1500;

export interface CaptureOfferHooks {
  /** The offer's id changed (a new link). `wasEditing`: the user had been typing in the bar. */
  onNewOffer?: (value: CaptureOffer, wasEditing: boolean) => void;
  /** After every applied snapshot. */
  onApplied?: () => void;
}

/** The capture windows' shared offer state: Rust's snapshot is the only read; `capture_changed` and the
 * poll below only ask again; one stale-offer guard (`apply`); `act` is the only way out. */
export function useCaptureOffer(hooks: CaptureOfferHooks = {}) {
  const { t } = useI18n();
  const { size: formatSize } = useLocaleNumber();
  const offer = ref<CaptureOffer>({ remoteImages: false, id: 0, url: '', site: '', title: '', read: false, details: emptyDetails(), failed: false, reason: null, resolving: false, feed: false });
  /** 'fetch' = the link could not be read; 'action' = Download/Options could not start. */
  const error = ref<false | 'fetch' | 'action'>(false);
  const acting = ref(false);
  const editing = ref(false);
  const slow = ref(false);
  const thumbFailed = ref(false);
  const faviconFailed = ref(false);
  const auth = ref<Settings['auth']>();
  let unlisten: UnlistenFn | undefined;
  let disposed = false;
  let slowTimer: ReturnType<typeof setTimeout> | undefined;
  let snapshotTimer: ReturnType<typeof setInterval> | undefined;

  const details = computed(() => offer.value.details ?? emptyDetails());
  const failure = computed(() => failReason(offer.value.reason));
  const automaticLogin = computed(() => offer.value.failed && canUseAutomaticLogin(offer.value.reason, auth.value));
  const failed = computed(() => offer.value.failed && !automaticLogin.value);
  /** An unsupported web page: offer the Nezzel browser instead of a dead end (#106). */
  const catchable = computed(() => failed.value && (readCanCatch(offer.value.reason, offer.value.url) || offer.value.auto?.phase === 'needsStep'));
  /** Read fine, but the media is another site's video the page only embeds: a quiet "open the page" link. */
  const embedded = computed(() => offer.value.read && !failed.value && !!details.value.embedded && !!embedCatchPage(offer.value.url));
  /** Automatic catch: Rust only sends it while the setting is on. */
  const auto = computed(() => autoCatchView(offer.value.auto, true));
  /** The hidden session works on a failed read: looking, found or a note replaces the failure. needsStep keeps the
   * existing "needs one step" failure with Open the page. */
  const autoOwns = computed(() => failed.value && !!auto.value && !auto.value.beside && auto.value.kind !== 'needsStep');
  /** Beside a usable embedded result: a quiet line instead of "Not the video you wanted?". */
  const autoBeside = computed(() => offer.value.read && !failed.value && !!auto.value?.beside);
  const fileKind = computed(() => details.value.mediaKind === 'file' ? fileCategory(offer.value.title || offer.value.url) : undefined);
  const isPhotos = computed(() => ['image', 'carousel', 'mixed'].includes(details.value.mediaKind ?? ''));
  const isPlaylist = computed(() => offer.value.feed || details.value.mediaKind === 'playlist');
  const choicesLoading = computed(() => !!offer.value.url && !offer.value.read && !offer.value.failed
    && !isPlaylist.value && !isKnownAudioSite(offer.value.url));
  const logo = computed(() => siteLogo(offer.value.url));
  /** Unknown sites only: the site's own favicon, when remote images are allowed. */
  const favicon = computed(() => {
    if (logo.value || !offer.value.remoteImages) return null;
    try {
      const url = new URL(offer.value.url);
      if (siteForHost(url.hostname)) return null;
      return url.protocol === 'https:' ? `${url.origin}/favicon.ico` : null;
    } catch {
      return null;
    }
  });

  /** "Best: 1080p MP4 · ~120 MB": what one click on Download gets you, as separate parts so each stays in
   * order inside Arabic. Computed once; the bar shows the parts, the card's facts line reuses them. */
  const bestParts = computed<{ text: string; ltr?: boolean; estimate?: boolean }[]>(() => {
    const { bestHeight, bestExt, size, photos } = details.value;
    if (fileKind.value) {
      const category = t(`files.category.${fileKind.value}`);
      return [{ text: category }, ...(size ? [{ text: formatSize(size), ltr: true }] : [])];
    }
    if (isPhotos.value && photos) return [{ text: t('capture.photos', { n: photos }, photos) }];
    const kind = bestHeight ? `${bestHeight}p` : bestExt ? t('capture.audio') : '';
    if (!kind) return [];
    const quality = [kind, bestExt.toUpperCase()].filter(Boolean).join(' ');
    return [{ text: t('capture.best', { quality }) }, ...(size ? [{ text: formatSize(size), ltr: true, estimate: true }] : [])];
  });
  /** The card's one facts line: duration, then what Download best gets ("12:34 · 1080p · MP4 · 120 MB"). */
  const facts = computed<{ text: string; strong?: boolean; ltr?: boolean }[]>(() => {
    const { bestHeight, bestExt, size, photos, duration } = details.value;
    if (fileKind.value || (isPhotos.value && photos)) return bestParts.value;
    return [
      duration ? { text: formatDuration(duration, { pad: true }), ltr: true } : null,
      bestHeight ? { text: t('capture.formats.height', { n: bestHeight }), strong: true } : bestExt ? { text: t('capture.audio'), strong: true } : null,
      bestExt ? { text: bestExt.toUpperCase(), ltr: true } : null,
      size ? { text: formatSize(size), ltr: true } : null,
    ].filter((part): part is NonNullable<typeof part> => !!part);
  });

  async function syncAppearance() {
    const settings = await invoke<Settings>('config_get');
    auth.value = settings?.auth;
    await applyAppearance(settings.appearance);
  }

  let asked = 0;
  async function refresh() {
    const ask = ++asked;
    const value = await invoke<CaptureOffer>('capture_snapshot');
    if (value.id !== offer.value.id) await syncAppearance().catch(() => {
      auth.value = undefined;
    });
    if (ask === asked) apply(value);
  }
  function apply(value: CaptureOffer) {
    // Typing in the bar replaces this offer: its own late answers must not bring the old link back.
    if (editing.value && value.id === offer.value.id) return;
    if (value.id !== offer.value.id) {
      hooks.onNewOffer?.(value, editing.value);
      editing.value = false;
    }
    if (value.url !== offer.value.url) {
      faviconFailed.value = false;
      thumbFailed.value = false;
    }
    offer.value = value;
    error.value = false;
    void nextTick(() => hooks.onApplied?.());
  }

  /** One action at a time: a double Enter must not start the download twice. */
  async function act(action: CaptureWindowAction, choice?: CaptureChoice) {
    // An anonymous login refusal can continue through the authenticated main-window fetch.
    if (action === 'download' && failed.value) return;
    if (action === 'download' && isPlaylist.value) {
      action = 'options';
    }
    if (action !== 'dismiss' && action !== 'report' && (!offer.value.url || offer.value.resolving || editing.value || acting.value)) return;
    if (action !== 'dismiss') acting.value = true;
    const id = offer.value.id;
    try {
      if (action === 'download' && automaticLogin.value) {
        await syncAppearance();
        if (failed.value || offer.value.id !== id || editing.value) return;
      }
      await invoke('capture_action', { id, action, choice: choice ?? null });
    } catch {
      if (action !== 'dismiss') error.value = 'action';
    } finally {
      acting.value = false;
    }
  }
  /** Failed read: Retry reads the same link again in place; Report opens Report a problem in the main window. */
  async function onFail(action: FailAction) {
    if (action === 'options' || action === 'report') return act(action);
    if (action === 'catch') {
      acting.value = true;
      try {
        await catchOpen(offer.value.url);
        acting.value = false;
        return act('dismiss');
      } catch (failure) {
        acting.value = false;
        // Six tabs: the Nezzel browser is in front, asking which tab to close for this link.
        if (String(failure).includes('catchTabLimit')) return act('dismiss');
        error.value = 'action';
        return;
      }
    }
    acting.value = true;
    try {
      await invoke('capture_retry');
    } catch {
      error.value = 'action';
    } finally {
      acting.value = false;
    }
  }

  /** Automatic catch actions. Cancel shows the pre-catch state at once; Download hands the found video to Rust's snapshot. */
  async function onAuto(action: AutoCatchAction) {
    const groupId = offer.value.auto?.groupId;
    if (action === 'openPage') return onFail('catch');
    if (!groupId || acting.value) return;
    if (action === 'cancel') {
      offer.value = { ...offer.value, auto: null };
      await catchAutoCancel(groupId).catch(() => {});
      return;
    }
    acting.value = true;
    try {
      await catchAutoDownload(groupId);
      acting.value = false;
      return act('dismiss');
    } catch (reason) {
      // Rust already sent the needsStep state: that view stays instead of a generic action error.
      if (String((reason as { message?: string } | null)?.message ?? reason) !== 'catchAutoNeedsStep') error.value = 'action';
      acting.value = false;
    }
  }

  /** Slow sites: after a few seconds say it's still working, and that Download already works (the list finishes reading). */
  watch(() => [offer.value.id, offer.value.url, offer.value.read, offer.value.failed] as const, ([, url, read, failed]) => {
    clearTimeout(slowTimer);
    slow.value = false;
    if (url && !read && !failed && !offer.value.feed) slowTimer = setTimeout(() => {
      slow.value = true;
    }, SLOW_MS);
  });
  /** Linux can hold capture_changed for this hidden, always-on-top window until the next input
   * (tauri#3654): while the card still reads, ask again without waiting on the event. Dismissed, reported
   * or expired, Rust has no offer (id 0): the panel takes the empty one and stops asking. */
  const waitingForRead = computed(() => !!offer.value.url && !offer.value.failed && !offer.value.feed && !offer.value.resolving
    && (!offer.value.read || auto.value?.kind === 'looking'));
  watch(waitingForRead, (waiting) => {
    clearInterval(snapshotTimer);
    snapshotTimer = waiting ? setInterval(() => void refresh().catch(() => {}), SNAPSHOT_MS) : undefined;
  });

  /** Listen for `capture_changed`, then take the first snapshot. Call from the window's onMounted. */
  async function start() {
    unlisten = await listen('capture_changed', () => void refresh().catch(() => {}));
    if (disposed) {
      unlisten();
      return;
    }
    await refresh();
  }
  onBeforeUnmount(() => {
    disposed = true;
    unlisten?.();
    clearTimeout(slowTimer);
    clearInterval(snapshotTimer);
  });

  return { offer, error, acting, editing, slow, thumbFailed, faviconFailed, details, failure, failed, catchable, embedded, auto, autoOwns, autoBeside, onAuto, automaticLogin, fileKind, isPhotos, isPlaylist, choicesLoading, logo, favicon, bestParts, facts, act, onFail, start };
}
