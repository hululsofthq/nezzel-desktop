<template>
  <!-- Opens under the bar's quality button; the bar keeps that button and its Save in view. -->
  <div
    ref="sheet"
    role="dialog"
    :aria-label="t('catch.picker.title')"
    class="w-[420px] max-w-[calc(100%-16px)] overflow-hidden rounded-[10px] border border-hairline bg-popover text-popover-foreground shadow-lg animate-in fade-in-0 zoom-in-95 duration-150 motion-reduce:animate-none"
    data-testid="catch-picker"
    @keydown.esc.stop.prevent="store.closePicker()"
  >
    <h2 class="px-3.5 pt-3 pb-2.5 text-[13.5px] font-semibold rtl:text-[14.5px]">{{ t('catch.picker.title') }}</h2>
    <div v-if="store.candidate" class="flex gap-3 border-b border-hairline px-3.5 pb-3">
      <div class="relative grid h-[54px] w-24 shrink-0 place-items-center overflow-hidden rounded-md bg-muted text-muted-foreground">
        <img v-if="thumbnail" :src="thumbnail" alt="" class="absolute inset-0 size-full object-cover" data-testid="catch-picker-thumb" @error="thumbFailed = thumbnail">
        <Film v-else class="size-5" aria-hidden="true" />
        <span v-if="store.candidate.durationSec" class="tnum absolute bottom-1 end-1 rounded-[3px] bg-black/70 px-1 text-[10.5px] leading-4 text-white" dir="ltr">{{ formatDuration(store.candidate.durationSec) }}</span>
      </div>
      <div class="flex min-w-0 flex-1 flex-col justify-center gap-0.5">
        <p dir="auto" class="line-clamp-2 text-start font-medium leading-snug [unicode-bidi:plaintext]">{{ candidateTitle(store.candidate) }}</p>
        <p class="flex min-w-0 items-center gap-1.5 text-[12px] text-muted-foreground rtl:text-[13px]">
          <template v-for="(part, i) in facts" :key="i">
            <span v-if="i > 0" class="nz-sep" aria-hidden="true" />
            <i18n-t v-if="part.size" keypath="sheet.size" tag="bdi" scope="global" class="tnum shrink-0"><template #size><bdi dir="ltr">{{ part.size }}</bdi></template></i18n-t>
            <bdi v-else :dir="part.ltr ? 'ltr' : undefined" class="tnum truncate">{{ part.text }}</bdi>
          </template>
        </p>
      </div>
    </div>

    <div role="radiogroup" :aria-label="t('catch.quality')" class="py-1">
      <p v-if="store.formatsLoading" class="flex h-9 items-center gap-2 px-3.5 text-muted-foreground" role="status">
        <LoaderCircle class="size-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" />{{ t('catch.picker.loading') }}
      </p>
      <button
        v-else-if="!store.candidateFormats.length"
        type="button"
        role="radio"
        aria-checked="true"
        :class="rowCls(true)"
      >
        <span :class="radioCls(true)" aria-hidden="true" />
        <span class="font-medium">{{ t('catch.picker.bestAvailable') }}</span>
      </button>
      <button
        v-for="format in store.candidateFormats"
        :key="format.id"
        type="button"
        role="radio"
        :aria-checked="store.format?.id === format.id"
        :tabindex="store.format?.id === format.id ? 0 : -1"
        :class="rowCls(store.format?.id === format.id)"
        :data-format="format.id"
        @click="store.pickFormat(format.id)"
        @keydown="onRadioKey"
      >
        <span :class="radioCls(store.format?.id === format.id)" aria-hidden="true" />
        <span class="min-w-[92px] text-start font-medium">{{ formatLabel(format) }}</span>
        <span class="w-11 shrink-0 text-start text-muted-foreground uppercase" dir="ltr">{{ format.ext }}</span>
        <span v-if="format.best" class="rounded-full bg-primary-soft px-1.5 text-[11px] leading-[18px] font-semibold text-primary-soft-foreground rtl:text-[12px]">{{ t('catch.picker.best') }}</span>
        <i18n-t v-if="format.sizeBytes" keypath="sheet.size" tag="span" scope="global" class="tnum ms-auto shrink-0 text-muted-foreground"><template #size><bdi dir="ltr">{{ number.size(format.sizeBytes) }}</bdi></template></i18n-t>
      </button>
    </div>

    <template v-if="others.length">
      <button
        type="button"
        class="flex h-10 w-full items-center gap-2 border-t border-hairline px-3.5 text-start hover:bg-foreground/[0.04]"
        :aria-expanded="othersOpen"
        data-testid="catch-others-toggle"
        @click="othersOpen = !othersOpen"
      >
        <ChevronRight class="size-4 shrink-0 text-muted-foreground transition-transform duration-150 rtl:-scale-x-100 motion-reduce:transition-none" :class="othersOpen && 'rotate-90 rtl:-rotate-90'" aria-hidden="true" />
        <span class="truncate">{{ t('catch.picker.others') }}</span>
        <span class="tnum text-muted-foreground">{{ `(${number.count(others.length)})` }}</span>
        <span class="ms-auto shrink-0 text-[12px] text-muted-foreground rtl:text-[13px]">{{ t('catch.picker.othersHint') }}</span>
      </button>
      <div v-if="othersOpen" role="radiogroup" :aria-label="t('catch.picker.others')" class="max-h-40 overflow-y-auto pb-1" data-testid="catch-others">
        <button
          v-for="other in others"
          :key="other.id"
          type="button"
          role="radio"
          :aria-checked="store.candidate?.id === other.id"
          :class="rowCls(store.candidate?.id === other.id)"
          :data-candidate="other.id"
          @click="store.pickCandidate(other.id)"
        >
          <span :class="radioCls(store.candidate?.id === other.id)" aria-hidden="true" />
          <bdi dir="auto" class="min-w-0 flex-1 truncate text-start [unicode-bidi:plaintext]">{{ candidateTitle(other) }}</bdi>
          <span v-if="other.durationSec" class="tnum shrink-0 text-muted-foreground" dir="ltr">{{ formatDuration(other.durationSec) }}</span>
        </button>
      </div>
    </template>

    <p v-if="store.saveFailed" class="border-t border-hairline px-3.5 py-2 text-[12.5px] text-destructive-foreground dark:text-destructive" role="alert">{{ t('catch.saveFailed') }}</p>
    <div class="flex justify-end gap-2 border-t border-hairline px-3.5 py-2.5">
      <Button variant="ghost" size="sm" @click="store.closePicker()">{{ t('catch.picker.cancel') }}</Button>
      <Button size="sm" :disabled="!store.canSave" data-testid="catch-picker-save" @click="store.save()">
        <LoaderCircle v-if="store.busy" class="animate-spin motion-reduce:animate-none" aria-hidden="true" /><Download v-else aria-hidden="true" />{{ t(store.candidate?.alreadySaved ? 'downloads.row.alreadySaved' : 'catch.save') }}
      </Button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { isCaughtThumbnail } from '../../helpers/remoteImages';
import { ChevronRight, Download, Film, LoaderCircle } from '@lucide/vue';
import { Button } from '@/components/ui/button';
import { useCatchStore } from '../../stores/catch';
import { useLocaleNumber } from '../../composables/useLocaleNumber';
import { formatDuration } from '../../helpers/units';
import { useCatchText } from './useCatchText';

const { t } = useI18n();
const store = useCatchStore();
const number = useLocaleNumber();
const { candidateTitle, formatLabel } = useCatchText();
const sheet = ref<HTMLElement | null>(null);
const othersOpen = ref(false);
/** Rust sends only a small `data:` picture it fetched itself; the shell never loads a remote one. */
const thumbFailed = ref<string | null>(null);
const thumbnail = computed(() => {
  const src = store.candidate?.thumbnail;
  return isCaughtThumbnail(src) && src !== thumbFailed.value ? src : null;
});

const others = computed(() => {
  const main = store.candidates.main;
  // The picked "other" video takes the summary; the main one moves into the list so it can be picked back.
  return [...(main && store.candidate?.id !== main.id ? [main] : []), ...store.candidates.others.filter(c => c.id !== store.candidate?.id)];
});

const facts = computed(() => {
  const c = store.candidate;
  if (!c) return [];
  const parts: { text: string; ltr?: boolean; size?: string }[] = [];
  if (c.durationSec) parts.push({ text: formatDuration(c.durationSec), ltr: true });
  const size = store.format?.sizeBytes ?? c.sizeBytes;
  if (size) parts.push({ text: '', size: number.size(size) });
  if (store.state?.host) parts.push({ text: store.state.host, ltr: true });
  return parts;
});

const rowCls = (on: boolean) => ['flex h-9 w-full items-center gap-2.5 px-3.5 outline-none hover:bg-foreground/[0.04] focus-visible:bg-accent', on && 'bg-accent'];
const radioCls = (on: boolean) => ['size-4 shrink-0 rounded-full border bg-card', on ? 'border-[5px] border-primary' : 'border-input'];

/** Radio group keys: arrows move and pick (inline direction follows the page). */
function onRadioKey(event: KeyboardEvent) {
  const keys = ['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'];
  if (!keys.includes(event.key)) return;
  event.preventDefault();
  const rows = [...(event.currentTarget as HTMLElement).parentElement!.querySelectorAll<HTMLElement>('[role=radio][data-format]')];
  const rtl = document.documentElement.dir === 'rtl';
  const forward = event.key === 'ArrowDown' || event.key === (rtl ? 'ArrowLeft' : 'ArrowRight');
  const index = rows.indexOf(event.currentTarget as HTMLElement);
  const next = rows[(index + (forward ? 1 : -1) + rows.length) % rows.length];
  next.click();
  next.focus();
}

onMounted(() => {
  void nextTick(() => sheet.value?.querySelector<HTMLElement>('[role=radio][aria-checked=true], [data-testid=catch-picker-save]')?.focus());
});
</script>
