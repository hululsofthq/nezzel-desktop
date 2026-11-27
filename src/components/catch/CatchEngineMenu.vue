<template>
  <!-- Which engine typed words go to, at the start of the home field and the address row. -->
  <DropdownMenu :open="store.engineMenu === place" :dir="localeDirection(locale)" @update:open="open => (store.engineMenu = open ? place : null)">
    <DropdownMenuTrigger as-child>
      <Button
        type="button"
        variant="ghost"
        size="xs"
        class="shrink-0 gap-1 px-1.5 text-muted-foreground data-[state=open]:bg-accent data-[state=open]:text-foreground"
        :aria-label="t('catch.search.engine', { engine: store.engineName })"
        :data-testid="`catch-engine-${place}`"
      >
        <EngineLogo :engine="store.engine" class="!size-3.5" />
        <bdi :class="place === 'address' && 'hidden @[360px]:inline'">{{ store.engineName }}</bdi>
        <ChevronDown class="!size-3" aria-hidden="true" />
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="start" @close-auto-focus="onCloseFocus">
      <DropdownMenuLabel>{{ t('catch.search.label') }}</DropdownMenuLabel>
      <DropdownMenuCheckboxItem
        v-for="(option, id) in SEARCH_ENGINES"
        :key="id"
        :model-value="store.engine === id"
        @update:model-value="choose(id)"
      >
        <EngineLogo :engine="id" />
        <bdi>{{ option.name }}</bdi>
      </DropdownMenuCheckboxItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>

<script setup lang="ts">
import { useI18n } from 'vue-i18n';
import { ChevronDown } from '@lucide/vue';
import EngineLogo from './EngineLogo.vue';
import { Button } from '@/components/ui/button';
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuLabel, DropdownMenuTrigger } from '@/components/ui/dropdown-menu';
import { useCatchStore } from '../../stores/catch';
import { SEARCH_ENGINES, type SearchEngine } from '../../tauri/types/preferences';
import { localeDirection } from '../../i18n';

defineProps<{ place: 'home' | 'address' }>();
const { t, locale } = useI18n();
const emit = defineEmits<{ picked: [] }>();
const store = useCatchStore();

/** The list stays mounted by the checkbox items: a pick closes it. */
let picked = false;
function choose(id: SearchEngine) {
  picked = true;
  store.engineMenu = null;
  void store.setEngine(id);
}
/** After a pick the keyboard goes to the field (`picked`), not back to this button: the next letters are the search. */
function onCloseFocus(event: Event) {
  if (!picked) return;
  picked = false;
  event.preventDefault();
  emit('picked');
}
</script>
