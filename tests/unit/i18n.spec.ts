import { afterEach, describe, expect, it, vi } from 'vitest';
import { applyDocumentLocale, getDefaultLocale, loadLocaleMessages, type Locale } from '../../src/i18n';
import { appLanguageOptions } from '../../src/helpers/languageOptions';
import { i18n } from '../../src/i18n';
import { mount } from '@vue/test-utils';
import { createPinia } from 'pinia';
import { defineComponent, h, ref } from 'vue';
import { useDiagnostic } from '../../src/composables/useDiagnostic';
import en from '../../src/locales/en.json';
import ar from '../../src/locales/ar.json';
import rustAr from '../../src-tauri/locales/ar.json';
import fr from '../../src/locales/fr.json';
import pkg from '../../package.json';
import { MAX_NOTE_LINES, MAX_SUMMARY_LINES, NOTES_LANGUAGES, RELEASE_NOTES } from '../../src/helpers/releaseNotes';

type RunnerCopy = { shortMessage?: string; action?: string; message?: string };
const runnerLocales = { en, ar, fr } as unknown as Record<string, { errors: { runner: Record<string, RunnerCopy> } }>;
/** Tool and engine names mean nothing to people; error copy names what happened and what to do. */
const ENGINE_WORDS = /yt-dlp|gallery-dl|ffmpeg|aria2|deno|\bengine\b|moteur|محرك/i;

describe('lazy locale loading', () => {
  it('starts with English only, installs requested messages, and keeps the English fallback', async () => {
    vi.resetModules();
    const { i18n: fresh, loadLocaleMessages: load } = await import('../../src/i18n');
    expect(fresh.global.availableLocales).toEqual(['en']);
    expect(fresh.global.locale.value).toBe('en');
    await Promise.all([load('ar'), load('ar')]);
    expect(fresh.global.availableLocales).toEqual(['ar', 'en']);
    expect(fresh.global.getLocaleMessage('ar')).toEqual(ar);
    fresh.global.locale.value = 'ar';
    expect(fresh.global.t('errors.runner.liveNow.message')).toBe(ar.errors.runner.liveNow.message);
    const fallbackKey = 'fallbackProbe';
    fresh.global.mergeLocaleMessage('en', { [fallbackKey]: 'English fallback' });
    expect(fresh.global.t(fallbackKey, {}, { missingWarn: false, fallbackWarn: false })).toBe('English fallback');
  });

  it('keeps the visible language until messages arrive and ignores a superseded load', async () => {
    vi.resetModules();
    const { i18n: fresh } = await import('../../src/i18n');
    const { applyLanguage } = await import('../../src/appearance');
    await applyLanguage('en');
    const arabic = applyLanguage('ar');
    expect(fresh.global.locale.value).toBe('en');
    expect(document.documentElement.dir).toBe('ltr');
    await arabic;
    expect(fresh.global.locale.value).toBe('ar');
    expect(document.documentElement.dir).toBe('rtl');
    expect(fresh.global.getLocaleMessage('ar')).toEqual(ar);
    const french = applyLanguage('fr');
    await applyLanguage('en');
    await french;
    expect(fresh.global.locale.value).toBe('en');
    expect(document.documentElement.dir).toBe('ltr');
  });
});

describe('download error copy', () => {
  const cases = Object.entries(runnerLocales).flatMap(([locale, messages]) =>
    Object.entries(messages.errors.runner).map(([code, copy]) => [locale, code, copy] as const));

  it.each(cases)('%s %s says what happened and what to do, without engine names', (_locale, _code, copy) => {
    expect(copy.shortMessage?.trim()).toBeTruthy();
    expect(copy.action?.trim()).toBeTruthy();
    expect(copy.action).not.toBe(copy.shortMessage);
    for (const text of [copy.shortMessage, copy.action, copy.message]) expect(text ?? '').not.toMatch(ENGINE_WORDS);
    // errors.rowLine adds the period between shortMessage and action.
    expect(copy.shortMessage).not.toMatch(/[.!?؟。…]$/);
  });

  it('provides every new failure and update-notice key in all shipped locales', () => {
    const locales = import.meta.glob('../../src/locales/*.json', { eager: true, import: 'default' }) as Record<string, {
      errors: { runner: Record<string, RunnerCopy> }; toolsUpdate: { stale: string; tryNow: string; failed: string };
    }>;
    for (const [locale, messages] of Object.entries(locales)) {
      for (const code of ['accountAccessDenied', 'browserCookiesUnreadable', 'browserKeychainDenied', 'browserSafariAccessDenied', 'browserDatabaseLocked', 'browserProfileMissing', 'diskFull', 'outputNameTooLong', 'networkTimeout']) {
        for (const key of ['shortMessage', 'message', 'action'] as const) expect(messages.errors.runner[code]?.[key]?.trim(), `${locale}: ${code}.${key}`).toBeTruthy();
      }
      for (const key of ['stale', 'tryNow', 'failed'] as const) expect(messages.toolsUpdate?.[key]?.trim(), `${locale}: toolsUpdate.${key}`).toBeTruthy();
    }
  });

  it.each(Object.entries(runnerLocales).flatMap(([locale, messages]) =>
    ['browserKeychainDenied', 'browserDpapiFailed', 'browserAppBound', 'browserSafariAccessDenied'].map(code => [locale, code, messages] as const),
  ))('%s displays the localized remedy for %s', (locale, code, messages) => {
    i18n.global.locale.value = locale as Locale;
    const component = defineComponent({
      setup() {
        const { diagnosticDisplay } = useDiagnostic(ref({ id: 'item', groupId: 'group', level: 'error', code, component: null, message: 'untrusted cookie failure', raw: '', timestamp: 1 }));
        return () => h('p', diagnosticDisplay.value.action);
      },
    });
    const wrapper = mount(component, { global: { plugins: [i18n, createPinia()] } });
    try {
      const action = messages.errors.runner[code]?.action;
      expect(action).toBeTruthy();
      expect(wrapper.text()).toBe(action);
    } finally {
      wrapper.unmount();
      i18n.global.locale.value = 'en';
    }
  });

  it.each([
    ['en', 'Nothing was found at this link'],
    ['ar', 'لم يُعثر على محتوى في هذا الرابط'],
    ['fr', 'Aucun contenu trouvé à ce lien'],
  ] as const)('%s shows a neutral message for a plain HTTP 404', (locale, expected) => {
    i18n.global.locale.value = locale;
    const wrapper = mount(defineComponent({
      setup() {
        const { diagnosticDisplay } = useDiagnostic(ref({
          id: 'page', groupId: 'page', level: 'error', code: 'notFound404',
          message: 'HTTP Error 404: Not Found', raw: '', component: null, timestamp: 1,
        }));
        return () => h('p', diagnosticDisplay.value.shortMessage);
      },
    }), { global: { plugins: [i18n, createPinia()] } });
    try {
      expect(wrapper.text()).toBe(expected);
    } finally {
      wrapper.unmount();
      i18n.global.locale.value = 'en';
    }
  });

  it.each(['en', 'ar', 'fr'] as const)('%s distinguishes a missing host from offline and reacts to connectivity', async (locale) => {
    const online = vi.spyOn(navigator, 'onLine', 'get').mockReturnValue(true);
    i18n.global.locale.value = locale;
    const diagnostic = ref<MediaDiagnostic>({
      id: 'dns', groupId: 'dns', level: 'error', code: 'networkNameResolutionFailed',
      message: 'Failed to resolve example.invalid', raw: '', component: null, timestamp: 1,
    });
    const wrapper = mount(defineComponent({
      setup() {
        const { diagnosticDisplay } = useDiagnostic(diagnostic);
        return () => h('p', [diagnosticDisplay.value.shortMessage, diagnosticDisplay.value.message, diagnosticDisplay.value.action].join(' | '));
      },
    }), { global: { plugins: [i18n, createPinia()] } });
    const expectCopy = (code: string) => {
      const copy = runnerLocales[locale].errors.runner[code];
      expect(wrapper.text()).toBe([copy.shortMessage, copy.message, copy.action].join(' | '));
    };
    try {
      expectCopy('hostNotFound');
      online.mockReturnValue(false);
      window.dispatchEvent(new Event('offline'));
      await wrapper.vm.$nextTick();
      expectCopy('networkNameResolutionFailed');
      online.mockReturnValue(true);
      window.dispatchEvent(new Event('online'));
      await wrapper.vm.$nextTick();
      expectCopy('hostNotFound');
      expect(diagnostic.value.code).toBe('networkNameResolutionFailed');
      diagnostic.value.code = 'unableToConnectToProxy';
      await wrapper.vm.$nextTick();
      expectCopy('unableToConnectToProxy');
    } finally {
      wrapper.unmount();
      online.mockRestore();
      i18n.global.locale.value = 'en';
    }
  });

  it('explains the live-stream refusal naturally in Arabic in both runtimes', () => {
    const expected = 'هذا بث مباشر. يمكنك حفظه بعد انتهاء البث.';
    i18n.global.locale.value = 'ar';
    try {
      expect(i18n.global.t('errors.runner.liveNow.message')).toBe(expected);
      expect(rustAr.errors.runner.liveNow).toBe(expected);
    } finally { i18n.global.locale.value = 'en'; }
  });

  it('keeps story expiry and account visibility ambiguous', () => {
    i18n.global.locale.value = 'en';
    const wrapper = mount(defineComponent({
      setup() {
        const { diagnosticDisplay } = useDiagnostic(ref({ id: 'story', groupId: 'story', level: 'error', code: 'storyExpired', message: 'This content is unreachable', raw: '', component: null, timestamp: 1 }));
        return () => h('p', diagnosticDisplay.value.message);
      },
    }), { global: { plugins: [i18n, createPinia()] } });
    expect(wrapper.text()).toBe('This story has expired or isn\'t available to your account');
    wrapper.unmount();
  });

  it('provides new link failure remedies in en/ar/fr', () => {
    for (const messages of Object.values(runnerLocales)) {
      for (const code of ['legalRestriction451', 'podcastFeed', 'galleryItemsSkipped', 'listingPartial', 'vimeoSignInRequired', 'hostNotFound']) {
        for (const key of ['shortMessage', 'message', 'action'] as const) expect(messages.errors.runner[code]?.[key]?.trim()).toBeTruthy();
      }
    }
  });

  it('has the same error codes in every locale', () => {
    const codes = Object.keys(en.errors.runner).sort();
    for (const messages of Object.values(runnerLocales)) expect(Object.keys(messages.errors.runner).sort()).toEqual(codes);
  });
});

describe('locale direction', () => {
  it('falls back to English for a new diagnostic key in an older locale', async () => {
    await loadLocaleMessages('es');
    i18n.global.locale.value = 'es';
    const component = defineComponent({
      setup() {
        const { diagnosticDisplay } = useDiagnostic(ref({
          id: 'image', groupId: 'images', level: 'error', code: 'imageEngineUnavailable',
          message: '', raw: '', component: 'gallery-dl', timestamp: 1,
        }));
        return () => h('p', diagnosticDisplay.value.message);
      },
    });
    const wrapper = mount(component, { global: { plugins: [i18n, createPinia()] } });
    try {
      expect(wrapper.text()).toBe(en.errors.runner.imageEngineUnavailable.message);
    } finally {
      wrapper.unmount();
      i18n.global.locale.value = 'en';
    }
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it.each([
    [['ar-MR'], 'ar'],
    [['ar'], 'ar'],
    [['fr-FR', 'en'], 'fr'],
    [['xx'], 'en'],
    [['de-DE'], 'en'],
    [['de-DE', 'ar'], 'ar'],
  ])('picks %j -> %s from the system languages', (languages, expected) => {
    vi.stubGlobal('navigator', { languages, language: languages[0] });
    expect(getDefaultLocale()).toBe(expected);
  });

  it.each([
    ['ar', 'rtl'],
    ['en', 'ltr'],
    ['fr', 'ltr'],
  ] as [Locale, string][])('sets lang and dir on <html> for %s', (locale, dir) => {
    applyDocumentLocale(locale);
    expect(document.documentElement.getAttribute('lang')).toBe(locale);
    expect(document.documentElement.getAttribute('dir')).toBe(dir);
  });
});

describe('batch report title counts', () => {
  it.each([
    ['ar', 0, 'الإبلاغ عن التنزيلات الفاشلة (0)'],
    ['ar', 1, 'الإبلاغ عن التنزيل الفاشل (1)'],
    ['ar', 2, 'الإبلاغ عن التنزيلين الفاشلين (2)'],
    ['ar', 3, 'الإبلاغ عن 3 تنزيلات فاشلة'],
    ['ar', 11, 'الإبلاغ عن 11 تنزيلًا فاشلًا'],
    ['ar', 100, 'الإبلاغ عن 100 تنزيل فاشل'],
    ['en', 1, 'Report 1 failed download'],
    ['en', 3, 'Report 3 failed downloads'],
    ['fr', 1, 'Signaler 1 téléchargement échoué'],
    ['fr', 3, 'Signaler 3 téléchargements échoués'],
  ] as const)('renders %s count %s', (locale, n, expected) => {
    const previous = i18n.global.locale.value;
    i18n.global.locale.value = locale;
    try {
      expect(i18n.global.t('report.batchTitle', { n: String(n) }, n)).toBe(expected);
    } finally { i18n.global.locale.value = previous; }
  });
});

describe('Arabic playlist selection counts', () => {
  it.each([
    ['downloads.row.items', 'n', ['لا عناصر', 'عنصر واحد', 'عنصران', '5 عناصر', '12 عنصرًا', '100 عنصر']],
    ['downloads.row.photos', 'n', ['لا صور', 'صورة واحدة', 'صورتان', '5 صور', '12 صورة', '100 صورة']],
    ['capture.photos', 'n', ['لا صور', 'صورة واحدة', 'صورتان', '5 صور', '12 صورة', '100 صورة']],
    ['downloads.list.results', 'n', ['لا نتائج', 'نتيجة واحدة', 'نتيجتان', '5 نتائج', '12 نتيجة', '100 نتيجة']],
    ['media.view.logs.skipped.items', 'count', ['لا عناصر', 'عنصر واحد', 'عنصران', '5 عناصر', '12 عنصرًا', '100 عنصر']],
    ['media.steps.paused.metadata.etaItems', 'n', ['المتبقي: لا عناصر', 'المتبقي: عنصر واحد', 'المتبقي: عنصران', 'المتبقي: 5 عناصر', 'المتبقي: 12 عنصرًا', 'المتبقي: 100 عنصر']],
    ['layout.footer.progress.completed', 'n', ['اكتمل التنزيل – لم يتم تنزيل أي عنصر.', 'اكتمل التنزيل – تم تنزيل عنصر واحد.', 'اكتمل التنزيل – تم تنزيل عنصرين.', 'اكتمل التنزيل – تم تنزيل 5 عناصر.', 'اكتمل التنزيل – تم تنزيل 12 عنصرًا.', 'اكتمل التنزيل – تم تنزيل 100 عنصر.']],
    ['layout.footer.progress.ready', 'n', ['جاهز للتنزيل! لا عناصر في قائمة الانتظار.', 'جاهز للتنزيل! عنصر واحد في قائمة الانتظار.', 'جاهز للتنزيل! عنصران في قائمة الانتظار.', 'جاهز للتنزيل! 5 عناصر في قائمة الانتظار.', 'جاهز للتنزيل! 12 عنصرًا في قائمة الانتظار.', 'جاهز للتنزيل! 100 عنصر في قائمة الانتظار.']],
    ['inputFilters.playlistSelection.selectionSummary', 'count', ['لم يتم تحديد أي عنصر من 200', 'تم تحديد عنصر واحد من 200', 'تم تحديد عنصرين من 200', 'تم تحديد 5 من 200 عناصر', 'تم تحديد 12 من 200 عنصرًا', 'تم تحديد 100 من 200 عنصر']],
    ['common.urlImport.added', 'n', ['لم تُضف أي روابط إلى القائمة', 'أُضيف رابط واحد إلى القائمة', 'أُضيف رابطان إلى القائمة', 'أُضيفت 5 روابط إلى القائمة', 'أُضيف 12 رابطًا إلى القائمة', 'أُضيف 100 رابط إلى القائمة']],
    ['downloads.list.cleared', 'n', ['لم يُزل أي تنزيل من القائمة', 'أُزيل تنزيل واحد من القائمة', 'أُزيل تنزيلان من القائمة', 'أُزيلت 5 تنزيلات من القائمة', 'أُزيل 12 تنزيلًا من القائمة', 'أُزيل 100 تنزيل من القائمة']],
    ['downloads.summary.ready', 'n', ['لا روابط جاهزة', 'رابط واحد جاهز', 'رابطان جاهزان', '5 روابط جاهزة', '12 رابطًا جاهزًا', '100 رابط جاهز']],
    ['perDownload.subtitles.selected', 'n', ['لم تختر أي لغة', 'لغة واحدة مختارة', 'لغتان مختارتان', '5 لغات مختارة', '12 لغةً مختارة', '100 لغة مختارة']],
  ] as const)('renders every Arabic plural form of %s', (key, name, expected) => {
    const previous = i18n.global.locale.value;
    i18n.global.locale.value = 'ar';
    try {
      [0, 1, 2, 5, 12, 100].forEach((count, i) => {
        expect(i18n.global.t(key, { [name]: count, total: 200 }, count)).toBe(expected[i]);
      });
    } finally { i18n.global.locale.value = previous; }
  });
  it.each([
    [0, 'لم يُحدَّد أي عنصر'],
    [1, 'عنصر واحد من 120'],
    [2, 'عنصران من 120'],
    [3, '3 من 120 محدّدة'],
    [11, '11 من 120 محدّدًا'],
    [100, '100 من 120 محدّد'],
  ])('renders the Arabic plural form for %s', (count, expected) => {
    const previous = i18n.global.locale.value;
    i18n.global.locale.value = 'ar';
    try {
      expect(i18n.global.t('inputFilters.playlistSelection.checkedCount', { count, total: 120 }, count)).toBe(expected);
    } finally { i18n.global.locale.value = previous; }
  });
  it.each([
    [0, 'تنزيل 0 فيديو', 'تنزيل كل العناصر (0)'],
    [1, 'تنزيل فيديو واحد', 'تنزيل العنصر (1)'],
    [2, 'تنزيل فيديوهين', 'تنزيل العنصرين (2)'],
    [3, 'تنزيل 3 فيديوهات', 'تنزيل كل العناصر (3)'],
    [11, 'تنزيل 11 فيديو', 'تنزيل كل العناصر (11)'],
    [100, 'تنزيل 100 فيديو', 'تنزيل كل العناصر (100)'],
  ])('counts the Arabic download actions for %s', (count, selected, all) => {
    const previous = i18n.global.locale.value;
    i18n.global.locale.value = 'ar';
    try {
      expect(i18n.global.t('inputFilters.playlistSelection.applySelection', { count }, count)).toBe(selected);
      expect(i18n.global.t('inputFilters.playlistSelection.applyFull', { count }, count)).toBe(all);
    } finally { i18n.global.locale.value = previous; }
  });

  it('offers en/ar/fr, plus a saved partial language until the user switches away', () => {
    expect(appLanguageOptions().map(option => option.value).sort()).toEqual(['ar', 'en', 'fr']);
    expect(appLanguageOptions('de').map(option => option.value).sort()).toEqual(['ar', 'de', 'en', 'fr']);
    expect(appLanguageOptions('system').map(option => option.value).sort()).toEqual(['ar', 'en', 'fr']);
  });
});

describe('release notes file', () => {
  it('has an entry for the version in package.json', () => {
    expect(Object.keys(RELEASE_NOTES)).toContain(pkg.version);
  });

  it.each(Object.entries(RELEASE_NOTES))('%s: en/ar/fr with the same shape, 1-3 summary lines, at most 6 lines', (_version, byLang) => {
    const shape = (lang: string) => byLang[lang].groups.map(([, lines]) => lines.length);
    for (const lang of NOTES_LANGUAGES) {
      const notes = byLang[lang];
      expect(notes, lang).toBeDefined();
      expect(notes.summary.length, lang).toBeGreaterThanOrEqual(1);
      expect(notes.summary.length, lang).toBeLessThanOrEqual(MAX_SUMMARY_LINES);
      expect(notes.groups.flatMap(([, lines]) => lines).length, lang).toBeLessThanOrEqual(MAX_NOTE_LINES);
      expect(shape(lang), lang).toEqual(shape('en'));
      expect(notes.summary.length, lang).toBe(byLang.en.summary.length);
      for (const text of [...notes.summary, ...notes.groups.flatMap(([group, lines]) => [group, ...lines])]) expect(text.trim(), lang).not.toBe('');
    }
  });
});
