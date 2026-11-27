<template>
  <!-- The Nezzel browser shell (contract v3): address row across the window, the tab list on the start side, then the
       "Video found" row and the home page in the content area. The active tab's page webview sits below `top`, beside
       `side` (catch_shell_fit); the picker, ⋯ menu and cards draw over it with `overlay`. The shell then covers the
       page, so it paints nothing over the page area: only its rows, the list and the home page have a surface. -->
  <div class="catch-bar relative flex h-dvh flex-col overflow-hidden text-foreground" :aria-label="t('catch.bar')" role="region" :style="{ fontSize: rtl ? '14px' : '13px' }">
    <header
      ref="header"
      class="flex h-11 shrink-0 items-center gap-1 border-b border-hairline bg-sidebar ps-2.5 pe-2.5"
      :style="inset ? { paddingLeft: `${inset}px` } : undefined"
      data-tauri-drag-region
    >
      <Tooltip :content="t('catch.shell.toggle')">
        <Button
          variant="ghost"
          size="icon-xs"
          class="me-1.5 text-muted-foreground"
          :class="store.sideOpen && 'bg-accent text-foreground'"
          :aria-label="t('catch.shell.toggle')"
          :aria-expanded="store.sideOpen ? 'true' : 'false'"
          data-testid="catch-side-toggle"
          @click="store.toggleSide()"
        >
          <PanelLeft class="rtl:-scale-x-100" aria-hidden="true" />
        </Button>
      </Tooltip>
      <Tooltip :content="t('catch.back')">
        <Button variant="ghost" size="icon-xs" class="text-muted-foreground" :aria-label="t('catch.back')" :disabled="!state?.canBack || store.isHome" data-testid="catch-back" @click="store.back()">
          <ChevronLeft class="rtl:-scale-x-100" aria-hidden="true" />
        </Button>
      </Tooltip>
      <Tooltip :content="t('catch.forward')">
        <Button variant="ghost" size="icon-xs" class="text-muted-foreground" :aria-label="t('catch.forward')" :disabled="!state?.canForward || store.isHome" data-testid="catch-forward" @click="store.forward()">
          <ChevronRight class="rtl:-scale-x-100" aria-hidden="true" />
        </Button>
      </Tooltip>
      <Tooltip :content="t('catch.reload')">
        <Button variant="ghost" size="icon-xs" class="text-muted-foreground" :aria-label="t('catch.reload')" :disabled="!state || store.isHome" data-testid="catch-reload" @click="store.reload()">
          <RotateCw aria-hidden="true" />
        </Button>
      </Tooltip>

      <form class="mx-auto flex h-7 min-w-[120px] max-w-[640px] flex-1 items-center gap-1.5 rounded-[7px] bg-foreground/[0.055] ps-0.5 pe-2.5 @container focus-within:bg-card focus-within:ring-2 focus-within:ring-primary/40" @submit.prevent="submitAddress">
        <CatchEngineMenu place="address" @picked="address?.focus()" />
        <Lock v-if="!store.isHome && secure && !editing" class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <!-- A plain-http page says so in words (colour is never the only signal); read before the address. -->
        <span v-else-if="!store.isHome && plainHttp && !editing" class="flex shrink-0 items-center gap-1 text-[12px] text-muted-foreground rtl:text-[13px]" data-testid="catch-not-secure">
          <LockOpen class="size-3.5 shrink-0" aria-hidden="true" />
          <span class="whitespace-nowrap @max-[300px]:sr-only">{{ t('catch.notSecure') }}</span>
        </span>
        <Globe v-else-if="!store.isHome" class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
        <label for="catch-address" class="sr-only">{{ t('catch.address') }}</label>
        <!-- An address reads left to right in Arabic too; `rtl:text-right` keeps it beside its icon there (the field's own dir is ltr, so `text-end` would not). -->
        <input
          id="catch-address"
          ref="address"
          v-model="addressText"
          type="text"
          inputmode="url"
          :dir="addressDir(addressText, editing)"
          autocomplete="off"
          spellcheck="false"
          :placeholder="t('catch.home.placeholder', { engine: store.engineName })"
          class="h-full min-w-0 flex-1 truncate bg-transparent text-[12.5px] outline-none placeholder:text-muted-foreground/70 rtl:text-right"
          :class="!editing && 'text-muted-foreground'"
          :aria-invalid="addressInvalid || undefined"
          :aria-describedby="addressInvalid ? 'catch-address-error' : undefined"
          data-testid="catch-address"
          @focus="startEditing"
          @blur="editing = false"
          @keydown.esc.prevent="cancelEditing"
        />
        <span v-if="state?.loading" class="size-3 shrink-0 animate-spin rounded-full border-2 border-primary/25 border-t-primary motion-reduce:animate-none" role="status" :aria-label="t('capture.loading')" />
      </form>

      <Tooltip v-if="popups > 0 && !store.isHome" :content="t('catch.popups', { n: number.count(popups) }, popups)">
        <span class="flex shrink-0 items-center gap-1 px-1 text-[12px] text-muted-foreground rtl:text-[13px]" data-testid="catch-popup-count">
          <Ban class="size-3.5" aria-hidden="true" />
          <span class="tnum" dir="ltr" :aria-label="t('catch.popups', { n: number.count(popups) }, popups)">{{ popupBadge }}</span>
        </span>
      </Tooltip>

      <div class="relative">
        <Tooltip :content="t('catch.more')">
          <Button
            ref="menuButton"
            variant="ghost"
            size="icon-xs"
            class="text-muted-foreground"
            :aria-label="t('catch.more')"
            aria-haspopup="menu"
            :aria-expanded="store.menuOpen ? 'true' : 'false'"
            data-testid="catch-more"
            @click="store.menuOpen = !store.menuOpen"
            @keydown.down.prevent="store.menuOpen = true"
          >
            <Ellipsis aria-hidden="true" />
          </Button>
        </Tooltip>
      </div>
    </header>

    <div class="flex min-h-0 flex-1">
      <CatchSideList />
      <div ref="content" class="relative flex min-w-0 flex-1 flex-col">
        <div ref="rows" class="shrink-0 bg-background">
          <p v-if="addressInvalid" id="catch-address-error" class="border-b border-destructive-border bg-destructive-soft px-4 py-1 text-destructive-foreground" role="alert">{{ t('catch.invalidAddress') }}</p>
          <div role="status">
            <div v-if="store.fileDownload" class="border-b border-hairline bg-muted px-4 py-1.5" data-testid="catch-file-download">
              <p class="line-clamp-2 [overflow-wrap:anywhere]" :title="fileDownloadName ?? undefined">
                <i18n-t v-if="fileDownloadName" :keypath="fileDownloadKey" scope="global">
                  <template #name><bdi dir="auto">{{ fileDownloadName }}</bdi></template>
                </i18n-t>
                <template v-else>{{ t(fileDownloadKey) }}</template>
              </p>
            </div>
          </div>
          <!-- One-line prompts: the search hint, and a site tool's plain error. -->
          <div v-if="store.searchHint && !store.isHome" class="flex min-h-9 items-center gap-1.5 border-b border-hairline bg-muted px-4 py-1" role="status" data-testid="catch-search-hint">
            <p class="min-w-0 truncate">{{ t('catch.search.hint', { engine: store.engineName }) }}</p>
            <span class="nz-sep" aria-hidden="true" />
            <Button variant="ghost" size="xs" class="shrink-0 text-primary" data-testid="catch-search-change" @click="changeEngine">{{ t('catch.search.change') }}</Button>
            <Button variant="ghost" size="icon-xs" class="ms-auto shrink-0 text-muted-foreground" :aria-label="t('catch.shell.dismiss')" @click="store.searchHint = false"><X aria-hidden="true" /></Button>
          </div>
          <div v-if="store.signinError" class="flex min-h-[42px] items-center gap-2.5 border-b border-amber-border bg-amber-soft px-4 py-1.5 text-amber-foreground" role="alert" data-testid="catch-signin-error">
            <CircleAlert class="size-4 shrink-0" aria-hidden="true" />
            <p class="min-w-0 flex-1 text-pretty">{{ t(`catch.tools.${store.signinError}`) }}</p>
            <Button variant="ghost" size="icon-xs" class="text-inherit" :aria-label="t('catch.shell.dismiss')" @click="store.signin = null"><X aria-hidden="true" /></Button>
          </div>
    <section
      v-if="!store.isHome"
      class="flex min-h-11 flex-wrap items-center gap-x-2.5 gap-y-1 border-b px-4 py-1.5"
      :class="toneCls"
      :data-view="view.kind"
      data-testid="catch-strip"
    >
      <div class="flex min-w-0 items-center gap-2" role="status" aria-live="polite">
        <span v-if="view.kind === 'adPlaying'" class="size-3.5 shrink-0 animate-spin rounded-full border-2 border-current/25 border-t-current motion-reduce:animate-none" aria-hidden="true" />
        <img v-else-if="foundThumb" :src="foundThumb" alt="" class="h-7 w-12 shrink-0 rounded-[4px] bg-muted object-cover" data-testid="catch-found-thumb" @error="thumbFailed = foundThumb">
        <component :is="icon" v-else class="size-4 shrink-0" aria-hidden="true" />
        <p class="flex min-w-0 flex-wrap items-baseline gap-x-2">
          <i18n-t v-if="view.kind === 'loadFailed'" keypath="catch.loadFailed.title" scope="global" tag="b" class="font-semibold [overflow-wrap:anywhere]" data-testid="catch-title">
            <template #site><bdi dir="ltr">{{ state?.host }}</bdi></template>
          </i18n-t>
          <b v-else class="font-semibold" data-testid="catch-title">{{ title }}</b>
          <span v-if="view.kind === 'found' && store.candidate" class="flex min-w-0 items-center gap-1.5 opacity-80">
            <bdi dir="auto" class="max-w-[40ch] truncate text-start [unicode-bidi:plaintext]">{{ candidateTitle(store.candidate) }}</bdi>
            <template v-if="store.candidate.durationSec">
              <span class="nz-sep" aria-hidden="true" />
              <bdi dir="ltr" class="tnum">{{ formatDuration(store.candidate.durationSec) }}</bdi>
            </template>
          </span>
          <span v-else-if="sub" class="text-muted-foreground" :class="view.tone === 'amber' && 'text-amber-foreground/80'">{{ sub }}</span>
        </p>
      </div>

      <div v-if="view.kind === 'found'" class="ms-auto flex shrink-0 flex-wrap items-center gap-2.5">
        <Button
          ref="qualityButton"
          variant="outline"
          size="xs"
          class="tnum"
          :class="store.pickerOpen && 'bg-accent'"
          aria-haspopup="dialog"
          :aria-expanded="store.pickerOpen ? 'true' : 'false'"
          :aria-label="`${t('catch.quality')}: ${shortLabel(store.format)}`"
          data-testid="catch-quality"
          @click="store.pickerOpen ? store.closePicker() : openPicker()"
        >
          {{ shortLabel(store.format) }}<ChevronDown class="text-muted-foreground" aria-hidden="true" />
        </Button>
        <Button :disabled="!store.canSave" data-testid="catch-save" @click="store.save()">
          <LoaderCircle v-if="store.busy" class="animate-spin motion-reduce:animate-none" aria-hidden="true" /><Download v-else aria-hidden="true" />{{ t(store.candidate?.alreadySaved ? 'downloads.row.alreadySaved' : 'catch.save') }}
        </Button>
        <p v-if="store.saveFailed && !store.pickerOpen" class="text-[12.5px] text-destructive-foreground dark:text-destructive" role="alert">{{ t('catch.saveFailed') }}</p>
      </div>
      <button
        v-if="view.kind === 'drm'"
        ref="whyButton"
        type="button"
        class="min-h-6 rounded-[4px] text-[12px] underline underline-offset-2 outline-none hover:no-underline focus-visible:ring-2 focus-visible:ring-current/50 rtl:text-[13px]"
        :aria-expanded="whyOpen ? 'true' : 'false'"
        aria-controls="catch-drm-why"
        data-testid="catch-drm-why"
        @click="whyOpen = !whyOpen"
      >
        {{ t('catch.drm.why') }}
      </button>
      <Check v-if="view.kind === 'saving'" class="size-4 text-primary" :aria-label="t('catch.saving.title')" />
      <template v-for="action in view.actions" :key="action">
        <Button v-if="action === 'reload'" variant="outline" size="xs" @click="store.reload()"><RotateCw aria-hidden="true" />{{ t(view.kind === 'loadFailed' ? 'catch.loadFailed.retry' : 'catch.notFound.reload') }}</Button>
        <Button v-else-if="action === 'report'" variant="ghost" size="xs" class="text-muted-foreground" data-testid="catch-report" @click="store.report()">{{ t('catch.notFound.report') }}</Button>
        <Button v-else-if="action === 'showDownload'" variant="outline" size="xs" data-testid="catch-show-download" @click="store.showDownload()">{{ t('catch.saving.show') }}</Button>
        <Button v-else-if="action === 'close'" variant="ghost" size="xs" data-testid="catch-close" @click="store.close()">{{ t('catch.close') }}</Button>
      </template>
      <!-- "Why?" opens in the row itself (the row grows, the page moves down): a tooltip would draw under the page. -->
      <div v-if="view.kind === 'drm' && whyOpen" id="catch-drm-why" class="flex basis-full flex-wrap items-center gap-x-2.5 gap-y-1 pb-0.5" data-testid="catch-drm-why-text">
        <p class="min-w-0 flex-1 text-pretty">{{ t('catch.drm.whyText') }}</p>
        <Button variant="outline" size="xs" class="shrink-0 bg-transparent" data-testid="catch-drm-report" @click="store.report()">{{ t('catch.drm.report') }}</Button>
      </div>
    </section>
        </div>
        <CatchHome v-if="store.isHome" ref="homeView" />
        <div v-else class="relative flex-1" aria-hidden="true" data-testid="catch-page-area">
          <!-- The page stays in view under the picker, a little dimmed; a press on it closes the picker. -->
          <div v-if="store.pickerOpen" class="absolute inset-0 bg-black/10 animate-in fade-in-0 duration-150 motion-reduce:animate-none" data-testid="catch-scrim" />
        </div>

        <!-- Overlays over the page (catch_shell_fit overlay). -->
        <CatchPicker v-if="store.pickerOpen" class="absolute z-20" :style="{ top: `${pickerAt.top}px`, insetInlineStart: `${pickerAt.start}px` }" />

        <!-- One-time 2-step card: play, then Save. -->
        <div
          v-if="firstRunCard"
          class="absolute start-4 z-10 w-[340px] max-w-[calc(100%-32px)] rounded-[10px] border border-hairline bg-popover p-3.5 shadow-lg animate-in fade-in-0 slide-in-from-top-1 duration-200 motion-reduce:animate-none"
          :style="{ top: `${rowsHeight + 6}px` }"
          role="note"
          data-testid="catch-first-run"
        >
          <p class="mb-2 font-semibold">{{ t('catch.firstRun.title') }}</p>
          <ol class="flex flex-col gap-1.5">
            <li v-for="(step, i) in ['step1', 'step2']" :key="step" class="flex items-start gap-2">
              <span class="tnum grid size-5 shrink-0 place-items-center rounded-full bg-primary-soft text-[11.5px] font-semibold text-primary-soft-foreground">{{ number.count(i + 1) }}</span>
              <span class="text-pretty">{{ t(`catch.firstRun.${step}`) }}</span>
            </li>
          </ol>
          <div class="mt-3 flex items-center gap-2 border-t border-hairline pt-2.5">
            <ShieldCheck class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            <p class="min-w-0 flex-1 text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.firstRun.privacy') }}</p>
            <Button variant="soft" size="xs" data-testid="catch-first-run-ok" @click="firstRunDismissed = true">{{ t('catch.firstRun.ok') }}</Button>
          </div>
        </div>
      </div>
    </div>

    <ul
      v-if="store.menuOpen"
      ref="menu"
      class="absolute end-2 top-10 z-30 w-72 rounded-[10px] border border-hairline bg-popover p-1 shadow-lg animate-in fade-in-0 zoom-in-95 duration-150 motion-reduce:animate-none"
      role="menu"
      data-testid="catch-menu"
      @keydown="onMenuKey"
    >
      <li role="none">
        <button role="menuitem" type="button" :class="itemCls" :disabled="!state" data-testid="catch-menu-new-tab" @click="menuAct(() => store.newTab())">
          <Plus aria-hidden="true" />
          <span class="min-w-0 flex-1">{{ t('catch.shell.newTab') }}</span>
          <kbd class="text-[11px] text-muted-foreground" dir="ltr">{{ newTabKeys }}</kbd>
        </button>
      </li>
      <li role="none">
        <button role="menuitem" type="button" :class="itemCls" :disabled="!state" data-testid="catch-menu-private" @click="menuAct(() => store.newTab({ private: true }))">
          <VenetianMask aria-hidden="true" />
          <span class="min-w-0 flex-1">
            <span class="block">{{ t('catch.shell.newPrivateTab') }}</span>
            <span class="block text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.shell.privateHint') }}</span>
          </span>
        </button>
      </li>
      <li role="separator" class="mx-1.5 my-1 h-px bg-hairline" />
      <li role="none">
        <button role="menuitem" type="button" :class="itemCls" :disabled="!state || store.isHome" @click="menuAct(store.deepRetry)">
          <SearchCheck aria-hidden="true" />
          <span class="min-w-0 flex-1">
            <span class="block">{{ t('catch.menu.cantFind') }}</span>
            <span class="block text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.menu.cantFindHint') }}</span>
          </span>
        </button>
      </li>
      <!-- Rarely needed, so it lives here and not in the side list (owner, t4): the page often looks the same after it. -->
      <li role="none">
        <button role="menuitem" type="button" :class="itemCls" :disabled="!state || store.isHome" :aria-pressed="!!store.activeTab?.mobile" data-testid="catch-try-mobile" @click="menuAct(store.toggleMobile)">
          <Smartphone aria-hidden="true" />
          <span class="min-w-0 flex-1">
            <span class="block">{{ t('catch.tools.iphone') }}</span>
            <span class="block text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.tools.iphoneHint') }}</span>
          </span>
          <Check v-if="store.activeTab?.mobile" class="text-primary" aria-hidden="true" />
        </button>
      </li>
      <li role="none">
        <button role="menuitem" type="button" :class="itemCls" :disabled="!state || store.isHome" @click="menuAct(store.forgetSite)">
          <UserX aria-hidden="true" />
          <span class="min-w-0 flex-1">
            <span class="block">{{ t('catch.menu.forget') }}</span>
            <span class="block text-[12px] text-muted-foreground rtl:text-[13px]"><i18n-t keypath="catch.menu.forgetHint" scope="global"><template #site><bdi dir="ltr">{{ store.host }}</bdi></template></i18n-t></span>
          </span>
        </button>
      </li>
      <li role="separator" class="mx-1.5 my-1 h-px bg-hairline" />
      <li role="none">
        <button role="menuitem" type="button" :class="itemCls" :disabled="!state" @click="menuAct(store.close)">
          <X aria-hidden="true" />
          <span class="min-w-0 flex-1">{{ t('catch.menu.close') }}</span>
        </button>
      </li>
    </ul>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch, type Component } from 'vue';
import { useI18n } from 'vue-i18n';
import { isCaughtThumbnail } from '../../helpers/remoteImages';
import {
  Ban, Check, ChevronDown, ChevronLeft, ChevronRight, CircleAlert, CircleCheck, Download, Ellipsis, Globe, KeyRound, LoaderCircle, Lock, LockOpen, PanelLeft, Play, Plus, Radio,
  RotateCw, SearchCheck, SearchX, ShieldAlert, ShieldCheck, Smartphone, UserX, VenetianMask, X,
} from '@lucide/vue';
import { Button } from '@/components/ui/button';
import Tooltip from '@/components/ui/tooltip/Tooltip.vue';
import CatchPicker from './CatchPicker.vue';
import CatchSideList from './CatchSideList.vue';
import CatchHome from './CatchHome.vue';
import CatchEngineMenu from './CatchEngineMenu.vue';
import { addressDir, CATCH_SIDE_FOLDED_PX, CATCH_SIDE_OPEN_PX, homeTarget, pickerStart, useCatchStore, type CatchTone, type CatchViewKind } from '../../stores/catch';
import { useCatchText } from './useCatchText';
import { useLocaleNumber } from '../../composables/useLocaleNumber';
import { useTrafficLightsInset } from '../../composables/useTrafficLightsInset';
import { formatDuration } from '../../helpers/units';
import { localeDirection } from '../../i18n';

const props = defineProps<{ windowId: string }>();
const { t, locale } = useI18n();
const store = useCatchStore();
const fileDownloadName = computed(() => {
  const notice = store.fileDownload;
  return notice && 'name' in notice ? notice.name : null;
});
const fileDownloadKey = computed(() => {
  const notice = store.fileDownload;
  if (!notice) return '';
  const suffix = 'name' in notice && !notice.name ? `${notice.kind}File` : notice.kind;
  return `catch.fileDownload.${suffix}`;
});
const number = useLocaleNumber();
const { candidateTitle, shortLabel } = useCatchText();
const { inset } = useTrafficLightsInset();
const rtl = computed(() => localeDirection(locale.value) === 'rtl');
/** Every field of the strip and address describes the tab in front. */
const state = computed(() => store.shown);
const popups = computed(() => state.value?.popupsBlocked ?? 0);
const newTabKeys = `${/Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl+'}T`;
const popupBadge = computed(() => (popups.value > 99 ? '99+' : number.count(popups.value)));
store.initSide(window.innerWidth);
const view = computed(() => store.view);

const TONES: Record<CatchTone, string> = {
  hint: 'bg-accent border-hairline',
  ok: 'bg-primary-soft text-primary-soft-foreground border-primary/20',
  amber: 'bg-amber-soft text-amber-foreground border-amber-border',
  neutral: 'bg-muted border-hairline',
  red: 'bg-destructive-soft text-destructive-foreground border-destructive-border',
};
const toneCls = computed(() => TONES[view.value.tone]);
const ICONS: Record<CatchViewKind, Component> = {
  hint: Play, nudge: SearchX, signIn: KeyRound, adPlaying: LoaderCircle, found: CircleCheck, drm: ShieldAlert, live: Radio, notFound: SearchX, saving: CircleCheck, crashed: CircleAlert, geoBlocked: Globe, loadFailed: CircleAlert,
};
const icon = computed(() => ICONS[view.value.kind]);
/** The found video's picture: only a `data:` image Rust fetched through an admitted request. */
const thumbFailed = ref<string | null>(null);
const foundThumb = computed(() => {
  const src = view.value.kind === 'found' ? store.candidate?.thumbnail : undefined;
  return isCaughtThumbnail(src) && src !== thumbFailed.value ? src : null;
});
const title = computed(() => t(`catch.${view.value.kind}.title`));
const sub = computed(() => {
  switch (view.value.kind) {
    case 'hint': return state.value?.firstRun ? t('catch.hint.firstRun') : '';
    case 'nudge': return t('catch.nudge.sub');
    case 'live': return t('catch.live.sub');
    case 'notFound': return t('catch.notFound.sub');
    case 'crashed': return t('catch.crashed.sub');
    case 'geoBlocked': return t('catch.geoBlocked.sub');
    default: return '';
  }
});
const secure = computed(() => state.value?.url.startsWith('https://') ?? false);
const plainHttp = computed(() => state.value?.url.startsWith('http://') ?? false);

// The window is named by its title; the native title follows the bar's document.
watch(() => t('catch.windowTitle'), (value) => {
  document.title = value;
}, { immediate: true });

// Address field: shows the page's address; editing keeps the typed text until Enter or Escape.
const editing = ref(false);
const addressText = ref('');
const addressInvalid = ref(false);
const address = ref<HTMLInputElement | null>(null);
const shownAddress = computed(() => state.value?.url.replace(/^https?:\/\//, '').replace(/\/$/, '') ?? '');
watch(() => store.activeTabId, () => {
  // Another tab in front: its address replaces whatever was being typed.
  editing.value = false;
  addressInvalid.value = false;
  addressText.value = shownAddress.value;
});
watch(addressText, () => {
  addressInvalid.value = false;
});
watch(shownAddress, (value) => {
  if (!editing.value) addressText.value = value;
}, { immediate: true });
function startEditing() {
  editing.value = true;
  addressInvalid.value = false;
  addressText.value = state.value?.url ?? '';
  void nextTick(() => address.value?.select());
}
function cancelEditing() {
  addressInvalid.value = false;
  addressText.value = shownAddress.value;
  address.value?.blur();
}
async function submitAddress() {
  const typed = addressText.value;
  if (!typed.trim()) return;
  if (!homeTarget(typed)) {
    addressInvalid.value = true;
    return;
  }
  if (await store.go(typed)) address.value?.blur();
}

// "Why?" on the own-player row: the explanation opens in the row; another row or another tab starts closed.
const whyOpen = ref(false);
const whyButton = ref<HTMLElement | null>(null);
watch(() => [view.value.kind, store.activeTabId], () => {
  whyOpen.value = false;
});

// First-run card: while the user still has to play the video, until "Got it".
const firstRunDismissed = ref(false);
const firstRunCard = computed(() => !!state.value?.firstRun && !firstRunDismissed.value && !store.pickerOpen && !store.menuOpen && !store.isHome
  && ['hint', 'nudge', 'signIn', 'adPlaying'].includes(view.value.kind));

/** "Change" on the one-time search line opens the address row's engine list (next frame: after this click). */
function changeEngine() {
  store.searchHint = false;
  requestAnimationFrame(() => (store.engineMenu = 'address'));
}

// ⋯ menu.
const menuOpen = computed({
  get: () => store.menuOpen,
  set: (open: boolean) => {
    store.menuOpen = open;
  },
});
const menu = ref<HTMLElement | null>(null);
const menuButton = ref<{ $el: HTMLElement } | null>(null);
const itemCls = 'flex w-full items-start gap-2.5 rounded-[6px] px-2 py-1.5 text-start outline-none hover:bg-accent focus-visible:bg-accent disabled:opacity-50 [&>svg]:mt-0.5 [&>svg]:size-4 [&>svg]:shrink-0 [&>svg]:text-muted-foreground';
watch(menuOpen, (open) => {
  if (open) void nextTick(() => menu.value?.querySelector<HTMLElement>('[role=menuitem]:not(:disabled)')?.focus());
});
function closeMenu(focusButton = true) {
  menuOpen.value = false;
  if (focusButton) menuButton.value?.$el.focus();
}
function menuAct(action: () => unknown) {
  closeMenu(false);
  void action();
}
function onMenuKey(event: KeyboardEvent) {
  const items = [...(menu.value?.querySelectorAll<HTMLElement>('[role=menuitem]:not(:disabled)') ?? [])];
  const index = items.indexOf(document.activeElement as HTMLElement);
  if (event.key === 'Escape' || event.key === 'Tab') {
    event.preventDefault();
    closeMenu();
  } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    items[(index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
  } else if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault();
    items[event.key === 'Home' ? 0 : items.length - 1]?.focus();
  }
}
function onPointerDown(event: PointerEvent) {
  const target = event.target as Node;
  if (menuOpen.value && !menu.value?.contains(target) && !menuButton.value?.$el.contains(target)) closeMenu(false);
  // A press anywhere else in the shell also puts the picker and the six-tab question away (the page gets its area back).
  const inside = (selector: string) => target instanceof Element && !!target.closest(selector);
  if (store.pickerOpen && !inside('[data-testid=catch-picker], [data-testid=catch-quality]')) store.closePicker();
  if (store.limitAsk && !inside('[data-testid=catch-tab-limit], [data-testid=catch-tab-new]')) store.cancelLimit();
}

// Picker: opens under the quality button, which stays in the row with Save; focus returns to it on close.
const qualityButton = ref<{ $el: HTMLElement } | null>(null);
const content = ref<HTMLElement | null>(null);
const pickerAt = ref({ top: 52, start: 14 });
function placePicker() {
  const chip = qualityButton.value?.$el.getBoundingClientRect();
  const box = content.value?.getBoundingClientRect();
  if (!chip || !box) return;
  pickerAt.value = { top: Math.round(chip.bottom - box.top + 6), start: pickerStart(rtl.value ? box.right - chip.right : chip.left - box.left, box.width) };
}
function openPicker() {
  menuOpen.value = false;
  store.openPicker();
}
watch(() => store.pickerOpen, (open, was) => {
  if (!open && was) void nextTick(() => qualityButton.value?.$el.focus());
});

// Shell geometry: the page goes below the address and found rows, beside the tab list; overlays draw over it.
const header = ref<HTMLElement | null>(null);
const rows = ref<HTMLElement | null>(null);
const homeView = ref<{ focus: () => void } | null>(null);
const rowsHeight = ref(0);
// Coalesced on a tick, not an animation frame: a window behind another app paints no frames
// (WebKit pauses them), and the page would keep its first, wrong place under the side list.
let measuring = false;
function measure() {
  if (measuring) return;
  measuring = true;
  void nextTick(() => {
    measuring = false;
    if (!header.value) return;
    rowsHeight.value = Math.ceil(rows.value?.offsetHeight ?? 0);
    placePicker();
    const top = Math.ceil((header.value?.offsetHeight ?? 0) + rowsHeight.value);
    store.fitShell({ top, side: store.sideOpen ? CATCH_SIDE_OPEN_PX : CATCH_SIDE_FOLDED_PX, overlay: firstRunCard.value, rtl: rtl.value });
  });
}

// Keyboard: ⌘/Ctrl+T new tab, ⌘/Ctrl+W close tab, Ctrl+Tab next (Shift: previous), ⌘/Ctrl+L address.
function onKey(event: KeyboardEvent) {
  // Escape closes what is drawn over the page wherever the focus is (inside them, their own handler ran first).
  if (event.key === 'Escape' && !event.defaultPrevented && (store.pickerOpen || store.limitAsk || store.menuOpen || whyOpen.value)) {
    event.preventDefault();
    if (store.pickerOpen) store.closePicker();
    else if (store.limitAsk) store.cancelLimit();
    else if (store.menuOpen) closeMenu();
    else {
      whyOpen.value = false;
      whyButton.value?.focus();
    }
    return;
  }
  const mod = event.metaKey || event.ctrlKey;
  if (event.ctrlKey && event.key === 'Tab') {
    event.preventDefault();
    store.nextTab(event.shiftKey ? -1 : 1);
    return;
  }
  if (!mod || event.altKey) return;
  const key = event.key.toLowerCase();
  if (key === 't' && !event.shiftKey) {
    event.preventDefault();
    void store.newTab();
  } else if (key === 'w' && !event.shiftKey) {
    event.preventDefault();
    if (store.activeTabId !== null) void store.closeTab(store.activeTabId);
  } else if (key === 'l') {
    event.preventDefault();
    focusAddress();
  }
}
function focusAddress() {
  if (store.isHome && homeView.value) homeView.value.focus();
  else address.value?.focus();
}
// A new tab opened from a page, or the address key pressed in one: the engine asks, the field takes the keyboard.
watch(() => store.addressAsks, () => void nextTick(focusAddress));

let observer: ResizeObserver | undefined;
onMounted(() => {
  void store.loadPreferences();
  void store.start(props.windowId).catch(() => {});
  document.addEventListener('pointerdown', onPointerDown, true);
  document.addEventListener('keydown', onKey);
  window.addEventListener('resize', measure);
  observer = new ResizeObserver(measure);
  if (header.value) observer.observe(header.value);
  if (rows.value) observer.observe(rows.value);
  measure();
});
watch(() => [store.windowId, store.pickerOpen, store.menuOpen, store.engineMenu, store.limitAsk, store.sideOpen, firstRunCard.value, rtl.value], measure);
watch(state, measure);
onBeforeUnmount(() => {
  store.stop();
  observer?.disconnect();
  document.removeEventListener('pointerdown', onPointerDown, true);
  document.removeEventListener('keydown', onKey);
  window.removeEventListener('resize', measure);
});
</script>
