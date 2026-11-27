import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest';
import { nextTick } from 'vue';
import { createPinia, disposePinia, getActivePinia, setActivePinia } from 'pinia';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { encodeQueue, decodeQueue, writeQueue, readQueue, type SavedDownload } from '../../src/helpers/queueRecovery';
import { useListVisibilityStore } from '../../src/stores/listVisibility';
import { useCatchAutoStore } from '../../src/stores/catchAuto';
import { useMediaStore } from '../../src/stores/media/media';
import { useMediaStateStore, MediaState } from '../../src/stores/media/state';
import { useMediaGroupStore } from '../../src/stores/media/group';
import { useMediaOptionsStore } from '../../src/stores/media/options';
import { usePlaylistStore } from '../../src/stores/media/playlist';
import { needsAttention } from '../../src/helpers/listFilter';
import { TrackType } from '../../src/tauri/types/media';

function saved(): SavedDownload & { options: NonNullable<SavedDownload['options']> } {
  return {
    group: { id: 'saved', url: 'https://example.com/video', title: 'فيديو', total: 1, processed: 1, errored: 0,
      isCombined: false, audioCodecs: [], formats: [], filesize: 100,
      items: { item: { id: 'item', groupId: 'saved', url: 'https://example.com/video', isLeader: true, playlistChild: true, title: 'فيديو', formats: [], audioCodecs: [], filesize: 100 } } },
    options: { trackType: TrackType.both, height: 720 },
    overrides: { output: { downloadDir: '/downloads' }, auth: { password: 'secret', cookieFile: '/private/cookies' }, network: { proxy: 'https://user:pass@proxy' } },
  };
}

it('disposes queue persistence timers and listeners when the test finishes', async () => {
  vi.useFakeTimers();
  localStorage.clear();
  mockIPC(() => undefined);
  const writes = vi.spyOn(Storage.prototype, 'setItem');
  // onTestFinished runs after the shared afterEach has disposed this test's Pinia.
  onTestFinished(async () => {
    const nextPinia = createPinia();
    setActivePinia(nextPinia);
    try {
      await vi.advanceTimersByTimeAsync(500);
      expect(getActivePinia()).toBe(nextPinia);
      expect(writes).not.toHaveBeenCalled();
      window.dispatchEvent(new Event('beforeunload'));
      expect(writes).not.toHaveBeenCalled();
    } finally {
      disposePinia(nextPinia);
      vi.useRealTimers();
      vi.restoreAllMocks();
      localStorage.clear();
    }
  });
  useMediaStore();
  const row = saved();
  useMediaGroupStore().createGroup(row.group);
  useMediaOptionsStore().setOptions('saved', row.options);
  useMediaStateStore().setState('item', MediaState.downloading);
  await nextTick();
});

describe('saved download queue', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    setActivePinia(createPinia());
  });
  afterEach(() => {
    disposePinia(getActivePinia()!);
    vi.useRealTimers();
    clearMocks();
    vi.restoreAllMocks();
    localStorage.clear();
  });
  it('keeps a playlist child under its playlist across a restart, without a thumbnail link', () => {
    const row = saved();
    row.group.parentId = 'pl';
    row.group.parentIndex = 4;
    row.parent = { id: 'pl', url: 'https://example.com/list', title: 'Series', thumbnail: 'https://cdn/signed.jpg', count: 9, skipped: 0 };
    const [back] = decodeQueue(encodeQueue([row]));
    expect(back.group).toMatchObject({ parentId: 'pl', parentIndex: 4 });
    expect(back.parent).toEqual({ id: 'pl', url: 'https://example.com/list', title: 'Series', count: 9, skipped: 0 });
  });

  it('still decodes an old snapshot with a combined playlist row', () => {
    const row = saved();
    row.group.isCombined = true;
    const [back] = decodeQueue(JSON.stringify({ version: 1, rows: [row] }));
    expect(back.group.isCombined).toBe(true);
    expect(back.parent).toBeUndefined();
  });

  it('round-trips choices, IDs and destination without credentials', () => {
    const text = encodeQueue([saved()]);
    const [row] = decodeQueue(text);
    expect(row.group).toEqual(saved().group);
    expect(row.options).toEqual(saved().options);
    expect(row.overrides).toEqual({ output: { downloadDir: '/downloads' } });
    expect(text).not.toContain('secret');
    expect(text).not.toContain('proxy');
  });
  it('drops nested credentials, signed media and bulky probe metadata', () => {
    const row = saved();
    row.group.items.item.overrides = { auth: { password: 'nested-secret', bearerToken: 'bearer-secret', headers: ['Authorization: header-secret'] } };
    row.group.items.item.resolvedUrl = 'https://cdn.example/video?signature=secret';
    row.group.items.item.attachments = [{ id: 'a', kind: 'image', url: 'https://cdn.example/photo?signature=secret' }];
    row.group.attachments = row.group.items.item.attachments;
    row.group.items.item.description = 'bulky description';
    const text = encodeQueue([row]);
    for (const secret of ['nested-secret', 'bearer-secret', 'header-secret', 'signature=secret', 'bulky description']) expect(text).not.toContain(secret);
    expect(decodeQueue(text)[0].group.items.item.url).toBe(row.group.items.item.url);
  });
  it('coalesces a burst of queue changes into one delayed persistence write', async () => {
    vi.useFakeTimers();
    mockIPC(() => undefined);
    useMediaStore();
    const writes = vi.spyOn(Storage.prototype, 'setItem');
    const row = saved();
    useMediaGroupStore().createGroup(row.group);
    useMediaOptionsStore().setOptions('saved', row.options);
    useMediaStateStore().setState('item', MediaState.downloading);
    for (let n = 0; n < 1000; n++) useMediaGroupStore().groups.saved.title = String(n);
    await nextTick();
    expect(writes).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(500);
    expect(writes).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
  });
  it.each(['bad json', '{"version":2,"rows":[]}', '{"version":1,"rows":[null,{}]}'])('ignores corrupt/unknown data: %s', (raw) => {
    expect(decodeQueue(raw)).toEqual([]);
  });
  it('does not start on launch; click resumes with the saved IDs and selection', async () => {
    writeQueue([saved()]);
    const calls: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => {
      calls.push({ cmd, payload });
      return undefined;
    });
    const media = useMediaStore();
    expect(media.restoreCount).toBe(1);
    expect(useMediaStateStore().getGroupState('saved')).toBe(MediaState.paused);
    expect(calls.find(call => call.cmd === 'group_sweep_partials')?.payload).toEqual({ saved: [['saved', 'item']], roots: ['/downloads'] });
    expect(calls.filter(call => call.cmd === 'media_download')).toHaveLength(0);
    await media.resumeSavedDownloads();
    expect(media.restoreCount).toBe(0);
    expect(calls.find(call => call.cmd === 'media_download')?.payload).toMatchObject({
      groupId: 'saved', origin: 'resume', items: [{ id: 'item', format: { height: 720 }, overrides: { output: { downloadDir: '/downloads' } } }],
    });
    expect(readQueue()).toHaveLength(1);
    useMediaStateStore().setState('item', MediaState.done);
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue()).toEqual([]);
  });
  it('shows interrupted, cancelled and saved children under one playlist before Resume', async () => {
    const interrupted = saved();
    interrupted.group.parentId = 'pl';
    interrupted.group.parentIndex = 1;
    interrupted.parent = { id: 'pl', title: 'Series', count: 3, skipped: 0 };
    const cancelled = saved();
    cancelled.group.id = 'cancelled';
    cancelled.group.items.item.id = 'cancelled-item';
    cancelled.group.parentId = 'pl';
    cancelled.group.parentIndex = 2;
    cancelled.parent = interrupted.parent;
    cancelled.cancelled = true;
    writeQueue([interrupted, cancelled]);
    mockIPC(cmd => cmd === 'library_list'
      ? [{
          id: 'completed', paths: ['/downloads/completed.mp4'], url: 'https://example.com/completed',
          title: 'Saved video', kind: 'video', size: 1, completedAt: 1,
          display: { playlist: { key: 'pl', title: 'Series', count: 3, index: 0 } },
        }]
      : undefined);

    const media = useMediaStore();
    await media.loadLibrary();
    const states = useMediaStateStore();
    const playlist = usePlaylistStore();
    expect(playlist.childrenOf('pl').map(child => [child.parentIndex, states.getGroupState(child.id)])).toEqual([
      [0, MediaState.done], [1, MediaState.paused], [2, MediaState.cancelled],
    ]);
    expect(playlist.findPlaylist('pl')?.count).toBe(3);
    expect(media.restoreCount).toBe(1);
  });
  it.each(['capture-5', 'library-playlist:capture-5'])('joins legacy capture history to recovered %s only with the same URL', async (playlistId) => {
    const key = 'capture-5';
    const url = 'https://example.com/list?list=current';
    const row = saved();
    row.group.parentId = playlistId;
    row.group.parentIndex = 1;
    row.parent = { id: playlistId, url, title: 'Current list', count: 2, skipped: 0 };
    writeQueue([row]);
    const completed = (id: string, parentUrl: string | null) => ({
      id, paths: [`/downloads/${id}.mp4`], url: `https://example.com/${id}`, title: id, kind: 'video', size: 1, completedAt: 1,
      display: { playlist: { key, url: parentUrl, count: 2, index: 0 } },
    });
    const history = [completed('other-list', 'https://example.com/list?list=other'), completed('unknown-list', null), completed('same-list', url)];
    mockIPC(cmd => cmd === 'library_list' ? history : undefined);

    await useMediaStore().loadLibrary();
    const { childrenOf } = usePlaylistStore();
    expect(childrenOf(playlistId).map(child => child.id)).toEqual(['library:same-list', row.group.id]);
    expect(childrenOf(`library-playlist:${playlistId}`).map(child => child.id)).toEqual(['library:other-list', 'library:unknown-list']);
  });
  it('uses completed library history if the queue snapshot still shows the download running', async () => {
    let receipt: object | undefined;
    mockIPC((cmd, payload) => {
      if (cmd === 'media_download') {
        const { metadata } = payload as { metadata: { item: { historyId: string } } };
        receipt = { id: metadata.item.historyId, paths: ['/downloads/clip.mp4'], url: 'https://example.com/video',
          title: 'Saved video', kind: 'video', size: 1, completedAt: 1 };
      }
      if (cmd === 'library_list') return [receipt];
    });
    const media = useMediaStore();
    const row = saved();
    useMediaGroupStore().createGroup(row.group);
    await media.downloadGroup(row.group.id, row.options);
    expect(receipt).toBeDefined();
    expect(readQueue()).toHaveLength(1);
    disposePinia(getActivePinia()!);
    setActivePinia(createPinia());

    const restored = useMediaStore();
    await restored.loadLibrary();
    const groups = useMediaGroupStore().orderedGroups;
    expect(groups).toHaveLength(1);
    expect(useMediaStateStore().getGroupState(groups[0].id)).toBe(MediaState.done);
    expect(restored.restoreCount).toBe(0);
    expect(readQueue()).toEqual([]);
  });
  it('keeps an interrupted Download again attempt and its partials after restart', async () => {
    type Download = { groupId: string; metadata: { item: { historyId: string } }; items: { id: string; overrides: { output: { keepBoth?: boolean } } }[] };
    const starts: Download[] = [];
    const cancels = vi.fn();
    const sweeps = vi.fn();
    const row = saved();
    mockIPC((cmd, payload) => {
      if (cmd === 'media_download') starts.push(payload as Download);
      if (cmd === 'group_cancel') cancels(payload);
      if (cmd === 'group_sweep_partials') sweeps(payload);
      if (cmd === 'library_list') return [{
        id: starts[0].metadata.item.historyId, paths: ['/downloads/clip.mp4'], url: row.group.url,
        title: row.group.title, kind: 'video', size: 1, completedAt: 1,
      }];
    });
    const media = useMediaStore();
    useMediaGroupStore().createGroup(row.group);
    useMediaOptionsStore().setOverrides(row.group.id, { output: { downloadDir: '/downloads' } });
    await media.downloadGroup(row.group.id, row.options);
    useMediaStateStore().setGroupState(row.group.id, MediaState.done);
    const repeat = media.downloadAgain(row.group.id);
    media.processMediaAddPayload({ groupId: row.group.id, total: 1, item: { ...row.group.items.item } });
    await repeat;
    window.dispatchEvent(new Event('beforeunload'));
    disposePinia(getActivePinia()!);
    setActivePinia(createPinia());

    const restored = useMediaStore();
    await restored.loadLibrary();
    expect(restored.restoreCount).toBe(1);
    expect(useMediaStateStore().getGroupState(row.group.id)).toBe(MediaState.paused);
    expect(useMediaStateStore().getGroupState(`library:${starts[0].metadata.item.historyId}`)).toBe(MediaState.done);
    expect(sweeps).toHaveBeenLastCalledWith({ saved: [[row.group.id, 'item']], roots: ['/downloads'] });
    expect(cancels).not.toHaveBeenCalled();
    await restored.resumeSavedDownloads();
    expect(starts[1].metadata.item.historyId).not.toBe(starts[0].metadata.item.historyId);
    expect(starts[2]).toMatchObject({ groupId: row.group.id, metadata: starts[1].metadata, items: [{ id: 'item', overrides: { output: { keepBoth: true } } }] });
  });
  it('restores pending offers in Needs you without probing or downloading until asked', async () => {
    const calls: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => {
      calls.push({ cmd, payload });
    });
    const media = useMediaStore();
    const source = 'https://example.com/pending';
    const ready = saved().group.items.item;
    const clip = await media.dispatchMediaInfoFetch(source, false, false, { ...ready, id: 'clip-item', url: source });
    const listSource = 'https://example.com/list';
    const list = await media.dispatchMediaInfoFetch(listSource, false, false, {
      ...ready, id: 'list-item', url: listSource, entries: [{ index: 0, videoUrl: source }],
    });
    const reading = media.createCaptureGroup('capture-1', 'https://example.com/reading');
    expect(useMediaStateStore().getGroupState(clip)).toBe(MediaState.configure);
    expect(useMediaStateStore().getGroupState(list)).toBe(MediaState.playlistSelection);
    window.dispatchEvent(new Event('beforeunload'));
    disposePinia(getActivePinia()!);
    setActivePinia(createPinia());
    calls.length = 0;

    const restored = useMediaStore();
    const groups = useMediaGroupStore();
    const states = useMediaStateStore();
    expect(groups.orderedGroups.map(group => group.url)).toEqual(['https://example.com/reading', listSource, source]);
    for (const group of groups.orderedGroups) {
      const state = states.getGroupState(group.id);
      expect(state).toBeDefined();
      expect(needsAttention(state!, false)).toBe(true);
      expect(state).not.toBe(MediaState.error);
    }
    expect(groups.findGroupById(reading)?.url).toBe('https://example.com/reading');
    expect(restored.restoreCount).toBe(0);
    await restored.downloadAllGroups();
    expect(calls.filter(({ cmd }) => ['media_info', 'media_download', 'library_record_failure'].includes(cmd))).toEqual([]);
    await restored.retryGroup(list);
    expect(calls.find(({ cmd }) => cmd === 'media_info')?.payload).toMatchObject({ groupId: list, url: listSource });
    restored.processMediaAddPayload({ groupId: list, total: 1, item: {
      ...ready, id: 'list-item', url: listSource, entries: [{ index: 0, videoUrl: source }],
    } });
    expect(states.getGroupState(list)).toBe(MediaState.playlistSelection);
    expect(calls).not.toContainEqual(expect.objectContaining({ cmd: 'media_download' }));
  });
  it('restores an automatic browser offer instead of its underlying failure history', async () => {
    let history: unknown;
    mockIPC((cmd, payload) => {
      if (cmd === 'library_record_failure') history = { paths: [], completedAt: 1, ...((payload as { row: object }).row) };
      if (cmd === 'library_list') return [history];
    });
    const media = useMediaStore();
    const row = saved();
    useMediaGroupStore().createGroup(row.group);
    useMediaStateStore().setGroupState(row.group.id, MediaState.error);
    media.rememberFailedGroup(row.group.id);
    useCatchAutoStore().apply({ groupId: row.group.id, phase: 'found', beside: false });
    await nextTick();
    expect(history).toMatchObject({ errorCode: 'unknown' });
    window.dispatchEvent(new Event('beforeunload'));
    disposePinia(getActivePinia()!);
    setActivePinia(createPinia());

    const restored = useMediaStore();
    await restored.loadLibrary();
    const groups = useMediaGroupStore().orderedGroups;
    expect(groups.map(group => group.id)).toEqual([row.group.id]);
    const state = useMediaStateStore().getGroupState(row.group.id)!;
    expect(needsAttention(state, false)).toBe(true);
    expect(state).not.toBe(MediaState.error);
  });
  it('re-resolves signed attachments while preserving the selected IDs', async () => {
    const row = saved();
    row.group.mediaKind = 'image';
    row.group.items.item.mediaKind = 'image';
    row.group.items.item.fetchEngine = 'gallery';
    row.group.attachments = [{ id: 'chosen', kind: 'image', url: 'https://cdn.example/signed' }];
    row.group.items.item.attachments = row.group.attachments;
    row.attachmentIds = ['chosen'];
    writeQueue([row]);
    const calls: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => {
      calls.push({ cmd, payload });
    });
    await useMediaStore().resumeSavedDownloads();
    expect(calls.find(call => call.cmd === 'media_download')?.payload).toMatchObject({
      items: [{ url: row.group.url, attachmentIds: ['chosen'], fetchEngine: 'gallery' }],
    });
    expect(JSON.stringify(calls)).not.toContain('https://cdn.example/signed');
  });
  it('dismiss clears persisted queue and requests partial cleanup without a download', async () => {
    writeQueue([saved()]);
    const calls: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => {
      calls.push({ cmd, payload });
    });
    const media = useMediaStore();
    await media.dismissSavedDownloads();
    expect(useMediaGroupStore().findGroupById('saved')).toBeUndefined();
    expect(readQueue()).toEqual([]);
    expect(calls.find(({ cmd }) => cmd === 'group_cancel')?.payload).toEqual({ groupId: 'saved', discardFiles: true, partialRoots: ['/downloads'] });
    expect(calls.some(({ cmd }) => cmd === 'media_download')).toBe(false);
  });
  it('uses the saved choice once when a row and the banner both resume it', async () => {
    const row = saved();
    row.options.trackType = TrackType.audio;
    writeQueue([row]);
    const calls: { cmd: string; payload: unknown }[] = [];
    mockIPC((cmd, payload) => {
      calls.push({ cmd, payload });
    });
    const media = useMediaStore();
    await Promise.all([media.resumeSavedDownloads(), media.downloadGroup(row.group.id, { trackType: TrackType.both })]);
    await media.dismissSavedDownloads();
    const starts = calls.filter(({ cmd }) => cmd === 'media_download');
    expect(starts).toHaveLength(1);
    expect(starts[0].payload).toMatchObject({ items: [{ format: { trackType: TrackType.audio } }] });
    expect(calls.some(({ cmd }) => cmd === 'group_cancel')).toBe(false);
    expect(useMediaStateStore().getGroupState(row.group.id)).toBe(MediaState.downloading);
    expect(media.restoreCount).toBe(0);
  });
  it('persists pending rows, retains an unanswered restore offer, and keeps cancelled rows flagged', async () => {
    writeQueue([saved()]);
    useMediaStore();
    const row = saved();
    row.group.id = 'new';
    row.group.items.item.id = 'new-item';
    useMediaGroupStore().createGroup(row.group);
    useMediaOptionsStore().setOptions('new', row.options);
    useMediaStateStore().setState('new-item', MediaState.downloading);
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue().map(row => row.group.id)).toEqual(['new', 'saved']);
    useMediaStateStore().setState('new-item', MediaState.cancelled);
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue().map(row => [row.group.id, !!row.cancelled])).toEqual([['new', true], ['saved', false]]);
  });
  it('a row hidden for undo is not saved for restore, and comes back when undone', async () => {
    useMediaStore();
    const row = saved();
    row.group.id = 'new';
    row.group.items.item.id = 'new-item';
    useMediaGroupStore().createGroup(row.group);
    useMediaOptionsStore().setOptions('new', row.options);
    useMediaStateStore().setState('new-item', MediaState.downloading);
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue().map(row => row.group.id)).toContain('new');

    const visibility = useListVisibilityStore();
    visibility.hide(['new']);
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue().map(row => row.group.id)).not.toContain('new');

    window.dispatchEvent(new Event('beforeunload'));
    expect(readQueue().map(row => row.group.id)).not.toContain('new');

    visibility.show(['new']);
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue().map(row => row.group.id)).toContain('new');
  });
  it('a cancelled row survives a restart as Cancelled, is not offered or resumed, and removing it clears it', async () => {
    const calls: string[] = [];
    mockIPC((cmd) => {
      calls.push(cmd);
      return undefined;
    });
    const row = saved();
    row.cancelled = true;
    writeQueue([row]);
    const media = useMediaStore();
    expect(useMediaStateStore().getState('item')).toBe(MediaState.cancelled);
    expect(useMediaGroupStore().findGroupById('saved')).toBeDefined();
    expect(media.restoreCount).toBe(0);
    await media.resumeSavedDownloads();
    expect(calls).not.toContain('media_download');
    expect(useMediaStateStore().getState('item')).toBe(MediaState.cancelled);
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue().map(row => [row.group.id, !!row.cancelled])).toEqual([['saved', true]]);
    await media.deleteGroup('saved');
    await nextTick();
    await vi.advanceTimersByTimeAsync(500);
    expect(readQueue()).toEqual([]);
  });
  it('a cancelled row keeps its thumbnail across a restart, like a finished or failed row', () => {
    mockIPC(() => undefined);
    const row = saved();
    row.cancelled = true;
    row.group.thumbnail = 'https://i.ytimg.com/vi/x/hq.jpg';
    row.group.items.item.thumbnail = 'https://i.ytimg.com/vi/x/hq.jpg';
    writeQueue([row]);
    useMediaStore();
    expect(useMediaGroupStore().findGroupById('saved')?.thumbnail).toBe('https://i.ytimg.com/vi/x/hq.jpg');
    row.group.thumbnail = 'file:///private/thumb.jpg';
    expect(decodeQueue(encodeQueue([row]))[0].group.thumbnail).toBeUndefined();
  });
});
