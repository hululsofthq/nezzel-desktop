import { mount } from '@vue/test-utils';
import { h } from 'vue';
import { describe, expect, it } from 'vitest';
import CaptureFailure from '../../src/components/downloads/CaptureFailure.vue';
import CaptureFailActions from '../../src/components/downloads/CaptureFailActions.vue';
import { failReason } from '../../src/helpers/captureFailure';
import { i18n } from '../../src/i18n';
import { autoCatchView, embedCatchPage, readCanCatch, rowCanCatch } from '../../src/helpers/catchOffer';
import type { CatchAutoPhase } from '../../src/tauri/types/catch';

describe('"Try in Nezzel browser" on a failed link', () => {
  // One table, error code → "Try in Nezzel browser" offered (audit 2026-11-26): yes where the page itself may still
  // play its video; never DRM, removed/private/legal, live timing, formats, connection or local errors.
  const OFFERED = [
    'urlNotSupported', 'urlUnsupported', 'unknownUrlType', 'noMediaFound', 'embedOnly',
    'signInRequired', 'signInRequiredForBotDetection', 'membersOnly', 'videoPasswordRequired', 'ageRestricted',
    'unableToExtractIdOrInfo', 'unableToExtractInitialData', 'accessForbidden403', 'blockedGeneric',
    'unknown',
  ];
  const NOT_OFFERED = [
    'drmProtected', 'mediaUnavailable', 'nsigExtractionFailed', 'textOnlyPost', 'linkOnlyPost', 'appPost', 'geoBlocked', 'rateLimited429',
    'mediaNotReady', 'requestedFormatUnavailable', 'noAudioTrack', 'notFound404', 'videoNotFoundOrPrivate',
    'playlistPrivateOrMissing', 'channelUnavailable', 'copyrightClaimed', 'storyExpired', 'legalRestriction451',
    'likeeUnavailable', 'telegramAppOnly', 'liveNow', 'notLive', 'livestreamNotStarted', 'photosOnly', 'networkTimeout',
    'networkNameResolutionFailed', 'server5xx', 'sslVerifyFailed', 'unableToConnectToProxy', 'diskFull', 'outputFileBusy',
    'permissionDenied', 'ffmpegNotFound', 'ffmpegFailed', 'rejectedByFilter',
  ];
  it.each([...OFFERED.map(code => [code, true] as const), ...NOT_OFFERED.map(code => [code, false] as const)])(
    'row %s → offered %s', (code, expected) => {
      expect(rowCanCatch(code, 'https://example.com/watch/1')).toBe(expected);
    });

  it.each([
    // Only web pages open in the browser.
    ['urlNotSupported', 'ftp://example.com/file'],
    ['urlNotSupported', 'file:///Users/me/video.mp4'],
    ['unknown', 'not a link'],
    ['urlNotSupported', null],
    [undefined, 'https://example.com/x'],
  ])('row %s %s → not offered', (code, url) => {
    expect(rowCanCatch(code, url)).toBe(false);
  });

  it.each([
    // A film page whose read found its embedded YouTube trailer: open the film page itself.
    ['https://www.cinema-house.example/movies/the-long-night-2024/', 'https://www.cinema-house.example/movies/the-long-night-2024/'],
    ['http://news.example.org/2024/storm', 'http://news.example.org/2024/storm'],
    // Not embedded, or not a web page: no secondary action.
    [undefined, null],
    [null, null],
    ['', null],
    ['file:///Users/me/page.html', null],
    ['not a link', null],
  ])('embedded media from %s → open %s', (embeddedIn, expected) => {
    expect(embedCatchPage(embeddedIn)).toBe(expected);
  });
});
describe('a failed read on a page the Nezzel browser may save (quick bar and card)', () => {
  const t = i18n.global.t;
  const page = 'https://example.com/watch/1';
  /** The two parts the bar and the card both render for a failed read. */
  function failedRead(reason: string, url = page) {
    const catchable = readCanCatch(reason, url);
    const wrapper = mount({ render: () => [
      h(CaptureFailure, { reason, site: 'Example', catchable }),
      h(CaptureFailActions, { reason: failReason(reason), busy: false, catchable }),
    ] }, { global: { plugins: [i18n] } });
    return { wrapper, buttons: () => wrapper.findAll('button').map(button => button.text() || button.attributes('aria-label')) };
  }

  it('an unsupported page is one step, not a failure: Open the page leads, Report stays quiet', async () => {
    const { wrapper, buttons } = failedRead('unsupported');
    expect(wrapper.get('[data-testid="capture-catch"]').text()).toContain(t('catch.read.title'));
    expect(buttons()).toEqual([t('capture.fail.report'), t('catch.row.open')]);
    await wrapper.get('[data-testid="capture-catch-open"]').trigger('click');
    expect(wrapper.findComponent(CaptureFailActions).emitted('act')).toEqual([['catch']]);
    wrapper.unmount();
  });

  it.each([
    // The browser is one more way beside what the reason already offered, in the reason's own words.
    ['login', ['capture.fail.retry', 'catch.row.try', 'capture.fail.useLogin']],
    ['unknown', ['capture.options', 'catch.row.try', 'capture.fail.report', 'capture.fail.retry']],
  ] as const)('%s keeps its own message and every action it had, plus "Try in Nezzel browser"', async (reason, keys) => {
    const { wrapper, buttons } = failedRead(reason);
    expect(wrapper.find('[data-testid="capture-catch"]').exists()).toBe(false);
    expect(wrapper.get('[data-testid="capture-fail"]').text()).toContain(t(`capture.fail.${reason}.title`, { site: 'Example' }));
    expect(wrapper.get('[data-testid="capture-fail"]').text()).toContain(t(`capture.fail.${reason}.hint`));
    expect(buttons()).toEqual(keys.map(key => t(key)));
    await wrapper.get('[data-testid="capture-catch-try"]').trigger('click');
    expect(wrapper.findComponent(CaptureFailActions).emitted('act')).toEqual([['catch']]);
    wrapper.unmount();
  });

  it.each([
    // Not a web page, or a reason the page cannot get past (DRM, removed, region, busy, connection).
    ['unsupported', 'magnet:?xt=urn:btih:abc'], ['login', 'ftp://example.com/file'],
    ['protected', page], ['unavailable', page], ['region', page], ['busy', page], ['network', page],
  ])('%s %s: no way into the browser', (reason, url) => {
    const { wrapper, buttons } = failedRead(reason, url);
    expect(wrapper.find('[data-testid="capture-catch"]').exists()).toBe(false);
    expect(buttons()).not.toContain(t('catch.row.try'));
    expect(buttons()).not.toContain(t('catch.row.open'));
    wrapper.unmount();
  });
});

describe('automatic catch: phase → row', () => {
  const two = [{ id: 'hd', label: 'video', height: 1080, ext: 'mp4', best: true }, { id: 'sd', label: 'video', height: 480, ext: 'mp4', best: false }] as const;
  it.each<[CatchAutoPhase, boolean, string | null, string[]]>([
    ['waiting', false, 'looking', ['cancel']],
    ['looking', false, 'looking', ['cancel']],
    ['found', false, 'found', ['download', 'options']],
    ['live', false, 'note', []],
    ['encrypted', false, 'note', []],
    // One step left: open the page. Never Retry.
    ['needsStep', false, 'needsStep', ['openPage']],
    ['cancelled', false, null, []],
    // Beside a usable embedded result only looking and found show; the embedded row stays as it is otherwise.
    ['looking', true, 'looking', ['cancel']],
    ['found', true, 'found', ['download', 'options']],
    ['live', true, null, []],
    ['encrypted', true, null, []],
    ['needsStep', true, null, []],
  ])('%s beside=%s → %s %j', (phase, beside, kind, actions) => {
    const view = autoCatchView({ groupId: 'g', phase, beside, formats: [...two] }, true);
    expect(view?.kind ?? null).toBe(kind);
    expect(view?.actions ?? []).toEqual(actions);
  });

  it('says "full video" beside an embedded result, and never "protected" for encryption', () => {
    expect(autoCatchView({ groupId: 'g', phase: 'looking', beside: true }, true)?.text).toBe('catch.auto.besideLooking');
    expect(autoCatchView({ groupId: 'g', phase: 'found', beside: true }, true)?.text).toBe('catch.auto.foundFull');
    expect(autoCatchView({ groupId: 'g', phase: 'encrypted', beside: false }, true)?.text).toBe('catch.drm.title');
  });

  it('shows the empty-page message after automatic catch finds no player', () => {
    expect(autoCatchView({ groupId: 'g', phase: 'noVideo', beside: false }, true)).toMatchObject({
      kind: 'note', text: 'errors.runner.playlistEmpty.shortMessage', actions: [],
    });
  });

  it('offers Options only with a choice of formats, and nothing when the setting is off or no session runs', () => {
    expect(autoCatchView({ groupId: 'g', phase: 'found', beside: false, formats: [two[0]] }, true)?.actions).toEqual(['download']);
    expect(autoCatchView({ groupId: 'g', phase: 'found', beside: false }, false)).toBeNull();
    expect(autoCatchView(null, true)).toBeNull();
  });
});
