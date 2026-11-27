<template>
  <!-- A tab's mark: search for the home page, a dark mask for a private tab, else the host's first letter. -->
  <Search v-if="row.home && !row.tab.private" :class="large ? 'size-4' : 'size-4 shrink-0 text-muted-foreground'" aria-hidden="true" />
  <span
    v-else
    class="grid shrink-0 place-items-center font-semibold leading-none"
    :class="[large ? 'size-5 rounded-[5px] text-[11px]' : 'size-4 rounded-[4px] text-[10px]', row.tab.private ? 'bg-foreground text-background' : 'bg-foreground/[0.11] text-foreground']"
    aria-hidden="true"
  >
    <VenetianMask v-if="row.tab.private" class="size-[11px]" :stroke-width="2" />
    <template v-else>{{ row.letter || '·' }}</template>
  </span>
</template>

<script setup lang="ts">
import { Search, VenetianMask } from '@lucide/vue';
import type { CatchTabRow } from '../../stores/catch';

defineProps<{ row: CatchTabRow; large?: boolean }>();
</script>
