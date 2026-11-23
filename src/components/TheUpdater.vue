<template>
  <div class="fixed bottom-30 end-0 z-40 pointer-events-none">
    <Transition
        appear
        enter-active-class="transition duration-250 ease-out"
        enter-from-class="translate-y-4 opacity-0"
        enter-to-class="translate-y-0 opacity-100"
        leave-active-class="transition duration-200 ease-in"
        leave-from-class="translate-y-0 opacity-100"
        leave-to-class="translate-y-4 opacity-0"
    >
      <div
          v-if="showCard"
          role="alert"
          data-testid="updater-alert"
          class="m-4 flex w-80 max-w-[calc(100vw-2rem)] items-start gap-3 rounded-xl border border-hairline bg-card p-3 text-foreground shadow-lg pointer-events-auto will-change-transform"
      >
        <span class="mt-0.5 grid size-7 shrink-0 place-items-center rounded-full bg-primary-soft text-primary-soft-foreground" aria-hidden="true">
          <Check v-if="isNeedingRestart && !isUpdating && !lastError" class="size-4" />
          <RefreshCw v-else class="size-4" :class="{ 'animate-spin motion-reduce:animate-none': isUpdating }" />
        </span>
        <div class="min-w-0 flex-1">
          <template v-if="needsPassword">
            <h3 class="text-[13px] font-medium leading-5 text-destructive-foreground">{{ t('feedback.updateNeedsPassword') }}</h3>
            <div class="mt-2 flex justify-end gap-2">
              <Button variant="ghost" @click="updaterStore.ignore()">{{ t('common.later') }}</Button>
              <Button variant="ghost" as="a" :href="pageUrl" target="_blank" rel="noopener" data-testid="updater-open-page">
                {{ t('updater.openPage') }}<ExternalLink aria-hidden="true" />
              </Button>
              <Button @click="updaterStore.retry()">{{ t('common.retry') }}</Button>
            </div>
          </template>
          <template v-else-if="stalled">
            <h3 class="text-[13px] font-medium leading-5 text-destructive-foreground">{{ t('feedback.updateDownloadStalled') }}</h3>
            <div class="mt-2 flex justify-end gap-2">
              <Button variant="ghost" @click="updaterStore.ignore()">{{ t('common.later') }}</Button>
              <Button @click="updaterStore.retry()">{{ t('common.retry') }}</Button>
            </div>
          </template>
          <template v-else-if="lastError">
            <h3 class="text-[13px] font-medium leading-5 text-destructive-foreground">{{ t(lastOperation === 'install' ? 'feedback.updateInstallFailed' : 'feedback.updateDownloadFailed') }}</h3>
            <div class="mt-2 flex justify-end gap-2">
              <Button variant="ghost" @click="updaterStore.ignore()">{{ t('common.later') }}</Button>
              <Button @click="updaterStore.retry()">{{ t('common.retry') }}</Button>
            </div>
          </template>
          <template v-else-if="info?.manual">
            <h3 :class="titleCls">{{ availableTitle }}</h3>
            <p :class="subCls">{{ t('updater.manualSub') }}</p>
            <div class="mt-2.5 flex items-center justify-end gap-2">
              <Button variant="ghost" @click="updaterStore.ignore()">{{ t('common.later') }}</Button>
              <Button as="a" :href="pageUrl" target="_blank" rel="noopener" data-testid="updater-open-page">
                {{ t('updater.openPage') }}<ExternalLink aria-hidden="true" />
              </Button>
            </div>
          </template>
          <template v-else-if="!isUpdating && !isNeedingRestart">
            <h3 :class="titleCls">{{ availableTitle }}</h3>
            <p v-if="info?.currentVersion && info.version" :class="subCls">{{ t('updater.youHave', { version: isolate(info.currentVersion) }) }}</p>
            <ul v-if="lines.length" class="mt-1 flex flex-col gap-0.5" data-testid="updater-notes">
              <li v-for="line in lines" :key="line" class="flex gap-2 text-[12.5px] leading-normal text-pretty before:mt-[0.6lh] before:size-1 before:shrink-0 before:rounded-full before:bg-muted-foreground/70 rtl:text-[13px]">
                <span class="min-w-0">{{ line }}</span>
              </li>
            </ul>
            <div class="mt-2.5 flex items-center justify-end gap-2">
              <button
                v-if="info?.version"
                type="button"
                class="me-auto rounded-[4px] text-[12px] text-muted-foreground underline decoration-input underline-offset-3 outline-none transition-colors duration-100 hover:text-foreground focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--nz-focus)] rtl:text-[12.5px]"
                data-testid="updater-skip"
                @click="updaterStore.skip()"
              >{{ t('updater.skip') }}</button>
              <Button variant="ghost" @click="updaterStore.ignore()">{{ t('common.later') }}</Button>
              <Button @click="updaterStore.download()">{{ t('common.download') }}</Button>
            </div>
          </template>
          <template v-else-if="isNeedingRestart && !isUpdating">
            <h3 :class="titleCls">{{ info?.version ? t('updater.ready', { version: isolate(info.version) }) : t('updater.install.title') }}</h3>
            <p :class="subCls" data-testid="updater-ready-sub">{{ readySub }}</p>
            <div class="mt-2.5 flex justify-end gap-2">
              <Button variant="ghost" @click="updaterStore.ignore()">{{ t('common.later') }}</Button>
              <Button @click="updaterStore.install()">{{ info?.version ? t('updater.installRestart') : t('common.install') }}</Button>
            </div>
          </template>
          <template v-else>
            <h3 :class="titleCls">{{ downloadingTitle }}</h3>
            <div v-if="lastOperation === 'download'" class="mt-1.5 flex items-center gap-2">
              <div
                  class="flex-1"
                  role="progressbar"
                  aria-valuemin="0"
                  aria-valuemax="100"
                  :aria-valuenow="progress == null ? undefined : Math.round(progress)"
                  :aria-label="downloadingTitle"
              >
                <Progress :value="progress" />
              </div>
              <p v-if="progress != null" class="shrink-0 text-[12px] text-muted-foreground">{{ t('common.percentage', { percent: progress.toFixed() }) }}</p>
            </div>
            <div v-if="lastOperation === 'download'" class="mt-2 flex justify-end">
              <Button variant="ghost" data-testid="updater-cancel" @click="updaterStore.cancel()">{{ t('common.cancel') }}</Button>
            </div>
          </template>
        </div>
      </div>
    </Transition>
  </div>
</template>

<script setup lang="ts">
import { Check, ExternalLink, RefreshCw } from '@lucide/vue';
import { Button } from '@/components/ui/button';
import { Progress } from '@/components/ui/progress';
import { useUpdaterStore } from '../stores/updater';
import { computed } from 'vue';
import { storeToRefs } from 'pinia';
import { useI18n } from 'vue-i18n';
import { usePlatform } from '../composables/usePlatform';
import { downloadPageUrl, summaryLines } from '../helpers/updateCheck';
import { useMediaProgressStore } from '../stores/media/progress';

const { t, locale } = useI18n();
const updaterStore = useUpdaterStore();
const { checkResult: info, showCard, downloadProgress, isUpdating, isNeedingRestart, lastError, lastOperation, needsPassword, stalled } = storeToRefs(updaterStore);
const { platform } = usePlatform();

const titleCls = 'text-[13px] font-medium leading-6 rtl:text-[14px]';
const subCls = 'text-[12px] text-muted-foreground rtl:text-[12.5px]';
/** Version numbers stay LTR inside Arabic sentences. */
const isolate = (version: string) => `\u2066${version}\u2069`;
const availableTitle = computed(() => (info.value?.version ? t('updater.availableVersion', { version: isolate(info.value.version) }) : t('updater.available')));
const progressStore = useMediaProgressStore();
/** Installing restarts the app, which stops running downloads: say so before the click. */
const readySub = computed(() => {
  if (!info.value?.version) return t('updater.install.subtitle');
  const running = progressStore.findAllCounts().downloading;
  return running > 0 ? t('updater.readySubRunning', { n: running }, running) : t('updater.readySub');
});
const downloadingTitle = computed(() => {
  if (lastOperation.value === 'install') return info.value?.systemPackage ? t('updater.passwordHint') : t('common.pleaseWait');
  return info.value?.version ? t('updater.downloadingVersion', { version: isolate(info.value.version) }) : t('updater.downloading');
});
const lines = computed(() => summaryLines(info.value?.notes, locale.value));
const pageUrl = computed(() => downloadPageUrl(platform.value, locale.value));

/** 0–100, or null (indeterminate) until the server says how big the update is. */
const progress = computed<number | null>(() => {
  const received = downloadProgress.value.received ?? 0;
  const total = downloadProgress.value.total ?? 0;
  if (total <= 0) return null;
  return Math.min(100, (received / total) * 100);
});
</script>
