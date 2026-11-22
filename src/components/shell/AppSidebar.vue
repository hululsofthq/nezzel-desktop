<template>
  <!-- shadcn-vue Sidebar pattern, collapsible="icon": 212px with labels, 56px icon rail, edge rail toggles. -->
  <aside
    class="group/sidebar relative flex shrink-0 flex-col border-e border-hairline bg-sidebar transition-[width] duration-200 ease-out motion-reduce:transition-none"
    :class="collapsed ? 'w-14' : 'w-[212px]'"
    :data-state="collapsed ? 'collapsed' : 'expanded'"
    :aria-label="t('shell.nav.label')"
  >
    <!-- Title-bar strip. macOS LTR: traffic lights (and the sidebar toggle, placed by the layout). Elsewhere, and in macOS RTL
         where the lights sit over the toolbar instead, the brand. -->
    <div class="flex h-[52px] shrink-0 items-center gap-2 px-4" data-tauri-drag-region>
      <slot v-if="$slots['title-start']" name="title-start" />
      <template v-else-if="showBrand">
        <img :src="nezzelMark" alt="" class="size-6 shrink-0" />
        <span v-if="!collapsed" class="text-[14px] font-semibold tracking-tight">{{ t('shell.brand') }}</span>
      </template>
    </div>

    <nav class="flex min-h-0 flex-1 flex-col overflow-y-auto overflow-x-hidden px-2.5 pb-0">
      <!-- What you saved, by type. Status (in progress, failed) lives in the chips above the list, so the two combine. -->
      <ul class="flex flex-col pt-1" :class="collapsed ? 'items-center gap-1.5' : 'gap-1'">
        <li v-for="item in items" :key="item.key">
          <tooltip :content="item.count ? `${item.label} (${n(item.count)})` : item.label" :side="tipSide" :disabled="!collapsed">
            <router-link :to="item.to" :class="linkCls(item.active, collapsed)" :aria-current="item.active ? 'page' : undefined" :aria-label="collapsed ? item.label : undefined">
              <component :is="item.icon" class="shrink-0" :class="[collapsed ? 'size-[18px]' : 'size-4', item.active ? 'text-primary' : 'text-muted-foreground']" aria-hidden="true" />
              <span v-if="!collapsed" class="flex-1 truncate">{{ item.label }}</span>
              <span v-if="!collapsed && item.count" class="tnum text-[11.5px]" :class="item.active ? 'text-primary-soft-foreground' : 'text-muted-foreground'">{{ n(item.count) }}</span>
            </router-link>
          </tooltip>
        </li>
      </ul>

      <!-- Bottom row shares the status bar's 28px line so both read as one edge. -->
      <div class="mt-auto flex flex-col pt-4" :class="collapsed && 'items-center'">
        <!-- After an update, once: "Updated to X · See what's new" (leads to Settings → About). Rail: the icon alone. -->
        <template v-if="whatsNew.showLine.value">
          <tooltip v-if="collapsed" :content="`${updatedText} · ${t('shell.whatsNew.see')}`" :side="tipSide">
            <button type="button" :class="linkCls(false, true)" class="mb-1.5" data-testid="sidebar-whats-new" :aria-label="`${updatedText}, ${t('shell.whatsNew.see')}`" @click="openWhatsNew">
              <circle-arrow-up class="size-[18px] shrink-0 text-primary" aria-hidden="true" />
            </button>
          </tooltip>
          <div v-else class="mb-1 flex items-start gap-2 rounded-[6px] py-1.5 ps-2 pe-1" data-testid="sidebar-whats-new">
            <circle-arrow-up class="mt-0.5 size-4 shrink-0 text-primary" aria-hidden="true" />
            <p class="min-w-0 flex-1 text-[12px] leading-normal rtl:text-[12.5px] rtl:leading-relaxed">
              {{ updatedText }}<br>
              <button type="button" class="rounded-[4px] font-medium text-primary outline-none hover:underline hover:underline-offset-3 focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--nz-focus)]" @click="openWhatsNew">{{ t('shell.whatsNew.see') }}</button>
            </p>
            <tooltip :content="t('shell.whatsNew.dismiss')" :side="tipSide">
              <button
                type="button"
                class="grid size-[22px] shrink-0 place-items-center rounded-[5px] text-muted-foreground outline-none transition-colors duration-100 hover:bg-muted hover:text-foreground focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--nz-focus)]"
                :aria-label="t('shell.whatsNew.dismiss')"
                data-testid="sidebar-whats-new-dismiss"
                @click="whatsNew.markSeen()"
              >
                <x class="size-3.5" aria-hidden="true" />
              </button>
            </tooltip>
          </div>
        </template>
        <tooltip :content="t('shell.nav.settings')" :side="tipSide" :disabled="!collapsed">
          <router-link :to="{ name: 'settings.general' }" :class="linkCls(isSettings, collapsed)" :aria-current="isSettings ? 'page' : undefined" :aria-label="collapsed ? t('shell.nav.settings') : undefined">
            <settings class="shrink-0" :class="[collapsed ? 'size-[18px]' : 'size-4', isSettings ? 'text-primary' : 'text-muted-foreground']" aria-hidden="true" />
            <span v-if="!collapsed" class="flex-1 truncate">{{ t('shell.nav.settings') }}</span>
          </router-link>
        </tooltip>
        <!-- Maker's line: quiet, expanded sidebar only, leads to Settings → About. -->
        <router-link
          v-if="!collapsed"
          :to="{ name: 'settings.about' }"
          data-testid="sidebar-byline"
          class="mb-1.5 mt-1 self-start rounded-[4px] px-2 py-0.5 text-[11px] text-muted-foreground outline-none transition-colors duration-100 hover:text-foreground focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--nz-focus)] rtl:text-[12px]"
        >{{ t('about.hululsoft.by') }}</router-link>
      </div>
    </nav>

    <!-- SidebarRail: the edge is a click target that toggles the sidebar (hover shows a line). -->
    <tooltip v-if="canToggle" :content="collapsed ? t('shell.sidebar.show') : t('shell.sidebar.hide')">
      <button
        type="button"
        tabindex="-1"
        class="absolute inset-y-0 -end-2 z-20 w-4 cursor-ew-resize outline-none after:absolute after:inset-y-0 after:start-1/2 after:w-[2px] after:-translate-x-1/2 after:transition-colors hover:after:bg-primary/40 rtl:after:translate-x-1/2"
        :aria-label="collapsed ? t('shell.sidebar.show') : t('shell.sidebar.hide')"
        @click="emit('toggle')"
      />
    </tooltip>
  </aside>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { CircleArrowUp, File as FileIcon, Image as ImageIcon, LayoutGrid, Music, Settings, Video, X } from '@lucide/vue';
import nezzelMark from '../../assets/brand/nezzel-mark.svg';
import Tooltip from '@/components/ui/tooltip/Tooltip.vue';
import { useDownloadList } from '../../composables/useDownloadList';
import { useLocaleNumber } from '../../composables/useLocaleNumber';
import { parseListView, viewQuery, type TypeFilter } from '../../helpers/listFilter';
import { usePlatform } from '../../composables/usePlatform';
import { localeDirection } from '../../i18n';
import { useWhatsNew } from '../../composables/useWhatsNew';

defineProps<{ collapsed: boolean; canToggle: boolean }>();
const emit = defineEmits<{ toggle: [] }>();

const { t, locale } = useI18n();
const { count: n } = useLocaleNumber();
const route = useRoute();
const { isMac } = usePlatform();
const { counts } = useDownloadList();

const showBrand = computed(() => !isMac.value || localeDirection(locale.value) === 'rtl');
const tipSide = computed(() => (localeDirection(locale.value) === 'rtl' ? 'left' : 'right'));
const isSettings = computed(() => String(route.name ?? '').startsWith('settings'));

const router = useRouter();
const whatsNew = useWhatsNew();
const updatedText = computed(() => t('shell.whatsNew.updated', { version: `\u2066${whatsNew.version}\u2069` }));
function openWhatsNew() {
  whatsNew.markSeen();
  whatsNew.revealRequested.value = true;
  void router.push({ name: 'settings.about' });
}

const type = computed<TypeFilter | null>(() => (route.name === 'home' ? parseListView(route.query).type : null));
const totals = computed(() => counts('all').types);

/** Picking a type resets the status chips: a "failed" chip carried into a type with no failures would show an empty list. */
const item = (key: TypeFilter, label: string, icon: unknown) => ({
  key,
  label,
  icon,
  to: { name: 'home', query: viewQuery({ type: key, status: 'all' }) },
  count: totals.value[key],
  active: type.value === key,
});
const items = computed(() => [
  item('all', t('shell.nav.all'), LayoutGrid),
  item('video', t('shell.nav.video'), Video),
  item('audio', t('shell.nav.audio'), Music),
  item('photos', t('shell.nav.photos'), ImageIcon),
  item('files', t('shell.nav.files'), FileIcon),
]);

function linkCls(on: boolean, iconOnly: boolean) {
  return [
    'flex items-center gap-2 rounded-[6px] text-start text-[13px] outline-none transition-colors duration-100 focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--nz-focus)]',
    // Rail: 36px square targets on a 6px rhythm, icon centred, the active pill is the same square. Expanded: 32px rows, 4px apart.
    iconOnly ? 'size-9 justify-center px-0' : 'h-8 w-full px-2',
    on ? 'bg-primary-soft font-medium text-primary-soft-foreground' : 'text-foreground/85 hover:bg-foreground/[0.035]',
  ];
}
</script>
