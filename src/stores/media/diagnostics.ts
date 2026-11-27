import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { MediaState, useMediaStateStore } from './state.ts';
import { useMediaGroupStore } from './group.ts';
import { notifyGroup } from '../../tauri/notifications';
import { NotificationKind } from '../../tauri/types/app';
import { useMediaStore } from './media.ts';
import { usePlaylistStore } from './playlist';
import { reportFailedDownload } from '../report';
import { downloadReportContext } from '../../helpers/reportContext';

export interface MediaAttempts {
  id: string;
  groupId: string;
  attempts: { stage: string; login: boolean; outcome: string; code: string | null; ms: number }[];
  winner: number | null;
  finalCode: string | null;
}

export const useMediaDiagnosticsStore = defineStore('media-diagnostics', () => {
  const diagnostics = ref<Record<string, MediaDiagnostic[]>>({});
  const attempts = ref<Record<string, MediaAttempts>>({});
  const fatals = ref<Record<string, MediaFatal>>({});

  const diagnosticsByGroupId = computed(() => {
    const byGroup = new Map<string, MediaDiagnostic[]>();
    // Keep each item's history together and preserve the order of the item buckets.
    for (const items of Object.values(diagnostics.value)) {
      for (const groupId of new Set(items.map(item => item.groupId))) {
        const entries = byGroup.get(groupId);
        if (entries) entries.push(...items);
        else byGroup.set(groupId, [...items]);
      }
    }
    return byGroup;
  });

  const fatalsByGroupId = computed(() => {
    const byGroup = new Map<string, MediaFatal[]>();
    for (const fatal of Object.values(fatals.value)) {
      const entries = byGroup.get(fatal.groupId);
      if (entries) entries.push(fatal);
      else byGroup.set(fatal.groupId, [fatal]);
    }
    return byGroup;
  });

  const stateStore = useMediaStateStore();
  const groupStore = useMediaGroupStore();
  const mediaStore = useMediaStore();

  function processMediaAttemptsPayload(payload: MediaAttempts) {
    if (!groupStore.findGroupById(payload.groupId)) return;
    attempts.value[payload.id] = payload;
  }

  function findAttemptsByGroupId(groupId: string): MediaAttempts[] {
    return Object.values(attempts.value).filter(report => report.groupId === groupId);
  }

  function processMediaDiagnosticPayload(payload: MediaDiagnostic) {
    if (!groupStore.findGroupById(payload.groupId)) return;
    diagnostics.value[payload.id] = diagnostics.value[payload.id] ?? [];
    diagnostics.value[payload.id].push(payload);
  }

  function processMediaFatalPayload(payload: MediaFatal, options: { report?: boolean } = {}) {
    const { groupId } = payload;
    const group = groupStore.findGroupById(groupId);
    if (!group) return;
    fatals.value[payload.id] = payload;
    const currentState = stateStore.getGroupState(groupId);
    const leader = groupStore.findGroupLeader(groupId);
    if (currentState === MediaState.fetchingList && leader?.entries && payload.id !== leader.id) {
      if (payload.exitCode === 0 && diagnostics.value[payload.id]?.some(event => event.code === 'input_filter_skipped')) {
        delete fatals.value[payload.id];
      }
      group.processed++;
      group.errored++;
      if (group.processed === group.total) {
        const hasMedia = Object.values(group.items).some(item => !item.isLeader);
        if (hasMedia) mediaStore.finalizePlaylistGroup(group);
        else {
          mediaStore.rejectPendingReadyGroup(groupId, payload.message);
          stateStore.setGroupState(groupId, MediaState.error);
        }
      }
      return;
    }
    if (options.report !== false) {
      // Automatic report (Rust checks the Settings switch and sends the link shape only).
      reportFailedDownload({
        context: downloadReportContext(findLastDiagnosticByGroupId(groupId)?.code, payload.exitCode, findAttemptsByGroupId(groupId)),
        url: group.url ?? null,
        extractor: group.extractor ?? null,
      });
    }
    if (currentState === MediaState.fetching || currentState === MediaState.fetchingList) {
      mediaStore.rejectPendingReadyGroup(groupId, payload.message);
    }
    group.processed++;
    group.errored++;
    if (leader && leader.entries) {
      if (stateStore.getGroupState(groupId) === MediaState.fetchingList) {
        return;
      }
      // We are combined, so we only set one item to error.
      stateStore.setState(payload.id, MediaState.error);
      const itemsWithoutLeader = Object.values(group.items).filter(item => !item.isLeader);
      const allItemsAreTerminal = itemsWithoutLeader.every((item) => {
        const state = stateStore.getState(item.id);
        return state === MediaState.done || state === MediaState.error;
      });
      if (allItemsAreTerminal) {
        const allItemsFailed = itemsWithoutLeader.every(
          item => stateStore.getState(item.id) === MediaState.error,
        );
        stateStore.setGroupState(
          groupId,
          allItemsFailed ? MediaState.error : MediaState.done,
        );
        if (allItemsFailed) {
          void notifyGroup(NotificationKind.DownloadFailed, group, { message: payload.message });
        }
      }
    } else {
      // We are not combined, so we set all items to error.
      if (group.processed === group.total && group.total > 1) {
        mediaStore.finalizePlaylistGroup(group);
      }
      stateStore.setGroupState(groupId, MediaState.error);
      if (group.parentId) usePlaylistStore().settle(group.parentId);
      else void notifyGroup(NotificationKind.DownloadFailed, group, { message: payload.message });
    }
    mediaStore.rememberFailedGroup(groupId);
  }

  function findFatalsByGroupId(groupId: string): MediaFatal[] {
    const group = groupStore.findGroupById(groupId);
    if (!group) return [];
    return fatalsByGroupId.value.get(group.id)?.slice() ?? [];
  }

  function findDiagnosticsByGroupId(groupId: string): MediaDiagnostic[] {
    const group = groupStore.findGroupById(groupId);
    if (!group) return [];
    return diagnosticsByGroupId.value.get(group.id)?.slice() ?? [];
  }

  function findLastFatalByGroupId(groupId: string): MediaFatal | undefined {
    const group = groupStore.findGroupById(groupId);
    return group ? fatalsByGroupId.value.get(group.id)?.at(-1) : undefined;
  }

  function findLastDiagnosticByGroupId(groupId: string): MediaDiagnostic | undefined {
    const group = groupStore.findGroupById(groupId);
    return group ? diagnosticsByGroupId.value.get(group.id)?.at(-1) : undefined;
  }

  function removeDiagnostics(id: string) {
    delete diagnostics.value[id];
    delete fatals.value[id];
    delete attempts.value[id];
  }

  return { diagnostics, fatals, attempts, processMediaAttemptsPayload, findAttemptsByGroupId, findLastFatalByGroupId, findLastDiagnosticByGroupId, findFatalsByGroupId, findDiagnosticsByGroupId, processMediaDiagnosticPayload, processMediaFatalPayload, removeDiagnostics };
});
