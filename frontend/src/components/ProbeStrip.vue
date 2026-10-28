<script setup lang="ts">
import type { Slot } from '../api'
import { ZPopover } from '@codex-proxy/ui'
import { hour, levelBar, levelLabel, statusLabel } from '../format'

defineProps<{ slots: Slot[] }>()
const emit = defineEmits<{ open: [runId: string] }>()
</script>

<template>
  <div class="flex h-7 min-w-0 flex-1 items-stretch gap-[2px]">
    <ZPopover
      v-for="slot in slots"
      :key="slot.start_ms"
      trigger="hover-click"
      placement="top"
      :hover-delay="80"
    >
      <template #reference>
        <button
          type="button"
          class="min-w-[3px] flex-1 rounded-[2px] transition-opacity hover:opacity-75"
          :class="levelBar(slot.level)"
          :aria-label="`${hour(slot.start_ms)} ${slot.level ? levelLabel[slot.level] : '没有探测'}`"
        />
      </template>
      <div class="flex max-w-80 flex-col gap-2 p-3 text-cp-sm">
        <div class="font-medium text-cp-text-heading">
          {{ hour(slot.start_ms) }} – {{ hour(slot.start_ms + 3_600_000) }}
        </div>
        <div v-if="!slot.runs.length" class="text-cp-text-tertiary">
          这一小时没有探测
        </div>
        <button
          v-for="run in slot.runs"
          :key="run.id"
          type="button"
          class="flex flex-col items-start gap-0.5 rounded-cp px-2 py-1.5 text-left hover:bg-cp-fill-tertiary disabled:cursor-default disabled:hover:bg-transparent"
          :disabled="!run.detail"
          @click="emit('open', run.id)"
        >
          <span class="flex items-center gap-2">
            <span class="inline-block size-2 rounded-full" :class="levelBar(run.status === 'failed' ? 'unknown' : run.level)" />
            <span class="text-cp-text">{{ hour(run.started_ms) }} · {{ run.model }} · {{ statusLabel(run.status, run.level) }}</span>
          </span>
          <span class="line-clamp-3 text-cp-xs text-cp-text-secondary">{{ run.reason }}</span>
        </button>
      </div>
    </ZPopover>
  </div>
</template>
