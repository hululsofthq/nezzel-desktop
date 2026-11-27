<template>
  <!-- An unsupported web page whose video plays in a browser: one step (amber), not a failure. A login wall or an
       unknown failure keeps its own words: there the browser is only one more action. -->
  <div v-if="catchable && reason === 'unsupported'" class="flex min-w-0 flex-col gap-0.5" role="status" data-testid="capture-catch">
    <p class="flex items-start gap-1.5 text-[12.5px] font-medium text-amber-foreground">
      <AppWindow class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" /><span>{{ t('catch.read.title') }}</span></p>
    <p class="ps-5 text-[12px] text-muted-foreground text-pretty">{{ t('catch.read.hint') }}</p>
  </div>
  <div v-else class="flex min-w-0 flex-col gap-0.5" role="alert" data-testid="capture-fail">
    <!-- A login need is "needs you" (amber), not a failure (red). -->
    <p class="flex items-start gap-1.5 text-[12.5px] font-medium" :class="reason === 'login' ? 'text-amber-foreground' : 'text-destructive-foreground dark:text-destructive'">
      <KeyRound v-if="reason === 'login'" class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" /><TriangleAlert v-else class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" /><bdi>{{ t(`capture.fail.${reason}.title`, { site: site || t('capture.fail.thisSite') }) }}</bdi></p>
    <p class="ps-5 text-[12px] text-muted-foreground text-pretty">{{ t(`capture.fail.${reason}.hint`) }}</p>
  </div>
</template>

<script setup lang="ts">
import { AppWindow, KeyRound, TriangleAlert } from '@lucide/vue';
import { useI18n } from 'vue-i18n';

defineProps<{ reason: string; site: string; catchable?: boolean }>();
const { t } = useI18n();
</script>
