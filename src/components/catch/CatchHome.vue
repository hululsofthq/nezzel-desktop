<template>
  <!-- Home page of a tab (`url: ''`): one field (search or address) with Paste, recent sites, recently saved. -->
  <div class="flex min-h-0 flex-1 flex-col items-center overflow-y-auto bg-background px-6 pt-11 pb-8 min-[1200px]:pt-20" data-testid="catch-home">
    <div class="w-full max-w-[560px] min-[1200px]:max-w-[620px]">
      <!-- The same mark as the main window's empty screen: the page reads as Nezzel's own, not a blank browser. -->
      <img :src="nezzelMark" alt="" class="mx-auto mb-4 size-14" data-testid="catch-home-mark" />
      <h1 class="mb-1 text-center text-[22px] font-semibold text-balance rtl:text-[24px]">{{ t('catch.home.title') }}</h1>
      <p class="mb-4.5 text-center text-muted-foreground text-pretty">{{ t('catch.home.sub') }}</p>

      <form
        class="flex h-[46px] items-center gap-2 rounded-[7px] border border-input bg-card ps-1.5 pe-1.5 shadow-xs focus-within:ring-2 focus-within:ring-primary/40"
        role="search"
        @submit.prevent="submit"
      >
        <CatchEngineMenu place="home" @picked="focusField" />
        <label for="catch-home-field" class="sr-only">{{ t('catch.home.field') }}</label>
        <input
          id="catch-home-field"
          ref="field"
          v-model="text"
          type="text"
          :dir="text ? 'auto' : undefined"
          autocomplete="off"
          spellcheck="false"
          enterkeyhint="go"
          :placeholder="t('catch.home.placeholder', { engine: store.engineName })"
          class="h-full min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-muted-foreground rtl:text-[16px]"
          data-testid="catch-home-field"
        />
        <Button v-if="text.trim()" type="submit" class="h-8 px-3.5" data-testid="catch-home-go">{{ t('catch.home.go') }}</Button>
        <Button v-else type="button" class="h-8 px-3.5" data-testid="catch-home-paste" @click="paste"><ClipboardPaste aria-hidden="true" />{{ t('catch.home.paste') }}</Button>
      </form>

      <h2 class="mt-6 mb-2 font-semibold">{{ t('catch.home.recentSites') }}</h2>
      <ul v-if="sites.length" class="grid grid-cols-[repeat(auto-fit,minmax(76px,1fr))] gap-2" data-testid="catch-home-sites">
        <li v-for="site in sites" :key="site.host">
          <button
            type="button"
            class="flex w-full flex-col items-center gap-1.5 rounded-[8px] bg-muted px-1 pt-2.5 pb-2 text-[12px] outline-none transition-colors duration-100 hover:bg-accent focus-visible:ring-2 focus-visible:ring-primary/50 active:scale-[0.97] motion-reduce:transition-none motion-reduce:active:scale-100 rtl:text-[13px]"
            :title="site.host"
            @click="store.go(site.host)"
          >
            <span class="grid size-[26px] place-items-center rounded-[7px] bg-card text-[13px] font-semibold" aria-hidden="true">{{ tabLetter(site) }}</span>
            <span dir="auto" class="w-full truncate text-start [unicode-bidi:plaintext]">{{ site.title?.trim() || site.host }}</span>
          </button>
        </li>
      </ul>
      <p v-else class="rounded-[8px] bg-muted px-3 py-2.5 text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.home.noSites') }}</p>

      <h2 class="mt-6 mb-1 font-semibold">{{ t('catch.home.recentSaved') }}</h2>
      <ul v-if="saved.length" data-testid="catch-home-saved">
        <li v-for="(video, i) in saved" :key="video.downloadId" class="flex items-center gap-3 border-b border-hairline py-2.5">
          <span class="grid h-10 w-[72px] shrink-0 place-items-center rounded-[5px] text-muted-foreground" :class="THUMBS[i % THUMBS.length]" aria-hidden="true"><Film class="size-4" /></span>
          <div class="min-w-0 flex-1">
            <p dir="auto" class="truncate text-start font-medium [unicode-bidi:plaintext]">{{ video.title }}</p>
            <p class="flex min-w-0 items-center gap-1.5 text-[12px] text-muted-foreground rtl:text-[13px]">
              <template v-for="(part, j) in facts(video)" :key="j">
                <span v-if="j > 0" class="nz-sep" aria-hidden="true" />
                <bdi dir="ltr" class="tnum truncate">{{ part }}</bdi>
              </template>
            </p>
          </div>
          <Tooltip :content="t('catch.home.show')">
            <Button variant="ghost" size="icon-xs" class="text-muted-foreground" :aria-label="t('catch.home.show')" @click="store.showSaved(video.downloadId)">
              <FolderOpen aria-hidden="true" />
            </Button>
          </Tooltip>
        </li>
      </ul>
      <p v-else class="rounded-[8px] bg-muted px-3 py-2.5 text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.home.noSaved') }}</p>
    </div>
  </div>
</template>

<script setup lang="ts">
import nezzelMark from '../../assets/brand/nezzel-mark.svg';
import { computed, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { ClipboardPaste, Film, FolderOpen } from '@lucide/vue';
import { Button } from '@/components/ui/button';
import Tooltip from '@/components/ui/tooltip/Tooltip.vue';
import CatchEngineMenu from './CatchEngineMenu.vue';
import { tabLetter, useCatchStore } from '../../stores/catch';
import { useLocaleNumber } from '../../composables/useLocaleNumber';
import { formatDuration } from '../../helpers/units';
import { readClipboardText } from '../../tauri/clipboard';
import type { CatchHome } from '../../tauri/types/catch';

const { t } = useI18n();
const store = useCatchStore();
const number = useLocaleNumber();
const text = ref('');
const field = ref<HTMLInputElement | null>(null);
const sites = computed(() => store.home?.recentSites.slice(0, 6) ?? []);
const saved = computed(() => store.home?.saved.slice(0, 3) ?? []);
const THUMBS = ['bg-[color-mix(in_srgb,#B98A3C_22%,var(--color-muted))]', 'bg-[color-mix(in_srgb,#3C6FB9_20%,var(--color-muted))]', 'bg-[color-mix(in_srgb,#5E8F4A_22%,var(--color-muted))]'];

function facts(video: CatchHome['saved'][number]) {
  return [video.host, video.height ? `${video.height}p` : '', video.sizeBytes ? number.size(video.sizeBytes) : '', video.durationSec ? formatDuration(video.durationSec) : ''].filter(Boolean);
}

async function submit() {
  if (await store.go(text.value)) text.value = '';
}

/** Paste opens what the clipboard holds at once (an address, or words to search). */
async function paste() {
  const clip = (await readClipboardText()) ?? (await navigator.clipboard?.readText().catch(() => '')) ?? '';
  if (!clip.trim()) {
    field.value?.focus();
    return;
  }
  text.value = clip.trim();
  await submit();
}

onMounted(() => {
  void store.loadHome();
  void store.loadPreferences();
  field.value?.focus();
});

/** The engine was picked: typing goes on in the field, over what it holds. */
function focusField() {
  field.value?.focus();
  field.value?.select();
}

defineExpose({ focus: () => field.value?.focus() });
</script>
