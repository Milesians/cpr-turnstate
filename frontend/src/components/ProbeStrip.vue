<script setup lang="ts">
import type { Span } from '../timeline'
import type { Cell, Method, Range, RunsPage } from '../api'
import { ZPopover } from '@codex-proxy/ui'
import { computed, onBeforeUnmount, onMounted, reactive, ref, useTemplateRef } from 'vue'
import { api } from '../api'
import { countItems, dateTime, hour, statusLabel } from '../format'
import { cellState, expand, fitSpans, stateLabel, stripLayout, total } from '../timeline'

const props = defineProps<{ range: Range, cells: Cell[], accountId: string, method: Method }>()
const emit = defineEmits<{ open: [runId: string] }>()
const strip = useTemplateRef('strip')
const width = ref(550)
const dpr = ref(typeof window === 'undefined' ? 1 : window.devicePixelRatio || 1)
const layout = computed(() => stripLayout(props.range.slots, width.value, dpr.value))
const spans = computed(() => fitSpans(expand(props.range, props.cells), layout.value.count))

/** 打开一格时才取这段时间的记录；每次打开都重新取 */
const pages = reactive(new Map<string, RunsPage | 'loading' | { error: string }>())
const key = (s: Span) => `${s.start_ms}-${s.end_ms}`
async function load(s: Span) {
  if (!total(s.counts))
    return
  const k = key(s)
  if (!pages.has(k))
    pages.set(k, 'loading')
  try {
    pages.set(k, await api.runs(props.accountId, props.method, s.start_ms, s.end_ms))
  }
  catch (e) {
    pages.set(k, { error: e instanceof Error ? e.message : '读取记录失败' })
  }
}
function page(s: Span) {
  const p = pages.get(key(s))
  return p && p !== 'loading' && 'runs' in p ? p : null
}
function pageError(s: Span) {
  const p = pages.get(key(s))
  return p && p !== 'loading' && 'error' in p ? p.error : ''
}

let observer: ResizeObserver | undefined
onMounted(() => {
  observer = new ResizeObserver(([entry]) => {
    if (entry) {
      width.value = entry.contentRect.width
      dpr.value = window.devicePixelRatio || 1
    }
  })
  if (strip.value)
    observer.observe(strip.value)
})
onBeforeUnmount(() => observer?.disconnect())
</script>

<template>
  <div ref="strip" class="probe-strip" aria-label="探测时间条" :style="{ columnGap: `${layout.gap}px` }">
    <ZPopover v-for="(s, i) in spans" :key="s.start_ms" trigger="hover-click" placement="top" :hover-delay="80" role="dialog" @open="load(s)">
      <template #reference>
        <button
          type="button"
          class="probe-cell"
          :style="{ width: `${layout.widths[i]}px` }"
          :data-state="cellState(s.counts)"
          :aria-label="`${dateTime(s.start_ms)} – ${dateTime(s.end_ms)} · ${stateLabel[cellState(s.counts)]} · ${total(s.counts)} 次探测`"
        />
      </template>
      <div class="probe-tooltip">
        <div class="flex items-center justify-between gap-4">
          <strong class="text-cp-text">{{ stateLabel[cellState(s.counts)] }}</strong>
          <span class="text-cp-xs text-cp-text-tertiary">{{ total(s.counts) }} 次探测</span>
        </div>
        <div class="text-cp-xs tabular-nums text-cp-text-secondary">
          {{ dateTime(s.start_ms) }} — {{ dateTime(s.end_ms) }}
        </div>
        <p v-if="!total(s.counts)" class="m-0 py-2 text-cp-sm text-cp-text-secondary">
          这段时间没有发起探测，不计入正常率。
        </p>
        <template v-else>
          <div class="count-chips">
            <span v-for="c in countItems(s.counts)" :key="c.label" class="count-chip"><i class="state-dot" :data-state="c.state" />{{ c.label }} {{ c.count }}</span>
          </div>
          <p v-if="pageError(s)" class="m-0 text-cp-xs text-cp-warning-text">{{ pageError(s) }}</p>
          <p v-else-if="!page(s)" class="m-0 py-1 text-cp-xs text-cp-text-tertiary">正在读取记录…</p>
          <template v-else>
            <div v-if="page(s)!.runs.length" class="mt-1 flex max-h-64 flex-col gap-1 overflow-y-auto">
              <button
                v-for="run in page(s)!.runs" :key="run.id" type="button"
                class="flex items-start gap-2.5 rounded-cp px-2 py-2 text-left hover:bg-cp-fill-quaternary disabled:cursor-default"
                :disabled="!run.detail" @click="emit('open', run.id)"
              >
                <span class="state-dot mt-1" :data-state="run.status === 'running' ? 'running' : run.status === 'failed' ? 'failed' : run.level ?? 'unknown'" />
                <span class="min-w-0 flex-1">
                  <span class="flex flex-wrap items-center justify-between gap-x-4 text-cp-xs">
                    <span class="text-cp-text">{{ run.model }}</span>
                    <span class="text-cp-text-secondary">{{ hour(run.started_ms) }} · {{ statusLabel(run.status, run.level) }}</span>
                  </span>
                  <span v-if="run.reason || run.status === 'running' || !run.level" class="mt-1 block text-cp-xs text-cp-text-tertiary">{{ run.reason || (run.status === 'running' ? '正在等待探测结果' : '没有可用的判定') }}</span>
                </span>
              </button>
            </div>
            <p v-if="!page(s)!.complete" class="m-0 text-cp-xs text-cp-text-tertiary">
              {{ page(s)!.runs.length ? '更早的部分' : '这段时间' }}只保留了各结果的次数（逐条记录保留最近 48 小时）。
            </p>
          </template>
        </template>
      </div>
    </ZPopover>
  </div>
</template>
