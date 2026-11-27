<template>
  <!-- The engine's own mark in its brand colour; the name stays in the text beside it, so the mark is decoration. -->
  <svg viewBox="0 0 24 24" class="size-4 shrink-0" aria-hidden="true" focusable="false" :data-engine="engine">
    <path :d="mark.path" :fill="mark.color" />
  </svg>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { ENGINE_MARKS } from '../../assets/engines/marks';
import type { SearchEngine } from '../../tauri/types/preferences';

const props = defineProps<{ engine: SearchEngine }>();
const mark = computed(() => ENGINE_MARKS[props.engine]);
</script>
