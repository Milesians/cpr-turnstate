<script setup lang="ts">
import type { Slot } from '../api'
import { ZPopover } from '@codex-proxy/ui'
import { computed, onBeforeUnmount, onMounted, ref, useTemplateRef } from 'vue'
import { dateTime, hour, statusLabel } from '../format'
import { cellState, fitSlots, stateLabel } from '../timeline'

const props = defineProps<{ slots: Slot[] }>()
const emit = defineEmits<{ open: [runId: string] }>()
const strip = useTemplateRef('strip')
const width = ref(550)
const cells = computed(() => props.slots.length ? fitSlots(props.slots, width.value) : [])
let observer: ResizeObserver | undefined
onMounted(() => {
  observer = new ResizeObserver(([entry]) => {
    if (entry)
      width.value = entry.contentRect.width
  })
  if (strip.value)
    observer.observe(strip.value)
})
onBeforeUnmount(() => observer?.disconnect())
</script>

<template>
  <div ref="strip" class="probe-strip" aria-label="探测时间条">
    <ZPopover v-for="slot in cells" :key="slot.start_ms" trigger="hover-click" placement="top" :hover-delay="80" role="dialog">
      <template #reference>
        <button
          type="button"
          class="probe-cell"
          :data-state="cellState(slot)"
          :aria-label="`${dateTime(slot.start_ms)} – ${dateTime(slot.end_ms)} · ${stateLabel[cellState(slot)]} · ${slot.runs.length} 次探测`"
        />
      </template>
      <div class="probe-tooltip">
        <div class="flex items-center justify-between gap-4">
          <strong class="text-cp-text">{{ stateLabel[cellState(slot)] }}</strong>
          <span class="text-cp-xs text-cp-text-tertiary">{{ slot.runs.length }} 次探测</span>
        </div>
        <div class="text-cp-xs tabular-nums text-cp-text-secondary">
          {{ dateTime(slot.start_ms) }} — {{ dateTime(slot.end_ms) }}
        </div>
        <p v-if="!slot.runs.length" class="m-0 py-2 text-cp-sm text-cp-text-secondary">
          这段时间没有发起探测，不计入正常率。
        </p>
        <div v-else class="mt-1 flex max-h-64 flex-col gap-1 overflow-y-auto">
          <button
            v-for="run in slot.runs" :key="run.id" type="button"
            class="flex items-start gap-2.5 rounded-cp px-2 py-2 text-left hover:bg-cp-fill-quaternary disabled:cursor-default"
            :disabled="!run.detail" @click="emit('open', run.id)"
          >
            <span class="state-dot mt-1" :data-state="run.status === 'running' ? 'running' : run.status === 'failed' ? 'unknown' : run.level ?? 'unknown'" />
            <span class="min-w-0 flex-1">
              <span class="flex flex-wrap items-center justify-between gap-x-4 text-cp-xs">
                <span class="text-cp-text">{{ run.model }}</span>
                <span class="text-cp-text-secondary">{{ hour(run.started_ms) }} · {{ statusLabel(run.status, run.level) }}</span>
              </span>
              <span class="mt-1 block text-cp-xs text-cp-text-tertiary">{{ run.reason || (run.status === 'running' ? '正在等待探测结果' : '没有可用的判定') }}</span>
            </span>
          </button>
        </div>
      </div>
    </ZPopover>
  </div>
</template>
