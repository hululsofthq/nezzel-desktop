import { describe, expect, it, vi } from 'vitest';
import { flushPromises } from '@vue/test-utils';
import { emit } from '@tauri-apps/api/event';
import * as events from '@tauri-apps/api/event';
import { installTauriMock } from '../utils/tauriMock';
import { useCatchAutoStore } from '../../src/stores/catchAuto';
import { useMediaGroupStore } from '../../src/stores/media/group';
import { useSettingsStore } from '../../src/stores/settings';
import { MediaState, useMediaStateStore } from '../../src/stores/media/state';
import { useDownloadList } from '../../src/composables/useDownloadList';
import { useDownloadBuckets } from '../../src/composables/useDownloadBuckets';
import { CATCH_AUTO_EVENT, type CatchAutoState } from '../../src/tauri/types/catch';

function addGroup(id: string) {
  const url = `https://example.com/${id}`;
  useMediaGroupStore().createGroup({ id, url, title: 'Page', total: 1, processed: 1, errored: 1, isCombined: false, audioCodecs: [], formats: [], filesize: 0,
    items: { [`${id}-item`]: { id: `${id}-item`, isLeader: true, url, audioCodecs: [], formats: [], filesize: 0 } } });
}
const found = (over: Partial<CatchAutoState> = {}): CatchAutoState => ({ groupId: 'g1', phase: 'found', beside: false, title: 'Full film', durationSec: 6738, ...over });

vi.mock('@tauri-apps/api/event', { spy: true });

describe('automatic catch store', () => {
  it('keeps the latest state per group from catch://auto and drops it on cancelled', async () => {
    addGroup('g1');
    const store = useCatchAutoStore();
    await store.start();
    await emit(CATCH_AUTO_EVENT, { groupId: 'g1', phase: 'looking', beside: false });
    expect(store.view('g1')?.kind).toBe('looking');
    await emit(CATCH_AUTO_EVENT, found());
    expect(store.view('g1')?.kind).toBe('found');
    await emit(CATCH_AUTO_EVENT, { groupId: 'g1', phase: 'cancelled', beside: false });
    expect(store.states.g1).toBeUndefined();
    // A late state for a row that is gone does not bring it back.
    await emit(CATCH_AUTO_EVENT, found({ groupId: 'gone' }));
    expect(store.states.gone).toBeUndefined();
    store.stop();
  });

  it('Cancel clears the row at once and calls catch_auto_cancel', async () => {
    const cancel = vi.fn(() => new Promise<void>(() => {}));
    installTauriMock({ catch_auto_cancel: cancel });
    addGroup('g1');
    const store = useCatchAutoStore();
    store.apply({ groupId: 'g1', phase: 'looking', beside: false });
    void store.cancel('g1');
    expect(store.view('g1')).toBeNull();
    expect(cancel).toHaveBeenCalledWith('catch_auto_cancel', { groupId: 'g1' });
  });

  it('Download goes through catch_auto_download (never media_download), shows starting at once, and the caught row replaces the failed one', async () => {
    let answer!: (value: { downloadId: string }) => void;
    const download = vi.fn(() => new Promise<{ downloadId: string }>((resolve) => {
      answer = resolve;
    }));
    const mediaDownload = vi.fn();
    installTauriMock({ catch_auto_download: download, media_download: mediaDownload });
    addGroup('g1');
    const store = useCatchAutoStore();
    store.apply(found());
    const done = store.download('g1', 'hd');
    expect(store.starting.g1).toBe(true);
    expect(await store.download('g1')).toBe(false); // a double click starts nothing twice
    answer({ downloadId: 'd9' });
    expect(await done).toBe(true);
    expect(download).toHaveBeenCalledTimes(1);
    expect(download).toHaveBeenCalledWith('catch_auto_download', { groupId: 'g1', formatId: 'hd' });
    expect(mediaDownload).not.toHaveBeenCalled();
    expect(store.states.g1).toBeUndefined();
    expect(useMediaGroupStore().findGroupById('g1')).toBeUndefined();
  });

  it('a found full video beside an embedded result keeps the embedded row; a failed start says so', async () => {
    installTauriMock({ catch_auto_download: () => ({ downloadId: 'd2' }) });
    addGroup('g1');
    const store = useCatchAutoStore();
    store.apply(found({ beside: true }));
    expect(await store.download('g1')).toBe(true);
    expect(useMediaGroupStore().findGroupById('g1')).toBeDefined();

    installTauriMock({ catch_auto_download: () => Promise.reject(new Error('expired')) });
    store.apply(found());
    expect(await store.download('g1')).toBe(false);
    expect(store.failed.g1).toBe(true);
  });

  it('shows nothing when the setting is off', () => {
    addGroup('g1');
    const store = useCatchAutoStore();
    store.apply({ groupId: 'g1', phase: 'looking', beside: false });
    const input = useSettingsStore().settings.input;
    input.autoCatch = false;
    try {
      expect(store.view('g1')).toBeNull();
    } finally {
      input.autoCatch = true;
    }
  });

  it('a removed row drops its state and stops a session still looking', async () => {
    const cancel = vi.fn();
    installTauriMock({ catch_auto_cancel: cancel });
    addGroup('g1');
    const store = useCatchAutoStore();
    store.apply({ groupId: 'g1', phase: 'looking', beside: false });
    useMediaGroupStore().deleteGroup('g1');
    await flushPromises();
    expect(store.states.g1).toBeUndefined();
    expect(cancel).toHaveBeenCalledWith('catch_auto_cancel', { groupId: 'g1' });
  });

  it('the list and status bar file a taken-over row as in progress or needs you, never failed', () => {
    const store = useCatchAutoStore();
    const { rows } = useDownloadList();
    const { failed, reading } = useDownloadBuckets();
    const row = () => rows.value.find(r => r.id === 'g1')!;
    // [list status, needs you, status-bar failed count, status-bar reading count]
    const counts = () => [row().status, row().needsAttention, failed.value.length, reading.value.length];
    addGroup('g1');
    useMediaStateStore().setGroupState('g1', MediaState.error);
    expect(counts()).toEqual(['failed', false, 1, 0]);
    for (const phase of ['waiting', 'looking'] as const) {
      store.apply({ groupId: 'g1', phase, beside: false });
      expect(counts()).toEqual(['active', false, 0, 1]);
    }
    store.apply(found());
    expect(counts()).toEqual(['active', true, 0, 0]);
    // The found video's title is searchable too.
    expect(row().title).toContain('Full film');
    store.apply({ groupId: 'g1', phase: 'needsStep', beside: false });
    expect(counts()).toEqual(['active', true, 0, 0]);
    // A plain note (live, encrypted) leaves the failed row as it was.
    store.apply({ groupId: 'g1', phase: 'encrypted', beside: false });
    expect(counts()).toEqual(['failed', false, 1, 0]);
  });

  it('start() registers one listener for concurrent calls and stop() mid-registration releases it', async () => {
    let finish!: (unlisten: events.UnlistenFn) => void;
    const unlisten = vi.fn();
    const listener = vi.spyOn(events, 'listen').mockImplementationOnce(() => new Promise((resolve) => {
      finish = resolve;
    }));
    try {
      const store = useCatchAutoStore();
      const first = store.start();
      const second = store.start();
      store.stop();
      finish(unlisten);
      await Promise.all([first, second]);
      expect(listener).toHaveBeenCalledTimes(1);
      expect(unlisten).toHaveBeenCalledTimes(1);
    } finally { listener.mockRestore(); }
  });

  it('a state arriving while Download is in flight does not re-enable the button', async () => {
    let settle!: (value: { downloadId: string }) => void;
    installTauriMock({ catch_auto_download: () => new Promise((resolve) => {
      settle = resolve;
    }) });
    addGroup('g1');
    const store = useCatchAutoStore();
    store.apply(found());
    const pending = store.download('g1');
    expect(store.starting.g1).toBe(true);
    store.apply({ groupId: 'g1', phase: 'looking', beside: false });
    expect(store.starting.g1).toBe(true);
    expect(await store.download('g1')).toBe(false);
    settle({ downloadId: 'g1' });
    await pending;
    expect(store.starting.g1).toBeUndefined();
  });
});
