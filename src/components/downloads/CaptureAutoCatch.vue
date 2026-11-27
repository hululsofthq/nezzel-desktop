<template>
  <!-- Beside a usable embedded result: one quiet line under it, the found full video offered first. -->
  <p v-if="view.beside" class="flex min-w-0 flex-wrap items-center gap-x-1.5 text-[12px] leading-4 text-muted-foreground" :role="view.kind === 'looking' ? 'status' : undefined" data-testid="capture-auto-beside">
    <LoaderCircle v-if="view.kind === 'looking'" class="size-3 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden="true" />
    <span :class="view.kind === 'found' && 'font-medium text-foreground'">{{ view.kind === 'found' ? fullLabel : t(view.text) }}</span>
    <Button v-if="view.kind === 'found'" type="button" variant="link" class="h-auto px-0 text-[12px]" :disabled="busy" data-testid="capture-auto-download" @click="emit('act', 'download')">{{ t('downloads.row.download') }}</Button>
    <Button v-else type="button" variant="link" class="h-auto px-0 text-[12px] text-muted-foreground" :disabled="busy" data-testid="capture-auto-cancel" @click="emit('act', 'cancel')">{{ t('catch.auto.cancel') }}</Button>
  </p>
  <p v-else-if="view.kind === 'looking'" class="flex items-center gap-1.5 text-[12.5px] text-muted-foreground" role="status" data-testid="capture-auto-looking">
    <LoaderCircle class="size-3.5 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden="true" /><span>{{ t(view.text) }}</span>
  </p>
  <div v-else-if="view.kind === 'found'" class="flex min-w-0 items-start gap-2.5" data-testid="capture-auto-found">
    <img v-if="isCaughtThumbnail(state.thumbnail)" :src="state.thumbnail" alt="" class="h-12 w-16 shrink-0 rounded-md object-cover" />
    <div class="min-w-0 flex-1">
      <p class="line-clamp-2 text-start text-[13px] leading-[1.4] font-medium rtl:leading-[1.5]"><bdi dir="auto">{{ state.title || site }}</bdi></p>
      <p class="mt-0.5 flex min-w-0 items-center gap-1.5 text-[12px] text-muted-foreground">
        <span>{{ t(view.text) }}</span>
        <template v-if="duration"><span class="nz-sep" aria-hidden="true" /><bdi dir="ltr" class="tnum">{{ duration }}</bdi></template>
      </p>
    </div>
  </div>
  <!-- needsStep is the existing "This page needs one step" (CaptureFailure); live and encrypted are a plain note. -->
  <div v-else class="flex min-w-0 flex-col gap-0.5" role="status" data-testid="capture-auto-note">
    <p class="flex items-start gap-1.5 text-[12.5px] font-medium text-foreground/85"><Info class="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" /><span>{{ t(view.text) }}</span></p>
    <p v-if="view.sub" class="ps-5 text-[12px] text-muted-foreground text-pretty">{{ t(view.sub) }}</p>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { isCaughtThumbnail } from '../../helpers/remoteImages';
import { Info, LoaderCircle } from '@lucide/vue';
import { Button } from '@/components/ui/button';
import type { AutoCatchAction, AutoCatchView } from '../../helpers/catchOffer';
import type { CatchAutoState } from '../../tauri/types/catch';
import { formatDuration } from '../../helpers/units';

const props = defineProps<{ view: AutoCatchView; state: CatchAutoState; site: string; busy: boolean }>();
const emit = defineEmits<{ act: [action: AutoCatchAction] }>();
const { t } = useI18n();
const duration = computed(() => (props.state.durationSec ? formatDuration(props.state.durationSec) : ''));
const fullLabel = computed(() => (duration.value ? t('catch.auto.foundFull', { duration: duration.value }) : t('catch.auto.foundFullNoLength')));
</script>
