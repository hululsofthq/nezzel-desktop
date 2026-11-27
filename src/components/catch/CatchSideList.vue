<template>
  <!-- The tab list on the start side: open (240 px, rows and "This site" tools) or folded (48 px, one icon per tab). -->
  <nav
    class="relative flex shrink-0 flex-col border-e border-hairline bg-sidebar"
    :class="store.sideOpen ? 'w-60 px-2 pt-2' : 'w-12 items-center gap-1.5 pt-2 pb-2.5'"
    :aria-label="t('catch.shell.tabs')"
    data-testid="catch-side"
    :data-open="store.sideOpen"
  >
    <!-- New tab -->
    <button
      v-if="store.sideOpen"
      type="button"
      :class="[rowCls, 'mb-1 text-foreground']"
      :aria-label="newTabLabel"
      data-testid="catch-tab-new"
      @click="store.newTab()"
    >
      <Plus class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      <span class="min-w-0 flex-1 truncate text-start">{{ t('catch.shell.newTab') }}</span>
      <kbd class="text-[11px] text-muted-foreground/80" dir="ltr" aria-hidden="true">{{ newTabKeys }}</kbd>
    </button>
    <Tooltip v-else :content="newTabLabel" :side="tipSide">
      <button type="button" :class="railCls" :aria-label="newTabLabel" data-testid="catch-tab-new" @click="store.newTab()">
        <Plus class="size-4" aria-hidden="true" />
      </button>
    </Tooltip>

    <!-- Six tabs: ask which one to close instead of refusing. -->
    <div
      v-if="store.limitAsk"
      :class="store.sideOpen
        ? '-mx-2 flex min-h-0 flex-col border-b border-hairline px-4 pt-1.5 pb-2'
        : 'absolute top-2 start-[calc(100%+6px)] z-30 w-64 rounded-[10px] border border-hairline bg-popover p-2.5 text-popover-foreground shadow-lg animate-in fade-in-0 zoom-in-95 duration-150 motion-reduce:animate-none'"
      role="dialog"
      :aria-label="t('catch.shell.limitTitle', { n: number.count(CATCH_TAB_LIMIT) })"
      data-testid="catch-tab-limit"
      @keydown.esc.stop.prevent="store.cancelLimit()"
    >
      <p class="font-semibold">{{ t('catch.shell.limitTitle', { n: number.count(CATCH_TAB_LIMIT) }) }}</p>
      <p class="mb-1 text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.shell.limitBody') }}</p>
      <ul class="flex flex-col overflow-y-auto" :class="store.sideOpen ? '-mx-2 min-h-0' : 'max-h-40'">
        <li v-for="row in store.rows" :key="row.tab.id">
          <button type="button" :class="[rowCls, 'w-full', store.sideOpen ? 'h-7 min-h-7!' : 'h-8']" :aria-label="`${t('catch.shell.limitClose')}: ${tabTitle(row.tab)}`" @click="store.closeForNew(row.tab.id)">
            <CatchTabMark :row="row" />
            <span dir="auto" class="min-w-0 flex-1 truncate text-start [unicode-bidi:plaintext]">{{ tabTitle(row.tab) }}</span>
            <X class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          </button>
        </li>
      </ul>
      <div class="mt-1 flex shrink-0 justify-end">
        <Button ref="limitCancel" variant="ghost" size="xs" @click="store.cancelLimit()">{{ t('catch.shell.limitCancel') }}</Button>
      </div>
    </div>

    <template v-if="store.sideOpen">
      <!-- The question lists the same tabs: it stands in for the list instead of pushing it down. -->
      <p v-if="!store.limitAsk" class="px-2 pt-2.5 pb-1 text-[11px] font-semibold text-muted-foreground rtl:text-[12px]">{{ t('catch.shell.openTabs') }}</p>
      <ul v-if="!store.limitAsk" class="flex min-h-0 flex-col gap-1 overflow-y-auto" data-testid="catch-tabs">
        <li
          v-for="row in store.rows"
          :key="row.tab.id"
          class="group flex h-8 shrink-0 items-center rounded-[6px] pe-1 transition-colors duration-100 motion-reduce:transition-none"
          :class="[row.active ? 'bg-primary-soft font-medium text-primary-soft-foreground' : 'hover:bg-foreground/[0.05]', row.linked && 'ps-4']"
          :data-tab="row.tab.id"
          :data-active="row.active"
          data-testid="catch-tab"
        >
          <button
            type="button"
            class="flex h-full min-w-0 flex-1 items-center gap-2 rounded-[6px] ps-2 text-start outline-none focus-visible:ring-2 focus-visible:ring-primary/50 active:opacity-80"
            :aria-current="row.active ? 'page' : undefined"
            :aria-label="tabLabel(row)"
            @click="store.selectTab(row.tab.id)"
          >
            <CornerDownRight v-if="row.linked" class="-me-1 size-3.5 shrink-0 text-muted-foreground rtl:-scale-x-100" aria-hidden="true" />
            <CatchTabMark :row="row" />
            <span dir="auto" class="min-w-0 flex-1 truncate text-start [unicode-bidi:plaintext]" :class="row.tab.asleep && !row.active && 'text-muted-foreground'">{{ tabTitle(row.tab) }}</span>
            <span v-if="row.tab.loading" class="size-3 shrink-0 animate-spin rounded-full border-2 border-primary/25 border-t-primary motion-reduce:animate-none" aria-hidden="true" />
            <span v-else-if="row.tab.found" class="size-[7px] shrink-0 rounded-full bg-primary" aria-hidden="true" data-testid="catch-tab-dot" />
            <span v-if="row.tab.private" class="shrink-0 rounded-full border border-input px-1.5 text-[11px] leading-4 text-muted-foreground">{{ t('catch.shell.private') }}</span>
          </button>
          <Tooltip :content="t('catch.shell.closeTab')" :side="tipSide">
            <button
              type="button"
              class="ms-0.5 grid size-6 shrink-0 place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-foreground/10 hover:text-foreground focus-visible:ring-2 focus-visible:ring-primary/50 active:scale-95 motion-reduce:active:scale-100"
              :aria-label="`${t('catch.shell.closeTab')}: ${tabTitle(row.tab)}`"
              data-testid="catch-tab-close"
              @click="store.closeTab(row.tab.id)"
            >
              <X class="size-3.5" aria-hidden="true" />
            </button>
          </Tooltip>
        </li>
      </ul>
      <p v-if="store.rows.length === 1 && !store.isHome" class="px-2 py-2.5 text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.shell.singleHint') }}</p>

      <!-- Footer: the home page explains sign-in; a page gets "This site" tools. -->
      <div class="-mx-2 mt-auto flex flex-col gap-1 border-t border-hairline px-2 pt-2 pb-2.5 text-[12px] rtl:text-[13px]" data-testid="catch-site-tools">
        <template v-if="store.isHome">
          <p class="flex items-center gap-2 px-2 py-1 text-muted-foreground"><KeyRound class="size-3.5 shrink-0" aria-hidden="true" />{{ t('catch.shell.shared') }}</p>
          <button type="button" :class="rowCls" data-testid="catch-tab-private" @click="store.newTab({ private: true })">
            <VenetianMask class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" /><span class="min-w-0 flex-1 truncate text-start">{{ t('catch.shell.newPrivateTab') }}</span>
          </button>
        </template>
        <template v-else>
          <p class="flex min-w-0 items-center gap-1.5 px-2 pb-0.5 text-muted-foreground">
            <span class="shrink-0">{{ t('catch.tools.thisSite') }}</span><span class="nz-sep" aria-hidden="true" /><bdi dir="ltr" class="truncate">{{ store.host }}</bdi>
          </p>
          <label :class="rowCls" class="cursor-default">
            <KeyRound class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            <span class="min-w-0 flex-1 text-start leading-snug text-balance">{{ t('catch.tools.signin') }}</span>
            <LoaderCircle v-if="store.signin?.status === 'busy'" class="size-3.5 animate-spin text-muted-foreground motion-reduce:animate-none" :aria-label="t('catch.tools.signinBusy')" />
            <Switch v-else :model-value="store.signinOn" :disabled="store.signinLocked" data-testid="catch-signin-switch" @update:model-value="store.toggleSignin()" />
          </label>
          <label :class="rowCls" class="cursor-default" :title="t('catch.tools.adblockHint')">
            <ShieldBan class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            <span class="min-w-0 flex-1 text-start leading-snug text-balance">{{ t('catch.tools.adblock') }}</span>
            <span v-if="ads > 0" class="tnum shrink-0 text-muted-foreground" dir="ltr" :aria-label="adsLabel" data-testid="catch-ads-count">{{ adsBadge }}</span>
            <Switch :model-value="store.adblockOn" :disabled="!!store.activeTab?.private" data-testid="catch-adblock-switch" @update:model-value="store.toggleAdblock()" />
          </label>
          <div v-if="popups > 0" class="flex min-h-8 items-center gap-2 px-2" data-testid="catch-popups">
            <Ban class="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
            <span class="min-w-0 flex-1 leading-snug">{{ t('catch.popups', { n: number.count(popups) }, popups) }}</span>
            <button type="button" class="shrink-0 rounded-[4px] font-medium whitespace-nowrap text-primary outline-none hover:underline focus-visible:ring-2 focus-visible:ring-primary/50" data-testid="catch-allow-popup" @click="store.allowPopupOnce()">{{ t('catch.allowOnce') }}</button>
          </div>
        </template>
      </div>
    </template>

    <template v-else>
      <span class="h-px w-5 shrink-0 bg-input" aria-hidden="true" />
      <ul class="flex min-h-0 flex-col items-center gap-1.5 overflow-y-auto" data-testid="catch-tabs">
        <li v-for="row in store.rows" :key="row.tab.id" :data-tab="row.tab.id" :data-active="row.active" data-testid="catch-tab">
          <Tooltip :content="tabTitle(row.tab)" :side="tipSide">
            <button
              type="button"
              :class="[railCls, row.active && 'bg-primary-soft! text-primary-soft-foreground']"
              :aria-current="row.active ? 'page' : undefined"
              :aria-label="tabLabel(row)"
              @click="store.selectTab(row.tab.id)"
            >
              <CatchTabMark :row="row" large />
              <span v-if="row.tab.found" class="absolute top-0.5 end-0.5 size-[7px] rounded-full bg-primary shadow-[0_0_0_2px_var(--color-sidebar)]" aria-hidden="true" data-testid="catch-tab-dot" />
            </button>
          </Tooltip>
        </li>
      </ul>
      <span class="flex-1" />
      <Tooltip v-if="store.isHome" :content="t('catch.shell.newPrivateTab')" :side="tipSide">
        <button type="button" :class="railCls" :aria-label="t('catch.shell.newPrivateTab')" data-testid="catch-tab-private" @click="store.newTab({ private: true })">
          <VenetianMask class="size-4" aria-hidden="true" />
        </button>
      </Tooltip>
      <template v-else>
        <Tooltip :content="signinTip" :side="tipSide">
          <button
            type="button"
            :class="[railCls, store.signinOn && 'text-primary!']"
            :aria-pressed="store.signinOn"
            :aria-label="t('catch.tools.signin')"
            :disabled="store.signinLocked"
            data-testid="catch-signin-switch"
            @click="store.toggleSignin()"
          >
            <LoaderCircle v-if="store.signin?.status === 'busy'" class="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
            <KeyRound v-else class="size-4" aria-hidden="true" />
          </button>
        </Tooltip>
        <Tooltip :content="store.adblockOn ? `${t('catch.tools.adblock')} · ${adsLabel}` : t('catch.tools.adblock')" :side="tipSide">
          <button
            type="button"
            :class="[railCls, store.adblockOn && 'text-primary!']"
            :aria-pressed="store.adblockOn"
            :aria-label="t('catch.tools.adblock')"
            :disabled="!!store.activeTab?.private"
            data-testid="catch-adblock-switch"
            @click="store.toggleAdblock()"
          >
            <ShieldBan class="size-4" aria-hidden="true" />
            <span v-if="ads > 0" class="tnum absolute -bottom-0.5 -end-0.5 min-w-[13px] rounded-full bg-foreground px-[3px] text-center text-[9.5px] leading-[13px] font-semibold text-background" dir="ltr" aria-hidden="true" data-testid="catch-ads-count">{{ adsBadge }}</span>
          </button>
        </Tooltip>
        <Tooltip v-if="popups > 0" :content="`${t('catch.popups', { n: number.count(popups) }, popups)} · ${t('catch.allowOnce')}`" :side="tipSide">
          <button
            type="button"
            :class="railCls"
            :aria-label="`${t('catch.popups', { n: number.count(popups) }, popups)}. ${t('catch.allowOnce')}`"
            data-testid="catch-allow-popup"
            @click="store.allowPopupOnce()"
          >
            <Ban class="size-4" aria-hidden="true" />
            <span class="tnum absolute -bottom-0.5 -end-0.5 min-w-[13px] rounded-full bg-foreground px-[3px] text-center text-[9.5px] leading-[13px] font-semibold text-background" dir="ltr" aria-hidden="true">{{ popupBadge }}</span>
          </button>
        </Tooltip>
      </template>
    </template>
  </nav>
</template>

<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { Ban, CornerDownRight, KeyRound, LoaderCircle, Plus, ShieldBan, VenetianMask, X } from '@lucide/vue';
import { Button } from '@/components/ui/button';
import { Switch } from '@/components/ui/switch';
import Tooltip from '@/components/ui/tooltip/Tooltip.vue';
import CatchTabMark from './CatchTabMark.vue';
import { CATCH_TAB_LIMIT, useCatchStore, type CatchTabRow } from '../../stores/catch';
import { useCatchText } from './useCatchText';
import { useLocaleNumber } from '../../composables/useLocaleNumber';
import { localeDirection } from '../../i18n';

const { t, locale } = useI18n();
const store = useCatchStore();
const number = useLocaleNumber();
const { tabTitle } = useCatchText();
const rtl = computed(() => localeDirection(locale.value) === 'rtl');
/** Tooltips open toward the page: the end side. */
const tipSide = computed(() => (rtl.value ? 'left' : 'right'));
const newTabKeys = `${/Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl+'}T`;
const popups = computed(() => store.shown?.popupsBlocked ?? 0);
const popupBadge = computed(() => (popups.value > 99 ? '99+' : number.count(popups.value)));
const ads = computed(() => store.adsBlocked);
const adsBadge = computed(() => (ads.value > 99 ? '99+' : number.count(ads.value)));
const adsLabel = computed(() => t('catch.tools.adblockCount', { n: number.count(ads.value) }, ads.value));
const newTabLabel = computed(() => store.atLimit ? `${t('catch.shell.newTab')}. ${t('catch.shell.limitTitle', { n: number.count(CATCH_TAB_LIMIT) })}` : t('catch.shell.newTab'));
const signinTip = computed(() => {
  if (store.signinDone) return store.signin?.browser ? t('catch.tools.signinDone', { browser: store.signin.browser }) : t('catch.tools.signinDoneAny');
  return t('catch.tools.signin');
});

const rowCls = 'flex min-h-8 w-full items-center gap-2 rounded-[6px] px-2 outline-none transition-colors duration-100 hover:bg-foreground/[0.05] focus-visible:ring-2 focus-visible:ring-primary/50 active:bg-foreground/[0.08] motion-reduce:transition-none';
const railCls = 'relative grid size-8 shrink-0 place-items-center rounded-[6px] text-muted-foreground outline-none transition-colors duration-100 hover:bg-foreground/[0.06] hover:text-foreground focus-visible:ring-2 focus-visible:ring-primary/50 active:bg-foreground/[0.1] motion-reduce:transition-none';

function tabLabel(row: CatchTabRow) {
  const parts = [tabTitle(row.tab)];
  if (row.tab.private) parts.push(t('catch.shell.private'));
  if (row.linked) parts.push(t('catch.shell.linked'));
  if (row.tab.found) parts.push(t('catch.shell.found'));
  if (row.tab.loading) parts.push(t('catch.shell.loading'));
  if (row.tab.asleep) parts.push(t('catch.shell.asleep'));
  return parts.join(', ');
}

// The limit question takes focus so the keyboard can answer it; put away, focus goes back to "New tab"
// (unless something else took it, like the new tab's home field).
const limitCancel = ref<{ $el: HTMLElement } | null>(null);
watch(() => !!store.limitAsk, (open) => {
  void nextTick(() => {
    if (open) limitCancel.value?.$el.closest('[role=dialog]')?.querySelector<HTMLElement>('button')?.focus();
    else if (!document.activeElement || document.activeElement === document.body) document.querySelector<HTMLElement>('[data-testid=catch-tab-new]')?.focus();
  });
});
</script>
