/**
 * "Catch from browser" (#106): the Nezzel bar above the page, driven by mocked `catch://state` events, in English and
 * Arabic; plus the "Open the page" entry points in the main window.
 * NZ_CATCH_SHOTS=<dir> also saves screenshots of every bar state and the failed row (en light, ar dark; 1100 and 880 wide).
 */
import { expect, type Page } from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import { test } from './utils/fixtures';
import type { MockData } from './utils/mockData';
import type { CatchCall } from '../utils/mocks/catchHandlers';
import type { CatchCandidate, CatchState, CatchTab } from '../../src/tauri/types/catch';
import { MAIN_PANE_ID } from '../../src/helpers/domIds';

const SHOTS = process.env.NZ_CATCH_SHOTS;
const MAIN: CatchCandidate = { id: 'c-main', kind: 'hls', title: 'Coastal drive, part 2', durationSec: 768, height: 1080, main: true };
const AD: CatchCandidate = { id: 'c-ad', kind: 'direct', durationSec: 15, main: false };
/** What Rust sends for a caught video's picture: a small JPEG `data:` URL. */
const THUMB = 'data:image/jpeg;base64,' + fs.readFileSync(new URL('./utils/catch-thumb.jpg', import.meta.url)).toString('base64');
const PICTURED: CatchCandidate = { ...MAIN, thumbnail: THUMB };
const LONG_AR: CatchCandidate = {
  ...MAIN, title: 'المحاضرة الكاملة «تاريخ المخطوطات الشنقيطية» — الجزء الثالث مع ترجمة فورية وأسئلة الجمهور ونقاش مطوّل حول الحفظ الرقمي',
};

const base = (over: Partial<CatchState> = {}): CatchState => ({
  windowId: 'catch-1', url: 'https://example.com/watch/coastal-drive-2', host: 'example.com', canBack: true, canForward: false, loading: false,
  phase: 'looking', popupsBlocked: 2, played: false, playedSec: 0, candidates: [], firstRun: false, ...over,
});

const TEXT = {
  en: { hint: 'Play the video. Nezzel will find it', found: 'Video found', save: 'Save', video: 'Video 1080p', audio: 'Audio only', open: 'Open the page',
    entry: 'Open a page to catch its video', more: 'More ways to add links' },
  ar: { hint: 'شغّل الفيديو وسيجده Nezzel', found: 'وُجد الفيديو', save: 'حفظ', video: 'فيديو 1080p', audio: 'الصوت فقط', open: 'افتح الصفحة',
    entry: 'افتح صفحة لالتقاط الفيديو', more: 'طرق أخرى لإضافة الروابط' },
} as const;

async function openBar(page: Page, locale: 'en' | 'ar', theme: 'light' | 'dark' = 'light', width = 1100) {
  await page.route(/^https?:\/\/(?!localhost)/, route => route.abort());
  const data: MockData = { settings: { appearance: { language: locale, theme } } };
  await page.addInitScript((d) => {
    window.E2E = d;
  }, data);
  await page.setViewportSize({ width, height: 520 });
  await page.goto('/?window=catch-bar-1');
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
  await expect(page.getByTestId('catch-strip')).toBeVisible({ timeout: 30_000 }); // first load waits for Vite
}
const push = (page: Page, state: CatchState) =>
  page.evaluate(s => (window as unknown as { __catchPush: (s: CatchState) => Promise<void> }).__catchPush(s), state);
const calls = (page: Page, cmd: string) =>
  page.evaluate(c => ((window as unknown as { __catchCalls?: CatchCall[] }).__catchCalls ?? []).filter(call => call.cmd === c).map(call => call.args), cmd);
const strip = (page: Page) => page.getByTestId('catch-strip');

async function openMain(page: Page, locale: 'en' | 'ar', seed?: string, theme: 'light' | 'dark' = 'light', width = 1100) {
  const data: MockData = { settings: { appearance: { language: locale, theme } } };
  await page.addInitScript((d) => {
    window.E2E = d;
  }, data);
  await page.setViewportSize({ width, height: 600 });
  await page.goto('/');
  await expect(page.locator(`#${MAIN_PANE_ID}`)).toBeVisible({ timeout: 30_000 });
  if (seed) {
    await page.evaluate(async (name) => {
      // @ts-expect-error served by the Vite dev server, not resolved by tsc
      const { seed } = await import('/tests/e2e/utils/uiStateSeeds.ts');
      await seed(name);
    }, seed);
  }
}

for (const locale of ['en', 'ar'] as const) {
  const text = TEXT[locale];
  test.describe(`catch bar (${locale})`, () => {
    test('hint → ad → found: Save saves the best; the picker opens under the quality button and picks a format', async ({ page }) => {
      await openBar(page, locale);
      await push(page, base({ firstRun: true }));
      await expect(page).toHaveTitle(locale === 'ar' ? 'متصفح Nezzel' : 'Nezzel browser');
      await expect(strip(page)).toHaveAttribute('data-view', 'hint');
      await expect(page.getByTestId('catch-title')).toHaveText(text.hint);
      await expect(page.getByTestId('catch-first-run')).toBeVisible();
      await page.getByTestId('catch-first-run-ok').click();
      await expect(page.getByTestId('catch-first-run')).toHaveCount(0);

      await push(page, base({ phase: 'adPlaying', played: true, candidates: [AD] }));
      await expect(strip(page)).toHaveAttribute('data-view', 'adPlaying');
      await expect(page.getByTestId('catch-save')).toHaveCount(0);

      await push(page, base({ phase: 'found', played: true, candidates: [AD, MAIN] }));
      await expect(page.getByTestId('catch-title')).toHaveText(text.found);
      await expect(strip(page)).toContainText(MAIN.title!);
      await expect(page.getByTestId('catch-quality')).toContainText('1080p');
      // Status, title, quality and Save sit together at the start side.
      const title = (await page.getByTestId('catch-title').boundingBox())!;
      const save = (await page.getByTestId('catch-save').boundingBox())!;
      expect(locale === 'ar' ? save.x < title.x : save.x > title.x).toBe(true);
      // The strip starts after the open tab list (240 px at 1100 wide), on the start side.
      expect(Math.abs((locale === 'ar' ? title.x + title.width : title.x) - (locale === 'ar' ? 1100 - 240 - 16 : 240 + 16))).toBeLessThan(40);

      await page.getByTestId('catch-save').click();
      expect((await calls(page, 'catch_save')).at(-1)).toEqual({ windowId: 'catch-1', candidateId: 'c-main', formatId: 'v1080' });

      await page.getByTestId('catch-quality').click();
      const picker = page.getByTestId('catch-picker');
      await expect(picker).toBeVisible();
      // No picture from Rust: the film icon; a remote address is never loaded by the shell.
      await expect(page.getByTestId('catch-picker-thumb')).toHaveCount(0);
      await push(page, base({ phase: 'found', played: true, candidates: [AD, { ...MAIN, thumbnail: 'https://cdn.example.com/p.jpg' }] }));
      await expect(page.getByTestId('catch-picker-thumb')).toHaveCount(0);
      await expect(page.getByTestId('catch-found-thumb')).toHaveCount(0);
      await push(page, base({ phase: 'found', played: true, candidates: [AD, PICTURED] }));
      await expect(page.getByTestId('catch-picker-thumb')).toHaveAttribute('src', THUMB);
      await expect(page.getByTestId('catch-found-thumb')).toHaveAttribute('src', THUMB);
      // The quality button and Save stay in the row; the picker opens under the button, inside the page area.
      await expect(page.getByTestId('catch-save')).toBeVisible();
      const chip = (await page.getByTestId('catch-quality').boundingBox())!;
      const sheet = (await picker.boundingBox())!;
      expect(sheet.y).toBeGreaterThanOrEqual(chip.y + chip.height);
      expect(locale === 'ar' ? sheet.x : 1100 - sheet.x - sheet.width).toBeGreaterThanOrEqual(8);
      // The shell is over the page now: nothing it draws hides the page area (owner test t1: the page went white),
      // only a light dim under the picker.
      const area = (await page.getByTestId('catch-page-area').boundingBox())!;
      const paint = () => page.evaluate(([x, y]) => document.elementsFromPoint(x, y).map((el) => {
        const style = getComputedStyle(el);
        // Chromium reports a Tailwind `bg-black/10` as `oklab(0 0 0 / 0.1)`, older ones as `rgba(0, 0, 0, 0.1)`: read the alpha of both forms.
        const alpha = /^rgba?\((?:[\d.]+,\s*){3}([\d.]+)\)$/.exec(style.backgroundColor)?.[1] ?? /\/\s*([\d.]+)\)$/.exec(style.backgroundColor)?.[1]
          ?? (style.backgroundColor === 'transparent' ? '0' : '1');
        return { alpha: Number(alpha) * Number(style.opacity), image: style.backgroundImage };
      }), [locale === 'ar' ? area.x + area.width - 20 : area.x + 20, area.y + area.height - 20]); // the corner away from the picker
      await expect.poll(async () => (await paint()).reduce((sum, el) => sum + el.alpha, 0)).toBeLessThanOrEqual(0.12);
      expect((await paint()).every(el => el.image === 'none')).toBe(true);
      await expect(page.getByTestId('catch-scrim')).toBeVisible();
      await expect(picker.getByRole('radio', { name: new RegExp(text.video) })).toHaveAttribute('aria-checked', 'true');
      await picker.getByRole('radio', { name: new RegExp(text.audio) }).click();
      await expect(page.getByTestId('catch-others')).toHaveCount(0); // folded
      await page.getByTestId('catch-others-toggle').click();
      await expect(page.getByTestId('catch-others').getByRole('radio')).toHaveCount(1);
      await page.getByTestId('catch-picker-save').click();
      expect((await calls(page, 'catch_save')).at(-1)).toEqual({ windowId: 'catch-1', candidateId: 'c-main', formatId: 'a' });
      await expect(picker).toHaveCount(0);
      // A press elsewhere in the shell, or Escape with the focus outside it, also puts the picker away.
      await page.getByTestId('catch-quality').click();
      await expect(picker).toBeVisible();
      await page.getByTestId('catch-address').click();
      await expect(picker).toHaveCount(0);
      await page.getByTestId('catch-quality').click();
      await page.getByTestId('catch-side-toggle').focus();
      await page.keyboard.press('Escape');
      await expect(picker).toHaveCount(0);
      // The shell drew over the page while the picker was open.
      const overlays = async () => (await calls(page, 'catch_shell_fit')).map(a => (a as { fit: { overlay: boolean } }).fit.overlay);
      expect(await overlays()).toContain(true);
      await expect.poll(async () => (await overlays()).at(-1)).toBe(false); // sent on the next frame
    });

    test('drm and live: no Save; drm offers Close window', async ({ page }) => {
      await openBar(page, locale);
      await push(page, base({ phase: 'live', played: true, candidates: [MAIN] }));
      await expect(strip(page)).toHaveAttribute('data-view', 'live');
      await expect(page.getByTestId('catch-save')).toHaveCount(0);
      await push(page, base({ phase: 'drm', played: true, candidates: [MAIN] }));
      await expect(strip(page)).toHaveAttribute('data-view', 'drm');
      await expect(page.getByTestId('catch-save')).toHaveCount(0);
      // "Why?" opens its explanation inside the row (owner test t1: a click did nothing), with a way to report the site.
      const why = page.getByTestId('catch-drm-why');
      const rowHeight = (await strip(page).boundingBox())!.height;
      await why.click();
      await expect(why).toHaveAttribute('aria-expanded', 'true');
      await expect(page.getByTestId('catch-drm-why-text')).toBeVisible();
      expect((await strip(page).boundingBox())!.height).toBeGreaterThan(rowHeight);
      await page.getByTestId('catch-drm-report').click();
      expect(await calls(page, 'catch_report')).toHaveLength(1);
      await page.keyboard.press('Escape');
      await expect(page.getByTestId('catch-drm-why-text')).toHaveCount(0);
      await expect(why).toBeFocused();
      await page.getByTestId('catch-close').click();
      expect(await calls(page, 'catch_close')).toEqual([{ windowId: 'catch-1' }]);
    });

    test('not found → reload and report; saving → show download; sign-in hint', async ({ page }) => {
      await openBar(page, locale);
      await push(page, base({ phase: 'signIn' }));
      await expect(strip(page)).toHaveAttribute('data-view', 'signIn');
      await push(page, base({ phase: 'notFound', played: true, playedSec: 12 }));
      await expect(strip(page)).toHaveAttribute('data-view', 'notFound');
      await strip(page).getByRole('button').first().click();
      await page.getByTestId('catch-report').click();
      expect(await calls(page, 'catch_navigate')).toEqual([{ windowId: 'catch-1', action: 'reload' }]);
      expect(await calls(page, 'catch_report')).toEqual([{ windowId: 'catch-1' }]);
      await push(page, base({ phase: 'saving', candidates: [MAIN], savedDownloadId: 'download-1' }));
      await page.getByTestId('catch-show-download').click();
      expect(await calls(page, 'catch_show_download')).toEqual([{ windowId: 'catch-1' }]);
    });

    test('chrome row: address, back, pop-ups Allow once, ⋯ menu', async ({ page }) => {
      await openBar(page, locale);
      await push(page, base());
      await expect(page.getByTestId('catch-forward')).toBeDisabled();
      await page.getByTestId('catch-back').click();
      const address = page.getByTestId('catch-address');
      await expect(address).toHaveValue('example.com/watch/coastal-drive-2');
      await address.fill('example.org/v/2');
      await address.press('Enter');
      expect(await calls(page, 'catch_navigate')).toEqual([
        { windowId: 'catch-1', action: 'back' },
        { windowId: 'catch-1', action: { url: 'https://example.org/v/2' } },
      ]);
      await page.getByTestId('catch-allow-popup').click();
      expect(await calls(page, 'catch_allow_popup_once')).toEqual([{ windowId: 'catch-1' }]);

      await page.getByTestId('catch-more').click();
      const menu = page.getByTestId('catch-menu');
      await expect(menu.getByRole('menuitem').first()).toBeFocused();
      await menu.getByRole('menuitem').nth(2).click();
      expect(await calls(page, 'catch_deep_retry')).toEqual([{ windowId: 'catch-1' }]);
      await page.getByTestId('catch-more').click();
      // The phone version is a menu item (rarely needed), not a side-list row.
      await menu.getByTestId('catch-try-mobile').click();
      expect(await calls(page, 'catch_try_mobile')).toHaveLength(1);
      await expect(page.getByTestId('catch-side').getByTestId('catch-try-mobile')).toHaveCount(0);
      await page.getByTestId('catch-more').click();
      await menu.getByRole('menuitem').nth(4).click();
      expect(await calls(page, 'catch_forget_site')).toEqual([{ windowId: 'catch-1' }]);
      await page.getByTestId('catch-more').click();
      await page.keyboard.press('Escape');
      await expect(menu).toHaveCount(0);
      await expect(page.getByTestId('catch-more')).toBeFocused();

      // The window closes with the menu open and is kept for the next open: the blank state closes
      // the menu, and the fit the closing window refused is sent again with the next page.
      const fits = async () => (await calls(page, 'catch_shell_fit')).map(a => (a as { fit: { top: number; overlay: boolean } }).fit);
      await page.getByTestId('catch-more').click();
      await expect.poll(async () => (await fits()).at(-1)?.overlay).toBe(true);
      await page.evaluate(() => {
        window.E2E = { ...window.E2E, catch: { fit: 'fail' } };
      });
      await push(page, base({ url: '', host: '', canBack: false, popupsBlocked: 0 }));
      await expect(menu).toHaveCount(0);
      await expect.poll(async () => (await fits()).at(-1)?.overlay).toBe(false);
      const refused = (await fits()).length;
      await page.evaluate(() => {
        window.E2E = { ...window.E2E, catch: {} };
      });
      await push(page, base({ url: 'https://example.org/next', host: 'example.org' }));
      await expect.poll(async () => (await fits()).length).toBeGreaterThan(refused);
      expect((await fits()).at(-1)).toMatchObject({ overlay: false, side: 240, rtl: locale === 'ar' });
    });

    test('tabs: list, select, close, new tab home page with search, six-tab question', async ({ page }) => {
      await openBar(page, locale, 'light', 880);
      const tab = (id: string, over: Partial<CatchTab> = {}): CatchTab => ({
        id, url: `https://${id}.example/v`, host: `${id}.example`, loading: false, phase: 'looking', found: false, private: false, asleep: false, mobile: false, ...over,
      });
      const tabs = [tab('a', { found: true }), tab('login', { openerId: 'a' }), tab('b', { private: true })];
      await push(page, base({ url: tabs[0].url, host: 'a.example', tabs, activeTabId: 'a' }));
      const side = page.getByTestId('catch-side');
      await expect(side).toHaveAttribute('data-open', 'false'); // folded under 1100 px
      await expect(side.getByTestId('catch-tab')).toHaveCount(3);
      await expect(side.getByTestId('catch-tab-dot')).toHaveCount(1);
      await page.getByTestId('catch-side-toggle').click();
      await expect(side).toHaveAttribute('data-open', 'true');
      await side.locator('[data-tab=b] button').first().click();
      await expect(side.locator('[data-tab=b]')).toHaveAttribute('data-active', 'true'); // at once
      expect(await calls(page, 'catch_tab_select')).toEqual([{ windowId: 'catch-1', tabId: 'b' }]);
      await side.locator('[data-tab=login]').getByTestId('catch-tab-close').click();
      await expect(side.locator('[data-tab=login]')).toHaveCount(0);
      expect(await calls(page, 'catch_tab_close')).toEqual([{ windowId: 'catch-1', tabId: 'login' }]);

      await page.keyboard.press('ControlOrMeta+t');
      await expect(page.getByTestId('catch-home')).toBeVisible();
      const field = page.getByTestId('catch-home-field');
      await expect(field).toBeFocused();
      await field.fill('salt road');
      await field.press('Enter');
      await expect.poll(async () => (await calls(page, 'catch_navigate')).at(-1)).toEqual({ windowId: 'catch-1', action: { url: 'https://duckduckgo.com/?q=salt%20road' } });

      const six = ['a', 'b', 'c', 'd', 'e', 'f'].map(id => tab(id));
      await push(page, base({ url: six[0].url, host: 'a.example', tabs: six, activeTabId: 'a' }));
      await page.getByTestId('catch-tab-new').first().click();
      const ask = page.getByTestId('catch-tab-limit');
      await expect(ask).toBeVisible();
      // "Keep all" gives the focus back to New tab.
      await ask.getByRole('button').last().click();
      await expect(ask).toHaveCount(0);
      await expect(page.getByTestId('catch-tab-new').first()).toBeFocused();
      await page.keyboard.press('Enter');
      await ask.getByRole('button').nth(2).click();
      expect(await calls(page, 'catch_tab_close')).toContainEqual({ windowId: 'catch-1', tabId: 'c' });
      await expect(ask).toHaveCount(0);
    });

    test('sign-in offer: imported without asking; a refused sign-in says why', async ({ page }) => {
      await openBar(page, locale, 'light', 880);
      await page.evaluate(() => {
        window.E2E = { ...window.E2E, catch: { signin: 'catchSigninDenied' } };
      });
      await push(page, base({ phase: 'signIn', signinHint: 'offer' }));
      await expect(page.getByTestId('catch-signin-error')).toBeVisible();
    });

    test('880 px with a long Arabic title: nothing spills out of the bar', async ({ page }) => {
      await openBar(page, locale, 'light', 880);
      await push(page, base({ phase: 'found', played: true, candidates: [LONG_AR, AD], popupsBlocked: 12 }));
      await expect(page.getByTestId('catch-save')).toBeVisible();
      const spill = await page.locator('.catch-bar').evaluate(bar => bar.scrollWidth - bar.clientWidth);
      expect(spill).toBeLessThanOrEqual(0);
      const save = (await page.getByTestId('catch-save').boundingBox())!;
      expect(save.x).toBeGreaterThanOrEqual(0);
      expect(save.x + save.width).toBeLessThanOrEqual(880);
    });

    test('main window: a failed unsupported page offers Open the page; the link menu opens a blank one', async ({ page }) => {
      await openMain(page, locale, 'row-error-unsupported');
      await expect(page.getByTestId('row-catch-hint')).toBeVisible();
      await page.getByTestId('row-catch-open').click();
      expect(await calls(page, 'catch_open')).toEqual([{ url: 'https://example.com/some/page' }]);

      await page.getByRole('button', { name: new RegExp(text.more) }).click();
      await page.getByRole('menuitem', { name: new RegExp(text.entry) }).click();
      expect((await calls(page, 'catch_open')).at(-1)).toEqual({ url: null });
    });
  });
}

test.describe('screenshots', () => {
  test.skip(!SHOTS, 'NZ_CATCH_SHOTS=<dir> to save them');
  test.describe.configure({ timeout: 240_000 });

  const STATES: [string, CatchState, ((page: Page) => Promise<void>)?][] = [
    ['hint-first-run', base({ firstRun: true })],
    ['nudge', base(), async page => page.clock.runFor(21_000)],
    ['sign-in', base({ phase: 'signIn' })],
    ['not-secure', base({ url: 'http://www.dopebox.to/', host: 'www.dopebox.to' })],
    ['ad-playing', base({ phase: 'adPlaying', played: true, candidates: [AD] })],
    ['found', base({ phase: 'found', played: true, candidates: [MAIN, AD] })],
    ['found-long-title', base({ phase: 'found', played: true, candidates: [LONG_AR, AD], popupsBlocked: 12 })],
    ['found-thumb', base({ phase: 'found', played: true, candidates: [PICTURED, AD] })],
    ['picker-thumb', base({ phase: 'found', played: true, candidates: [PICTURED, AD] }), async page => page.getByTestId('catch-quality').click()],
    ['picker', base({ phase: 'found', played: true, candidates: [MAIN, AD] }), async (page) => {
      await page.getByTestId('catch-quality').click();
      await page.getByTestId('catch-others-toggle').click();
    }],
    ['menu', base(), async page => page.getByTestId('catch-more').click()],
    ['drm', base({ phase: 'drm', played: true, candidates: [MAIN] })],
    ['live', base({ phase: 'live', played: true, candidates: [MAIN] })],
    ['not-found', base({ phase: 'notFound', played: true, playedSec: 12 })],
    ['saving', base({ phase: 'saving', candidates: [MAIN], savedDownloadId: 'download-1' })],
  ];
  const variants = [['en', 'light'], ['ar', 'dark']] as const;

  // Search engine choice and shell polish (en + ar, light): `search-<name>-<locale>.png`.
  for (const locale of ['en', 'ar'] as const) {
    test(`search and shell ${locale}`, async ({ page }) => {
      fs.mkdirSync(SHOTS!, { recursive: true });
      const shot = async (name: string) => {
        await expect.poll(() => page.evaluate(() => document.getAnimations()
          .filter(a => a.playState === 'running' && a.effect?.getComputedTiming().iterations !== Infinity).length)).toBe(0);
        await page.screenshot({ path: path.join(SHOTS!, `search-${name}-${locale}.png`), animations: 'disabled', caret: 'hide' });
      };
      const tab = (id: string, over: Partial<CatchTab> = {}): CatchTab => ({
        id, url: `https://${id}.example/v`, host: `${id}.example`, loading: false, phase: 'looking', found: false, private: false, asleep: false, mobile: false, ...over,
      });
      const home = tab('h', { url: '', host: '' });
      const linked = [tab('videos', { found: true }), tab('login', { openerId: 'videos' }), tab('news', { private: true })];
      const found: Partial<CatchState> = { phase: 'found', played: true, candidates: [MAIN, AD] };

      await openBar(page, locale, 'light', 1100);
      await push(page, base({ url: '', host: '', canBack: false, popupsBlocked: 0, tabs: [home], activeTabId: 'h' }));
      await page.getByTestId('catch-engine-home').click();
      await expect(page.getByRole('menuitemcheckbox')).toHaveCount(4);
      await shot('home-engine-open');
      await page.keyboard.press('Escape');
      await shot('address-empty');
      await page.getByTestId('catch-home-field').fill(locale === 'ar' ? 'طريق الملح' : 'salt road');
      await page.getByTestId('catch-home-field').press('Enter');
      await push(page, base({ url: 'https://duckduckgo.com/?q=salt%20road', host: 'duckduckgo.com', popupsBlocked: 0, tabs: [tab('h', { url: 'https://duckduckgo.com/?q=salt%20road', host: 'duckduckgo.com' })], activeTabId: 'h' }));
      await expect(page.getByTestId('catch-search-hint')).toBeVisible();
      await shot('first-search-hint');

      // 3a: the picker under the quality button, Save still in the row.
      for (const width of [1100, 880]) {
        await openBar(page, locale, 'light', width);
        await push(page, base(found));
        await page.getByTestId('catch-quality').click();
        await shot(`3a-picker-${width}`);
      }
      // 3c + 3e: open list on a page (site tools, linked row indent); 3d: the folded rail with a linked tab.
      await openBar(page, locale, 'light', 1100);
      await push(page, base({ url: linked[1].url, host: linked[1].host, tabs: linked, activeTabId: 'login' }));
      await shot('3ce-open-list-linked');
      await openBar(page, locale, 'light', 880);
      await push(page, base({ url: linked[1].url, host: linked[1].host, tabs: linked, activeTabId: 'login' }));
      await shot('3d-folded-rail-linked');
      // 3b: the six-tab question, flat in the open list.
      const six = ['a', 'b', 'c', 'd', 'e', 'f'].map(id => tab(id));
      await openBar(page, locale, 'light', 1100);
      await push(page, base({ url: six[0].url, host: 'a.example', tabs: six, activeTabId: 'a' }));
      await page.getByTestId('catch-tab-new').first().click();
      await expect(page.getByTestId('catch-tab-limit')).toBeVisible();
      await shot('3b-six-tab-question');

      // Settings → General: the search engine row.
      await page.setViewportSize({ width: 1100, height: 700 });
      await page.goto('/');
      await expect(page.locator(`#${MAIN_PANE_ID}`)).toBeVisible({ timeout: 30_000 });
      await page.evaluate(async () => {
        // @ts-expect-error served by the Vite dev server, not resolved by tsc
        const { seed } = await import('/tests/e2e/utils/uiStateSeeds.ts');
        await seed('settings-general');
      });
      await page.locator('#pref-search-engine').scrollIntoViewIfNeeded();
      await shot('settings-engine');
    });
  }

  // Engine logos: list open on the home field, and the chip on the address row (en + ar, light + dark).
  for (const locale of ['en', 'ar'] as const) {
    for (const theme of ['light', 'dark'] as const) {
      test(`engine logos ${locale} ${theme}`, async ({ page }) => {
        fs.mkdirSync(SHOTS!, { recursive: true });
        await openBar(page, locale, theme, 1100);
        await push(page, base({ url: '', host: '', canBack: false, popupsBlocked: 0, tabs: [{ id: 'h', url: '', host: '', loading: false, phase: 'looking', found: false, private: false, asleep: false, mobile: false }], activeTabId: 'h' }));
        await page.getByTestId('catch-engine-home').click();
        await expect(page.getByRole('menuitemcheckbox')).toHaveCount(4);
        await expect(page.locator('[role=menuitemcheckbox] svg[data-engine]')).toHaveCount(4);
        await expect(page.getByTestId('catch-engine-home').locator('svg[data-engine]')).toHaveCount(1);
        await page.screenshot({ path: path.join(SHOTS!, `logos-list-${locale}-${theme}.png`), animations: 'disabled', caret: 'hide' });
        await page.keyboard.press('Escape');
        await page.screenshot({ path: path.join(SHOTS!, `logos-chip-${locale}-${theme}.png`), animations: 'disabled', caret: 'hide', clip: { x: 0, y: 0, width: 1100, height: 120 } });
      });
    }
  }

  for (const [locale, theme] of variants) {
    for (const width of [1100, 880]) {
      test(`bar ${locale} ${theme} ${width}`, async ({ page }) => {
        fs.mkdirSync(SHOTS!, { recursive: true });
        await page.clock.install();
        for (const [name, state, act] of STATES) {
          await page.evaluate(() => {
            (window as unknown as { __catchCalls?: unknown[] }).__catchCalls = [];
          }).catch(() => {});
          await openBar(page, locale, theme, width);
          await push(page, state);
          await act?.(page);
          await page.clock.runFor(500);
          await expect.poll(() => page.evaluate(() => document.getAnimations()
            .filter(a => a.playState === 'running' && a.effect?.getComputedTiming().iterations !== Infinity).length)).toBe(0);
          await page.screenshot({ path: path.join(SHOTS!, `bar-${name}-${locale}-${theme}-${width}.png`), animations: 'disabled', caret: 'hide' });
        }
      });

      test(`failed row ${locale} ${theme} ${width}`, async ({ page }) => {
        fs.mkdirSync(SHOTS!, { recursive: true });
        await openMain(page, locale, 'row-error-unsupported', theme, width);
        await expect(page.getByTestId('row-catch-open')).toBeVisible();
        await page.screenshot({ path: path.join(SHOTS!, `row-failed-${locale}-${theme}-${width}.png`), animations: 'disabled', caret: 'hide' });
        await page.getByRole('button', { name: new RegExp(TEXT[locale].more) }).click();
        await page.screenshot({ path: path.join(SHOTS!, `link-menu-${locale}-${theme}-${width}.png`), animations: 'disabled', caret: 'hide' });
      });
    }
  }
});
