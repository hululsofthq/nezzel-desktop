import { computed, effectScope, ref, watchEffect } from 'vue';
import { persistedRef, textCodec } from '../helpers/persistedRef';
import { afterUpdateLine } from '../helpers/releaseNotes';
import { useOnboardingStore } from '../stores/onboarding';

/** Running version (vite define); empty in tests that don't set it. */
export const APP_VERSION = typeof __APP_VERSION__ === 'undefined' ? '' : __APP_VERSION__;

/** The last version whose notes the user saw (or the first version installed), on this computer only. */
const lastSeen = persistedRef('nezzel.whatsNew.lastSeen', textCodec, '');
/** The version whose after-update line was already on screen in some run: the line is shown in one run only. */
const shownFor = persistedRef('nezzel.whatsNew.shownFor', textCodec, '');
const shownBefore = shownFor.value;
/** Settings → About scrolls to What's new and flashes it once, then clears this. */
const revealRequested = ref(false);
let initialised = false;

export function useWhatsNew(current: string = APP_VERSION) {
  const onboarding = useOnboardingStore();
  const state = () => afterUpdateLine(current, lastSeen.value, onboarding.welcomeDoneBefore, shownBefore);
  if (!initialised) {
    initialised = true;
    // App-lifetime watcher (not tied to the first component that asked): records the version on a fresh install.
    effectScope(true).run(() => watchEffect(() => {
      const { show, record } = state();
      if (record) lastSeen.value = record;
      if (show) shownFor.value = current;
    }));
  }
  const showLine = computed(() => state().show);
  function markSeen() {
    if (current) lastSeen.value = current;
  }
  return { version: current, lastSeen, showLine, markSeen, revealRequested };
}
