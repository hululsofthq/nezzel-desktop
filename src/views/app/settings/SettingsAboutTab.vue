<template>
  <!-- The app: icon, name, what it does, version. -->
  <settings-section>
    <div class="flex items-center gap-3 border-b border-hairline pb-4">
      <!-- The icon file carries its own margin: pull it to the column edge. -->
      <img :src="nezzelIcon" alt="" class="-ms-1.5 size-16 shrink-0" />
      <div class="min-w-0">
        <p class="text-[15px] font-semibold rtl:text-[16px]">{{ t('shell.brand') }}</p>
        <p class="mt-0.5 text-[12.5px] leading-relaxed text-muted-foreground text-pretty rtl:text-[13px]">{{ t('welcome.title') }}</p>
      </div>
    </div>
    <settings-row search-id="version" :label="t('about.version', { version: `\u2066${appVersion}\u2069` })" :help="checkHelp">
      <p v-if="storeChannel" class="text-[12.5px] text-muted-foreground rtl:text-[13px]" data-testid="about-store">
        {{ t(storeChannel === 'snap' ? 'about.update.fromSnap' : 'about.update.fromStore') }}
      </p>
      <Button v-else variant="outline" data-testid="about-check" :disabled="checking" :aria-busy="checking || undefined" @click="checkNow">
        <LoaderCircle v-if="checking" class="animate-spin" aria-hidden="true" />{{ checking ? t('about.update.checking') : t('about.update.check') }}
      </Button>
    </settings-row>
    <span class="sr-only" role="status" aria-live="polite">{{ checkHelp }}</span>
  </settings-section>

  <!-- What's new: bundled notes. The running version open, older ones behind a disclosure each. -->
  <settings-section id="whats-new" ref="whatsNewSection" :title="t('about.whatsNew.title')" class="nz-whats-new scroll-mt-4" data-testid="about-whats-new">
    <template v-for="(entry, i) in releases" :key="entry.version">
      <div v-if="i === 0" class="pb-2">
        <p class="flex items-baseline gap-2 pb-0.5 pt-2.5">
          <b class="font-semibold" dir="ltr">{{ entry.version }}</b>
          <span v-if="entry.version === appVersion" class="text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('about.whatsNew.current') }}</span>
        </p>
        <ul :class="notesCls">
          <template v-for="[group, groupLines] in entry.notes.groups" :key="group">
            <li :class="groupCls"><bdi>{{ group }}</bdi></li>
            <li v-for="line in groupLines" :key="line" :class="lineCls">{{ line }}</li>
          </template>
        </ul>
      </div>
      <Collapsible v-else :unmount-on-hide="false" class="border-t border-hairline">
        <CollapsibleTrigger :class="discCls" :data-testid="`about-whats-new-${entry.version}`">
          <b class="font-semibold group-hover:text-primary" dir="ltr">{{ entry.version }}</b>
          <ChevronDown class="size-4 shrink-0 text-muted-foreground transition-transform duration-200 ease-[cubic-bezier(.2,.8,.2,1)] group-data-[state=open]:rotate-180 motion-reduce:transition-none" aria-hidden="true" />
        </CollapsibleTrigger>
        <CollapsibleContent>
          <ul :class="[notesCls, 'pb-2']">
            <template v-for="[group, groupLines] in entry.notes.groups" :key="group">
              <li :class="groupCls"><bdi>{{ group }}</bdi></li>
              <li v-for="line in groupLines" :key="line" :class="lineCls">{{ line }}</li>
            </template>
          </ul>
        </CollapsibleContent>
      </Collapsible>
    </template>
    <a :href="RELEASES_URL" class="group flex min-h-[52px] items-center justify-between gap-8 border-t border-hairline py-3 text-[13px] hover:text-primary" target="_blank" rel="noopener">
      <span>{{ t('about.whatsNew.allChanges') }}</span><ExternalLink class="size-4 shrink-0 text-muted-foreground group-hover:text-primary" aria-hidden="true" />
    </a>
  </settings-section>

  <!-- Who built it. Flat block, no card: mark + name, tagline, one line, two ways out. -->
  <settings-section :title="t('about.hululsoft.madeBy')">
    <div class="pb-1 pt-2" data-testid="about-hululsoft">
      <div class="flex items-center gap-3">
        <img :src="hululsoftMark" alt="" class="size-9 shrink-0" />
        <div class="min-w-0">
          <p class="text-[15px] font-semibold leading-tight rtl:text-[16px]">{{ t('about.hululsoft.name') }}</p>
          <p class="mt-0.5 text-[12.5px] text-muted-foreground rtl:text-[13px]">{{ t('about.hululsoft.tagline') }}</p>
        </div>
      </div>
      <p class="mt-3 max-w-[62ch] text-[13px] leading-relaxed text-pretty rtl:text-[14px]">{{ t('about.hululsoft.whoWeAre') }}</p>
      <div class="mt-3.5 flex flex-wrap gap-2">
        <Button as="a" :href="HULULSOFT_URL" target="_blank" rel="noopener" variant="outline">
          {{ t('about.hululsoft.visit') }}<ExternalLink class="text-muted-foreground" aria-hidden="true" />
        </Button>
        <Button as="a" :href="`${HULULSOFT_URL}/contact`" target="_blank" rel="noopener" variant="outline">
          {{ t('about.hululsoft.workWithUs') }}<ExternalLink class="text-muted-foreground" aria-hidden="true" />
        </Button>
      </div>
    </div>
  </settings-section>

  <settings-section :title="t('about.hululsoft.more')">
    <a :href="NEZZEL_URL" :class="linkRowCls" target="_blank" rel="noopener">
      <span>{{ t('about.hululsoft.nezzelAndroid') }}</span><ExternalLink class="size-4 shrink-0 text-muted-foreground group-hover:text-primary" aria-hidden="true" />
    </a>
    <ul class="flex flex-wrap items-center gap-x-2 gap-y-1 pt-3 text-[12.5px] text-muted-foreground rtl:text-[13px]">
      <template v-for="(product, i) in OTHER_PRODUCTS" :key="product">
        <li v-if="i" class="nz-sep" aria-hidden="true" />
        <li><bdi>{{ product }}</bdi></li>
      </template>
    </ul>
  </settings-section>

  <settings-section>
    <a v-for="link in links" :key="link.href" :href="link.href" :class="linkRowCls" target="_blank" rel="noopener">
      <span>{{ link.label }}</span><ExternalLink class="size-4 shrink-0 text-muted-foreground group-hover:text-primary" aria-hidden="true" />
    </a>
    <settings-row search-id="reportProblem" :label="t('report.title')" :help="t('report.aboutHelp')">
      <Button variant="outline" data-testid="about-report" @click="report.openReport()">
        <MessageSquareWarning aria-hidden="true" />{{ t('report.open') }}
      </Button>
    </settings-row>
    <settings-row search-id="licenses" :label="t('about.poweredBy')" :help="t('about.credit')">
      <Button variant="outline" @click="openLicenses()">{{ t('about.viewLicenses') }}</Button>
    </settings-row>
  </settings-section>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, ref, useTemplateRef, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { ChevronDown, ExternalLink, LoaderCircle, MessageSquareWarning } from '@lucide/vue';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { noteVersions, notesFor } from '../../../helpers/releaseNotes';
import { useWhatsNew } from '../../../composables/useWhatsNew';
import { useReportStore } from '../../../stores/report';
import { useUpdaterStore } from '../../../stores/updater';
import { Button } from '@/components/ui/button';
import SettingsRow from '../../../components/settings/SettingsRow.vue';
import SettingsSection from '../../../components/settings/SettingsSection.vue';
import { useOpener } from '../../../composables/useOpener.ts';
import nezzelIcon from '../../../assets/brand/nezzel-icon.svg';
import hululsoftMark from '../../../assets/brand/hululsoft-mark.svg';

defineOptions({
  inheritAttrs: false,
});

const { t, locale } = useI18n();
// Product names are proper nouns: Latin script in every language, like "Nezzel" and "Hululsoft".
const HULULSOFT_URL = 'https://hululsoft.mr';
const NEZZEL_URL = 'https://nezzel.hululsoft.mr';
const OTHER_PRODUCTS = ['Najahi', 'Souki', 'Bgalti', 'cvite', 'Sorety', 'Mahdarat'];
const linkRowCls = 'group flex min-h-[52px] items-center justify-between gap-8 border-b border-hairline py-3 text-[13px] hover:text-primary';
const links = computed(() => [
  { href: NEZZEL_URL, label: t('about.links.website') },
  { href: 'https://github.com/hululsofthq/nezzel-desktop', label: t('about.links.github') },
]);
const { openInternalPath } = useOpener();
const appVersion = typeof __APP_VERSION__ === 'undefined' ? '' : __APP_VERSION__;

const RELEASES_URL = 'https://github.com/hululsofthq/nezzel-desktop/releases';
const releases = computed(() => noteVersions(appVersion).flatMap((version) => {
  const notes = notesFor(version, locale.value);
  return notes ? [{ version, notes }] : [];
}));
const notesCls = 'flex flex-col text-[13px] rtl:text-[14px]';
const groupCls = 'pb-0.5 pt-2 text-[12px] font-medium text-muted-foreground rtl:text-[13px]';
const lineCls = 'flex gap-2 py-0.5 text-pretty before:mt-[0.72em] before:size-1 before:shrink-0 before:rounded-full before:bg-muted-foreground/70';
const discCls = 'group flex min-h-11 w-full items-center justify-between gap-4 text-start outline-none focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--nz-focus)]';

/** Arriving from the sidebar's "See what's new": scroll here and flash the section once. */
const whatsNew = useWhatsNew();
const whatsNewSection = useTemplateRef<{ $el: HTMLElement }>('whatsNewSection');
async function reveal() {
  if (!whatsNew.revealRequested.value) return;
  whatsNew.revealRequested.value = false;
  await nextTick();
  const el = whatsNewSection.value?.$el;
  if (!el) return;
  const reduce = globalThis.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
  el.scrollIntoView({ block: 'start', behavior: reduce ? 'auto' : 'smooth' });
  el.classList.remove('nz-flash');
  void el.offsetWidth;
  el.classList.add('nz-flash');
}
onMounted(reveal);
watch(whatsNew.revealRequested, reveal);

const updater = useUpdaterStore();
const report = useReportStore();
const checking = ref(false);
const checkResult = ref<'latest' | 'failed'>();
/** A known newer version (background or manual check) wins over "latest"; store builds say where updates come from. */
const storeChannel = computed(() => updater.checkResult?.channel);
const checkHelp = computed(() => {
  if (checking.value) return undefined;
  if (checkResult.value === 'failed') return t('about.update.failed');
  const info = updater.checkResult;
  if (info?.available) return info.version ? t('about.update.availableVersion', { version: `\u2066${info.version}\u2069` }) : t('about.update.available');
  return checkResult.value ? t(`about.update.${checkResult.value}`) : undefined;
});
async function checkNow() {
  checking.value = true;
  checkResult.value = undefined;
  try {
    await updater.check(true);
    checkResult.value = 'latest';
  } catch {
    checkResult.value = 'failed';
  } finally {
    checking.value = false;
  }
}

const openLicenses = async () => {
  await openInternalPath('licenses/3rdpartylicenses.txt');
};
</script>

<style scoped>
/* Arrival from "See what's new": a brief tint that fades out. */
.nz-flash { animation: nz-flash 1.4s ease-out; border-radius: 6px; }
@keyframes nz-flash { 0%, 40% { background: var(--primary-soft); } 100% { background: transparent; } }
@media (prefers-reduced-motion: reduce) { .nz-flash { animation: none; } }
</style>
