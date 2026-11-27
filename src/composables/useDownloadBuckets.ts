import { computed } from 'vue';
import { useMediaGroupStore } from '../stores/media/group';
import { MediaState, useMediaStateStore } from '../stores/media/state';
import { useListVisibilityStore } from '../stores/listVisibility';
import { phaseOf, type Phase } from '../helpers/groupView';
import type { Group } from '../tauri/types/group';
import { useCatchAutoStore } from '../stores/catchAuto';

/** Splits the queue into what still needs the user or the engine, and what is finished. */
export function useDownloadBuckets() {
  const groupStore = useMediaGroupStore();
  const stateStore = useMediaStateStore();
  const hidden = useListVisibilityStore();

  const withState = computed(() => groupStore.orderedGroups.filter((group: Group) => !hidden.isHidden(group.id)).map((group: Group) => ({
    group,
    state: stateStore.getGroupState(group.id) ?? MediaState.fetching,
  })));

  const inPhase = (phase: Phase) => computed(() => withState.value.filter(entry => phaseOf(entry.state) === phase).map(entry => entry.group));

  const active = computed(() => withState.value.filter(entry => phaseOf(entry.state) !== 'done').map(entry => entry.group));
  const finished = inPhase('done');
  /** A failed read the automatic catch has taken over is not failed: looking counts as reading, found or one step
   * left as needs you (not in `ready`, so "Download all" never sends its page link to the engine). */
  const catchAuto = useCatchAutoStore();
  const failed = computed(() => withState.value.filter(entry => phaseOf(entry.state) === 'error' && !catchAuto.takenOver(entry.group.id)).map(entry => entry.group));
  const ready = inPhase('ready');
  /** Reading the link (single or list): the status bar says "Reading…". */
  const reading = computed(() => withState.value.filter(entry => phaseOf(entry.state) === 'reading'
    || (phaseOf(entry.state) === 'error' && catchAuto.takenOver(entry.group.id) === 'looking')).map(entry => entry.group));
  /** Transferring right now: the one number the sidebar badge and the status bar both show. */
  const downloading = inPhase('downloading');

  return { active, finished, failed, ready, reading, downloading, withState };
}
