<template>
  <!-- Failed read: only what can work. Quiet actions first, the primary last (inline end), like the picks. -->
  <div class="flex items-center gap-2" data-testid="capture-fail-actions">
    <template v-for="action in actions.secondary" :key="action">
      <!-- Beside the reason's own actions the browser is an icon: its full words do not fit beside them in every
           language. On the card (360 px) Retry beside it is an icon too, so the login action keeps its words. -->
      <tooltip v-if="iconOnly(action)" :content="iconLabel(action)">
        <Button type="button" variant="outline" size="icon" :aria-label="iconLabel(action)" :disabled="busy" :data-testid="action === 'catch' ? 'capture-catch-try' : undefined" @click="emit('act', action)">
          <AppWindow v-if="action === 'catch'" aria-hidden="true" /><RotateCw v-else aria-hidden="true" />
        </Button>
      </tooltip>
      <Button
        v-else
        type="button"
        :variant="action === 'report' ? 'ghost' : 'outline'"
        size="sm"
        :class="action === 'report' && 'px-2 text-muted-foreground'"
        :disabled="busy"
        @click="emit('act', action)"
      >
        {{ label(action) }}
      </Button>
    </template>
    <Button v-if="actions.primary" type="button" size="sm" :variant="reason === 'login' || actions.primary === 'catch' ? 'amber' : 'default'" :disabled="busy" :data-testid="actions.primary === 'catch' ? 'capture-catch-open' : undefined" @click="emit('act', actions.primary)">
      <RotateCw v-if="actions.primary === 'retry'" aria-hidden="true" /><AppWindow v-else-if="actions.primary === 'catch'" aria-hidden="true" />{{ label(actions.primary) }}
    </Button>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { AppWindow, RotateCw } from '@lucide/vue';
import { Button } from '@/components/ui/button';
import Tooltip from '../ui/tooltip/Tooltip.vue';
import { failActions, type FailAction, type FailReason } from '../../helpers/captureFailure';

const props = defineProps<{ reason: FailReason; busy: boolean; catchable?: boolean; compact?: boolean }>();
const emit = defineEmits<{ act: [action: FailAction] }>();
const { t } = useI18n();
const actions = computed(() => failActions(props.reason, props.catchable));
const iconOnly = (action: FailAction) => action === 'catch' || (props.compact && action === 'retry' && actions.value.secondary.includes('catch'));
const iconLabel = (action: FailAction) => t(action === 'catch' ? 'catch.row.try' : 'capture.fail.retry');
const label = (action: FailAction) => action === 'catch' ? t('catch.row.open') : action === 'options' ? t(props.reason === 'login' ? 'capture.fail.useLogin' : 'capture.options') : t(`capture.fail.${action}`);
</script>
