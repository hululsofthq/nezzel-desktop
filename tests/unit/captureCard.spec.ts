import { mount } from '@vue/test-utils';
import { computed, nextTick, ref } from 'vue';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import CaptureCard from '../../src/components/downloads/CaptureCard.vue';
import { emptyDetails, type CaptureOffer } from '../../src/captureTypes';
import { i18n } from '../../src/i18n';
import { CAPTURE_CARD_SIZE } from '../../src/generated/contract';
import { autoCatchView } from '../../src/helpers/catchOffer';

const invoke = vi.hoisted(() => vi.fn((_cmd: string, _args?: unknown): Promise<boolean> => Promise.resolve(true)));
vi.mock('@tauri-apps/api/core', () => ({ invoke }));
const onFail = vi.hoisted(() => vi.fn());
const onAuto = vi.hoisted(() => vi.fn());
const offer = ref<CaptureOffer>({ id: 1, remoteImages: false, url: 'https://example.com/video', site: 'example.com', title: '', read: false, details: emptyDetails(), failed: false, reason: null, resolving: false, feed: false });
vi.mock('../../src/composables/useCaptureOffer', () => ({
  useCaptureOffer: () => ({
    offer, details: computed(() => offer.value.details), error: ref(false), acting: ref(false), slow: ref(false),
    isPlaylist: computed(() => offer.value.feed || offer.value.details.mediaKind === 'playlist'), choicesLoading: computed(() => !offer.value.read),
    thumbFailed: ref(false), faviconFailed: ref(false), failure: ref(null), favicon: ref(null), facts: ref([]),
    embedded: computed(() => offer.value.read && !!offer.value.details.embedded),
    failed: computed(() => offer.value.failed),
    auto: computed(() => autoCatchView(offer.value.auto, true)),
    autoOwns: computed(() => offer.value.failed && !!offer.value.auto && !offer.value.auto.beside && offer.value.auto.phase !== 'needsStep'),
    autoBeside: computed(() => offer.value.read && !offer.value.failed && !!offer.value.auto?.beside),
    act: vi.fn(), onFail, onAuto, start: vi.fn(),
  }),
}));

describe('copied-link card engagement', () => {
  let card: ReturnType<typeof mount>;
  const reports = () => invoke.mock.calls.filter(([cmd, args]) => cmd === 'capture_fit' && args && typeof args === 'object' && 'engaged' in args).map(([, args]) => args);
  beforeEach(() => {
    vi.useFakeTimers();
    invoke.mockClear();
    invoke.mockResolvedValue(true);
    offer.value = { ...offer.value, id: 1, title: 'Read video', read: true, details: { ...emptyDetails(), mediaKind: 'video', bestHeight: 1080, choices: [{ choice: { kind: 'video', height: 1080 }, size: null }, { choice: { kind: 'audio' }, size: null }] } };
    card = mount(CaptureCard, { attachTo: document.body, global: { plugins: [i18n], stubs: { Tooltip: { props: ['content'], template: '<slot />' } } } });
  });
  afterEach(() => {
    card.unmount();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it('checks native hover even without an enter event, including when first shown under the pointer', async () => {
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
    const before = reports().length;
    await vi.advanceTimersByTimeAsync(12_000);
    expect(reports()).toHaveLength(before + 4);
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
    card.unmount();
    const stopped = reports().length;
    await vi.advanceTimersByTimeAsync(6000);
    expect(reports()).toHaveLength(stopped);
  });

  it.each(['pointerenter', 'pointerdown', 'click'])('holds on %s, including without a hover event, until leave', async (event) => {
    await card.get('section').trigger(event);
    expect(reports().at(-1)).toEqual({ id: 1, engaged: true });
    await vi.advanceTimersByTimeAsync(20_000);
    expect(reports().at(-1)).toEqual({ id: 1, engaged: true });
    await card.get('section').trigger('pointerleave');
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
  });

  it('keeps focus within held across child focus changes and pointer leave', async () => {
    const buttons = card.findAll('button');
    await buttons[0]!.trigger('focusin');
    await card.get('section').trigger('pointerleave');
    expect(reports().at(-1)).toEqual({ id: 1, engaged: true });
    await buttons[0]!.trigger('focusout', { relatedTarget: buttons[1]!.element });
    expect(reports().at(-1)).toEqual({ id: 1, engaged: true });
    await buttons[1]!.trigger('focusout', { relatedTarget: null });
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
  });

  it('holds the open formats list after pointer leave, then releases when closed', async () => {
    await card.get('[data-testid=capture-formats-toggle]').trigger('click');
    await nextTick();
    (document.activeElement as HTMLElement | null)?.blur();
    await nextTick();
    expect(card.get('[data-testid=capture-formats-toggle]').attributes('aria-expanded')).toBe('true');
    await card.get('section').trigger('pointerleave');
    expect(reports().at(-1)).toEqual({ id: 1, engaged: true });
    await card.get('[data-testid=capture-formats-toggle]').trigger('click');
    await nextTick();
    (document.activeElement as HTMLElement | null)?.blur();
    await nextTick();
    await card.get('section').trigger('pointerleave');
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
  });

  it('lets go after 30 s without pointer or key activity when the leave event never arrives', async () => {
    await card.get('section').trigger('pointerdown');
    await vi.advanceTimersByTimeAsync(20_000);
    await card.get('section').trigger('pointermove');
    await vi.advanceTimersByTimeAsync(27_000);
    expect(reports().at(-1)).toEqual({ id: 1, engaged: true });
    await vi.advanceTimersByTimeAsync(6000);
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
  });

  it('shrinks the window when the idle reset closes an open formats list', async () => {
    const fits = () => invoke.mock.calls.filter(([cmd, args]) => cmd === 'capture_fit' && args && typeof args === 'object' && 'height' in args);
    await card.get('[data-testid=capture-formats-toggle]').trigger('click');
    await nextTick();
    const opened = fits().length;
    await vi.advanceTimersByTimeAsync(36_000);
    expect(card.get('[data-testid=capture-formats-toggle]').attributes('aria-expanded')).toBe('false');
    expect(fits().length).toBe(opened + 1);
    expect(fits().at(-1)?.[1]).toEqual({ height: CAPTURE_CARD_SIZE.height });
  });

  it('renews engagement every three seconds and checks native hover after leave', async () => {
    await card.get('section').trigger('pointerdown');
    const before = reports().length;
    await vi.advanceTimersByTimeAsync(6000);
    expect(reports().length).toBe(before + 2);
    await card.get('section').trigger('pointerleave');
    const released = reports().length;
    await vi.advanceTimersByTimeAsync(12_000);
    expect(reports().length).toBe(released + 4);
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
  });

  it('clears engagement and the open menu when the reused native card is hidden', async () => {
    await card.get('[data-testid=capture-formats-toggle]').trigger('click');
    await nextTick();
    invoke.mockResolvedValue(false);
    await vi.advanceTimersByTimeAsync(3000);
    expect(reports().at(-1)).toEqual({ id: 1, engaged: false });
    expect(card.get('[data-testid=capture-formats-toggle]').attributes('aria-expanded')).toBe('false');
    const hidden = reports().length;
    await vi.advanceTimersByTimeAsync(12_000);
    expect(reports().length).toBe(hidden);
  });

  it('does not retain focus from a format row removed by a replacement offer', async () => {
    await card.get('[data-testid=capture-formats-toggle]').trigger('click');
    await nextTick();
    expect(card.get('[data-testid=capture-formats]').element.contains(document.activeElement)).toBe(true);
    const panel = card.get('section').element;
    const matches = panel.matches.bind(panel);
    vi.spyOn(panel, 'matches').mockImplementation(selector => selector === ':hover' ? false : matches(selector));
    offer.value = { ...offer.value, id: 2 };
    await nextTick();
    await nextTick();
    expect(card.get('section').element.contains(document.activeElement)).toBe(false);
    expect(reports().at(-1)).toEqual({ id: 2, engaged: false });
  });

  it('re-derives real focus for a replacement instead of retaining the previous flags', async () => {
    const button = card.findAll('button')[0]!;
    (button.element as HTMLElement).focus();
    offer.value = { ...offer.value, id: 2 };
    await nextTick();
    expect(reports().at(-1)).toEqual({ id: 2, engaged: true });
    (button.element as HTMLElement).blur();
    await nextTick();
    expect(reports().at(-1)).toEqual({ id: 2, engaged: false });
  });

  it('clears the previous interaction for a replacement offer and releases on unmount', async () => {
    await card.get('section').trigger('pointerdown');
    offer.value = { ...offer.value, id: 2 };
    await nextTick();
    expect(reports().at(-1)).toEqual({ id: 2, engaged: false });
    card.unmount();
    expect(reports().at(-1)).toEqual({ id: 2, engaged: false });
  });
});

describe('copied-link card dismiss tooltip (#83)', () => {
  it('stays closed on the focus the webview gives at open, opens on keyboard focus', async () => {
    vi.useFakeTimers();
    vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
    offer.value = { ...offer.value, id: 1, read: false };
    const card = mount(CaptureCard, { attachTo: document.body, global: { plugins: [i18n] } });
    const dismiss = card.get(`button[aria-label="${i18n.global.t('capture.dismiss')}"]`).element as HTMLButtonElement;
    const focusVisible = vi.spyOn(dismiss, 'matches').mockImplementation(selector => selector !== ':focus-visible');
    dismiss.focus();
    await vi.advanceTimersByTimeAsync(2000);
    expect(document.body.textContent).not.toContain(`${i18n.global.t('capture.dismiss')}${i18n.global.t('capture.dismiss')}`);
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
    dismiss.blur();
    focusVisible.mockImplementation(() => true);
    dismiss.focus();
    await vi.advanceTimersByTimeAsync(2000);
    expect(document.querySelector('[role="tooltip"]')).not.toBeNull();
    card.unmount();
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });
});

describe('copied-link card for a page that only embeds another site\'s video', () => {
  it('keeps Download and adds one quiet link that opens the page in the Nezzel browser', async () => {
    vi.stubGlobal('ResizeObserver', class { observe() {} unobserve() {} disconnect() {} });
    offer.value = { ...offer.value, id: 3, title: 'Official trailer', read: true, details: { ...emptyDetails(), mediaKind: 'video', bestHeight: 1080, embedded: true } };
    const card = mount(CaptureCard, { attachTo: document.body, global: { plugins: [i18n], stubs: { Tooltip: { props: ['content'], template: '<slot />' } } } });
    expect(card.get('[data-testid="capture-embed-hint"]').text()).toContain(i18n.global.t('catch.embed.question'));
    expect(card.find('[data-pick="best"]').exists()).toBe(true);
    await card.get('[data-testid="capture-embed-open"]').trigger('click');
    expect(onFail).toHaveBeenCalledWith('catch');
    offer.value = { ...offer.value, id: 4, details: { ...offer.value.details, embedded: false } };
    await nextTick();
    expect(card.find('[data-testid="capture-embed-hint"]').exists()).toBe(false);
    card.unmount();
    vi.unstubAllGlobals();
  });
});

describe('copied-link card: automatic catch', () => {
  afterEach(() => {
    offer.value = { ...offer.value, failed: false, auto: null };
    onAuto.mockClear();
  });
  const mountCard = () => mount(CaptureCard, { global: { plugins: [i18n], stubs: { Tooltip: { props: ['content'], template: '<slot />' } } } });

  it.each([
    ['looking', 'capture-auto-looking', 'cancel'],
    ['found', 'capture-auto-found', 'download'],
  ] as const)('a failed read with the catch %s shows it instead of the failure, and its button acts on it', async (phase, testid, action) => {
    offer.value = { ...offer.value, read: true, failed: true, reason: 'unsupported', auto: { groupId: 'g1', phase, beside: false, title: 'The Long Night', durationSec: 6738 } };
    const card = mountCard();
    try {
      expect(card.find(`[data-testid="${testid}"]`).exists()).toBe(true);
      expect(card.find('[data-testid="capture-catch"]').exists()).toBe(false);
      await card.get(`[data-testid="capture-auto-${action}"]`).trigger('click');
      expect(onAuto).toHaveBeenCalledWith(action);
    } finally { card.unmount(); }
  });

  it('beside an embedded result the found full video replaces "Not the video you wanted?"', () => {
    offer.value = { ...offer.value, read: true, failed: false, details: { ...emptyDetails(), embedded: true }, auto: { groupId: 'g1', phase: 'found', beside: true, durationSec: 6738 } };
    const card = mountCard();
    try {
      expect(card.get('[data-testid="capture-auto-beside"]').text()).toContain('Found the full video (1:52:18)');
      expect(card.find('[data-testid="capture-embed-hint"]').exists()).toBe(false);
    } finally { card.unmount(); }
  });
});
