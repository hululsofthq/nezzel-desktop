<template>
  <form class="mx-auto grid min-w-0 max-w-[720px] flex-1 grid-cols-[auto_minmax(0,1fr)_auto] items-center gap-x-2" autocomplete="off" @submit.prevent="onSubmit()">
    <!-- The second way in, always in sight: a labelled button outside the field, at its start (the field and
         Download stay one pair). Its name is the visible word, so voice control finds it in every language. -->
    <tooltip :content="`${browserLabel}  ${browserKeys}`">
      <Button
        type="button"
        variant="secondary"
        class="h-8 gap-1.5 rounded-[7px] px-2.5"
        data-testid="toolbar-browser"
        :aria-keyshortcuts="ariaKeyShortcuts(BUILT_IN_KEYS.browser)"
        @click="openBrowser"
      >
        <AppWindow class="text-primary" aria-hidden="true" />
        <span>{{ t('catch.entry.button') }}</span>
      </Button>
    </tooltip>
    <!-- Grows and centres in the toolbar, like Safari's address field. -->
    <div class="flex h-8 w-full min-w-28 flex-1 items-center gap-1.5 rounded-[7px] ps-2.5 bg-foreground/[0.055] pe-0.5 transition-[background-color,box-shadow] focus-within:bg-card focus-within:ring-2 focus-within:ring-ring/30">
      <Link class="size-3.5 shrink-0 text-muted-foreground rtl:-scale-x-100" aria-hidden="true" />
      <label for="queue-url-input" class="sr-only">{{ t('downloads.field.label') }}</label>
      <Input variant="bare"
        id="queue-url-input"
        ref="input"
        v-model="url"
        class="h-full min-w-0 flex-1 bg-transparent text-start text-[12.5px] text-foreground outline-none placeholder:text-muted-foreground"
        :placeholder="nudge ? t('downloads.field.pasteFirst') : placeholder"
        :class="nudge && 'placeholder:text-foreground/80'"
        type="text"
        inputmode="url"
        :dir="url ? 'auto' : clipboardHasValidUrl ? 'ltr' : undefined"
        autocomplete="off"
        autocorrect="off"
        autocapitalize="off"
        spellcheck="false"
        :aria-describedby="feedback ? 'queue-url-status' : undefined"
        @keydown.enter.shift.prevent="onSubmit(true)"
        @paste="onNativePaste" />
      <Kbd v-if="!url" class="hidden sm:inline-flex">{{ isMac ? t('shell.keys.pasteMac') : t('shell.keys.paste') }}</Kbd>
      <!-- Clipboard state as a small indicator, not placeholder text. -->
      <tooltip :content="clipboardTip">
        <span class="inline-flex size-5 shrink-0 items-center justify-center" :aria-label="clipboardTip" role="img">
          <ClipboardCheck v-if="watchClipboard.enabled" class="size-3.5 text-primary" aria-hidden="true" />
          <ClipboardX v-else class="size-3.5 text-muted-foreground/60" aria-hidden="true" />
        </span>
      </tooltip>
      <input ref="fileInput" type="file" accept=".csv,.txt,text/csv,text/plain" class="hidden" @change="onFile" />
      <div ref="menuRoot" class="relative">
        <tooltip :content="moreLabel">
          <Button
            size="icon-xs"
            variant="ghost"
            class="relative size-6 text-muted-foreground"
            :aria-label="moreLabel"
            aria-haspopup="menu"
            :aria-expanded="menuOpen ? 'true' : 'false'"
            @click="menuOpen = !menuOpen"
            @keydown.down.prevent="menuOpen = true"
          >
            <Ellipsis />
            <!-- A saved filter can skip links days later: a quiet dot says one is still on. -->
            <span v-if="filtersOnLabel" data-testid="filters-on-dot" class="pointer-events-none absolute end-0.5 top-0.5 size-1.5 rounded-full bg-primary" aria-hidden="true" />
          </Button>
        </tooltip>
        <ul
          v-if="menuOpen"
          class="absolute end-0 top-8 z-[80] w-72 rounded-lg border border-hairline bg-popover p-1 text-[13px] shadow-lg animate-in fade-in-0 zoom-in-95 duration-150"
          role="menu"
          @keydown="onMenuKey"
        >
          <li role="none">
            <button role="menuitem" type="button" :class="itemCls" data-testid="menu-catch-open" @click="openCatchPage">
              <AppWindow aria-hidden="true" />
              <span class="min-w-0 flex-1">
                <span class="block">{{ t('catch.menuEntry') }}</span>
                <span class="block text-[12px] text-muted-foreground">{{ t('catch.menuEntryHint') }}</span>
              </span>
            </button>
          </li>
          <li role="none">
            <button role="menuitem" type="button" :class="itemCls" @click="pickFile">
              <FileUp aria-hidden="true" />
              <span class="min-w-0 flex-1">
                <span class="block">{{ t('downloads.field.importFile') }}</span>
                <span class="block text-[12px] text-muted-foreground">{{ t('downloads.field.importFileHint') }}</span>
              </span>
            </button>
          </li>
          <li role="none">
            <button role="menuitemcheckbox" type="button" :class="itemCls" :aria-checked="watchClipboard.enabled ? 'true' : 'false'" @click="toggleWatch">
              <ClipboardCheck aria-hidden="true" />
              <span class="min-w-0 flex-1">
                <span class="block">{{ t('downloads.field.watch') }}</span>
                <span class="block text-[12px] text-muted-foreground">{{ t('downloads.field.watchHint') }}</span>
              </span>
              <Switch :model-value="watchClipboard.enabled" tabindex="-1" aria-hidden="true" class="pointer-events-none mt-0.5" />
            </button>
          </li>
          <li role="none">
            <button role="menuitem" type="button" :class="[itemCls, hasActiveInputFilters && 'is-on']" @click="openFilters">
              <Filter :class="hasActiveInputFilters && 'text-primary'" aria-hidden="true" />
              <span class="min-w-0 flex-1">
                <span class="block" :class="hasActiveInputFilters && 'font-medium text-primary'">{{ t('downloads.field.filters') }}</span>
                <span class="block text-[12px] text-muted-foreground">{{ hasActiveInputFilters ? t('downloads.field.filtersOn') : t('downloads.field.filtersHint') }}</span>
              </span>
            </button>
          </li>
        </ul>
      </div>
    </div>
    <p class="sr-only" role="status" aria-live="polite">{{ nudge ? t('downloads.field.pasteFirst') : '' }}</p>
    <!-- Teal only once there is a link to act on; empty, it focuses the field and says what to do. -->
    <Button type="submit" :variant="canSubmit ? 'default' : 'outline'" @click.shift.prevent="onSubmit(true)">
      {{ t('downloads.field.submit') }}
    </Button>
    <p id="queue-url-status" :class="feedback ? 'col-start-2 col-end-4 mt-1 text-start text-xs text-muted-foreground' : 'sr-only'" role="status" aria-live="polite">{{ feedback }}</p>
  </form>
</template>

<script setup lang="ts">
import { Input, type InputApi } from '../ui/input';
import { readClipboardText } from '../../tauri/clipboard';
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';
import { AppWindow, ClipboardCheck, ClipboardX, Ellipsis, FileUp, Filter, Link } from '@lucide/vue';
import Switch from '@/components/ui/switch/Switch.vue';
import Tooltip from '@/components/ui/tooltip/Tooltip.vue';
import { Button } from '@/components/ui/button';
import Kbd from '@/components/ui/kbd/Kbd.vue';
import { usePlatform } from '../../composables/usePlatform';
import { useToastStore } from '../../stores/toast';
import { useLinkIntake } from '../../composables/useLinkIntake';
import { useCaptureShortcut } from '../../composables/useCaptureShortcut';
import { useCapturePlatform } from '../../composables/useCapturePlatform';
import { useSettingsStore } from '../../stores/settings';
import { useOpenCatch } from '../../composables/useOpenCatch';
import { ariaKeyShortcuts, BUILT_IN_KEYS, shortcutLabel } from '../../helpers/shortcut';
import { activeInputFilterKinds, settingsToInputFilterOverride } from '../../helpers/inputFilters';
import { isValidUrl } from '../../helpers/url';
import { flattenUrlInput } from '../../helpers/urlImport';

const { t } = useI18n();
const router = useRouter();
const { url, placeholder, clipboardHasValidUrl, canSubmit, feedback, submit, importFile, hasActiveInputFilters, watchClipboard } = useLinkIntake();
const { isMac } = usePlatform();
const capture = useCaptureShortcut();
const capturePlatform = useCapturePlatform();
const settings = useSettingsStore();
const filterKinds = computed(() => activeInputFilterKinds(settingsToInputFilterOverride(settings.settings)));
const filtersOnLabel = computed(() => {
  const kinds = filterKinds.value;
  if (kinds.length === 0) return '';
  return kinds.length === 1 ? t(`downloads.field.filterOn.${kinds[0]}`) : t('downloads.field.filtersOnCount', { count: kinds.length }, kinds.length);
});
const moreLabel = computed(() => (filtersOnLabel.value
  ? t('downloads.field.moreWithFilters', { more: t('downloads.field.more'), filters: filtersOnLabel.value })
  : t('downloads.field.more')));
const clipboardTip = computed(() => {
  const state = watchClipboard.enabled ? t('downloads.field.watchOnTip') : t('downloads.field.watchOffTip');
  if (capturePlatform.session === 'wayland') return state;
  const shortcut = settings.settings.input.globalShortcuts && !capture.failed && capture.active;
  return t('downloads.field.clipboardTip', {
    state,
    keys: shortcut
      ? t('downloads.field.captureShortcut', { shortcut: shortcutLabel(shortcut, isMac.value) })
      : t('downloads.field.captureSettings'),
  });
});

const input = ref<InputApi | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);
const menuRoot = ref<HTMLElement | null>(null);
const menuOpen = ref(false);
const itemCls = 'flex w-full items-start gap-2.5 rounded-[6px] px-2 py-1.5 text-start outline-none transition-colors duration-100 hover:bg-foreground/[0.05] focus-visible:bg-foreground/[0.05] focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[var(--nz-focus)] [&>svg]:mt-0.5 [&>svg]:size-4 [&>svg]:shrink-0 [&>svg]:text-muted-foreground';
let immediateFromFile = false;

/** With nothing to add (empty field, no copied link) the button points the user at the field instead. */
function onSubmit(immediateDownload = false) {
  if (!canSubmit.value) {
    url.value = '';
    pointAtField();
    return;
  }
  void submit(immediateDownload);
}

/** Empty submit: focus the field and swap its placeholder for a short "paste a link first" for a moment. */
const nudge = ref(false);
let nudgeTimer: ReturnType<typeof setTimeout> | undefined;
function pointAtField() {
  input.value?.focus();
  nudge.value = true;
  clearTimeout(nudgeTimer);
  nudgeTimer = setTimeout(() => {
    nudge.value = false;
  }, 2000);
}
watch(url, () => {
  nudge.value = false;
}, { flush: 'sync' });

/** Closing the menu from an item puts focus back on its button, so keyboard users don't land on the page body. */
function closeMenu() {
  menuOpen.value = false;
  menuRoot.value?.querySelector<HTMLElement>('[aria-haspopup="menu"]')?.focus();
}

function pickFile(event: MouseEvent) {
  immediateFromFile = event.shiftKey;
  closeMenu();
  fileInput.value?.click();
}

async function onFile(event: Event) {
  const target = event.target as HTMLInputElement;
  const file = target.files?.[0];
  target.value = '';
  await importFile(file, immediateFromFile);
  immediateFromFile = false;
}

/** A switch, not a one-shot command: the menu stays open so the new state is visible. */
function toggleWatch() {
  watchClipboard.toggle();
}

const openCatch = useOpenCatch();
const browserLabel = computed(() => t('catch.entry.label'));
/** Isolated left-to-right, so the keycaps keep their order whatever the translated label ends with. */
const browserKeys = computed(() => `\u2066${shortcutLabel(BUILT_IN_KEYS.browser, isMac.value)}\u2069`);
function openBrowser() {
  void openCatch();
}
function openCatchPage() {
  menuOpen.value = false;
  void openCatch();
}

function openFilters() {
  menuOpen.value = false;
  void router.push({ name: 'input-filters' });
}

/** Menu keys: focus moves into the menu on open; ↑/↓ move, Esc closes and returns to the button, Tab closes. */
watch(menuOpen, async (open) => {
  if (!open) return;
  await nextTick();
  menuRoot.value?.querySelector<HTMLElement>('[role^="menuitem"]')?.focus();
});

function onMenuKey(event: KeyboardEvent) {
  const items = Array.from(menuRoot.value?.querySelectorAll<HTMLElement>('[role^="menuitem"]') ?? []);
  const index = items.indexOf(document.activeElement as HTMLElement);
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    const step = event.key === 'ArrowDown' ? 1 : -1;
    items[(index + step + items.length) % items.length]?.focus();
  } else if (event.key === 'Home' || event.key === 'End') {
    event.preventDefault();
    items[event.key === 'Home' ? 0 : items.length - 1]?.focus();
  } else if (event.key === 'Escape') {
    event.preventDefault();
    closeMenu();
  } else if (event.key === 'Tab') {
    menuOpen.value = false;
  }
}

function onDocumentClick(event: MouseEvent) {
  if (menuOpen.value && menuRoot.value && !menuRoot.value.contains(event.target as Node)) {
    menuOpen.value = false;
  }
}

async function paste() {
  input.value?.focus();
  const text = await readClipboardText();
  if (text != null) url.value = flattenUrlInput(text);
  else useToastStore().showToast(t('feedback.clipboardFailed'), { style: 'error' });
}

/** WebKitGTK on some Wayland desktops hands the field an empty paste; read the clipboard ourselves then. */
function onNativePaste(event: ClipboardEvent) {
  const text = event.clipboardData?.getData('text/plain');
  const field = event.target as HTMLInputElement;
  const draft = field.value;
  const start = field.selectionStart ?? draft.length;
  const end = field.selectionEnd ?? start;
  const before = draft.slice(0, start);
  const after = draft.slice(end);

  function insertPaste(pasted: string, native: boolean) {
    let replacement = flattenUrlInput(pasted);
    if (isValidUrl(before.match(/[^\s,]+$/)?.[0] ?? null) && isValidUrl(replacement.match(/^[^\s,]+/)?.[0] ?? null)) {
      replacement = ' ' + replacement;
    }
    if (isValidUrl(replacement.match(/[^\s,]+$/)?.[0] ?? null) && isValidUrl(after.match(/^[^\s,]+/)?.[0] ?? null)) {
      replacement += ' ';
    }
    if (native && replacement === pasted) return;
    if (native) event.preventDefault();
    field.setRangeText(replacement, start, end, 'end');
    url.value = field.value;
  }

  if (text) {
    insertPaste(text, true);
    return;
  }
  event.preventDefault();
  void readClipboardText().then((text) => {
    if (text && field.value === draft && field.selectionStart === start && field.selectionEnd === end) {
      insertPaste(text, false);
    }
  });
}

defineExpose({ focus: () => input.value?.focus(), paste });

onMounted(() => {
  input.value?.focus();
  document.addEventListener('click', onDocumentClick);
});
onBeforeUnmount(() => {
  document.removeEventListener('click', onDocumentClick);
  clearTimeout(nudgeTimer);
});
</script>
