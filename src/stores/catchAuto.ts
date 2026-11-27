import { defineStore } from 'pinia';
import { computed, ref, watch } from 'vue';
import type { UnlistenFn } from '@tauri-apps/api/event';
import type { CatchAutoState } from '../tauri/types/catch';
import { catchAutoCancel, catchAutoDownload, listenCatchAuto } from '../tauri/listeners/catch';
import { autoCatchView, type AutoCatchKind, type AutoCatchView } from '../helpers/catchOffer';
import { useSettingsStore } from './settings';
import { useMediaGroupStore } from './media/group';
import { useMediaStore } from './media/media';

/**
 * Automatic catch in the main window (contract v2): the latest `catch://auto` state per download group.
 * Rust starts sessions; this store only shows them and acts through `catch_auto_cancel` / `catch_auto_download`.
 */
export const useCatchAutoStore = defineStore('catchAuto', () => {
  const settingsStore = useSettingsStore();
  const groupStore = useMediaGroupStore();
  const states = ref<Record<string, CatchAutoState>>({});
  /** Download clicked, Rust not answered yet: the button shows it at once (optimistic). */
  const starting = ref<Record<string, boolean>>({});
  /** The last Download could not start; cleared by the next state or click. */
  const failed = ref<Record<string, boolean>>({});
  let unlisten: UnlistenFn | undefined;
  let registering: Promise<void> | undefined;
  let listening = 0;

  /** Older configs have no field: missing means on (the default). */
  const enabled = computed(() => settingsStore.settings.input?.autoCatch !== false);

  function forget(groupId: string) {
    delete states.value[groupId];
    delete starting.value[groupId];
    delete failed.value[groupId];
  }

  function apply(next: CatchAutoState) {
    if (next.phase === 'cancelled') {
      forget(next.groupId);
      return;
    }
    // A late state for a row already removed must not bring it back.
    if (!groupStore.findGroupById(next.groupId)) return;
    states.value[next.groupId] = next;
    // `starting` belongs to download(): a re-read state while Rust still answers must not re-enable the button.
    delete failed.value[next.groupId];
  }

  function view(groupId: string): AutoCatchView | null {
    return autoCatchView(states.value[groupId], enabled.value);
  }

  /** A failed row the automatic catch has taken over: looking (in progress), found or one step left (needs you).
   * Null beside an embedded result and for a plain note (live, encrypted): those rows count as they did. */
  function takenOver(groupId: string): Exclude<AutoCatchKind, 'note'> | null {
    const current = view(groupId);
    return current && !current.beside && current.kind !== 'note' ? current.kind : null;
  }

  /** Cancel (or the user chose the embedded result): the row goes back at once; Rust confirms with `cancelled`. */
  async function cancel(groupId: string) {
    if (!states.value[groupId]) return;
    forget(groupId);
    try {
      await catchAutoCancel(groupId);
    } catch (error) {
      console.warn('catch_auto_cancel failed', error);
    }
  }

  /** Downloads the found video from Rust's snapshot. The caught download gets its own row (`catch://queued`). */
  async function download(groupId: string, formatId?: string): Promise<boolean> {
    const state = states.value[groupId];
    if (!state || state.phase !== 'found' || starting.value[groupId]) return false;
    starting.value[groupId] = true;
    delete failed.value[groupId];
    try {
      const { downloadId } = await catchAutoDownload(groupId, formatId);
      forget(groupId);
      // The caught download replaces the failed row; beside an embedded result that row stays a choice.
      if (!state.beside && downloadId !== groupId) void useMediaStore().deleteGroup(groupId);
      return true;
    } catch (error) {
      console.warn('catch_auto_download failed', error);
      if (states.value[groupId]) failed.value[groupId] = true;
      return false;
    } finally {
      delete starting.value[groupId];
    }
  }

  // A removed row drops its state; a session still looking for it is stopped.
  watch(() => Object.keys(states.value).filter(id => !groupStore.findGroupById(id)), (gone) => {
    for (const id of gone) {
      const looking = ['waiting', 'looking'].includes(states.value[id]?.phase ?? '');
      forget(id);
      if (looking) void catchAutoCancel(id).catch(() => {});
    }
  });

  async function start() {
    if (!registering) {
      const generation = ++listening;
      registering = listenCatchAuto(apply).then((off) => {
        // stop() came first: release this listener at once.
        if (generation !== listening) off();
        else unlisten = off;
      });
    }
    await registering;
  }

  function stop() {
    listening++;
    registering = undefined;
    unlisten?.();
    unlisten = undefined;
  }

  return { states, starting, failed, enabled, apply, view, takenOver, cancel, download, forget, start, stop };
});
