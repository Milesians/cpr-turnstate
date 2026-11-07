<script setup lang="ts">
import type { Day } from '../api'
import { computed, ref, watch } from 'vue'
import { countItems, day } from '../format'
import { judged, total } from '../timeline'

/**
 * 每天各结果的次数，堆叠成柱；没有探测的日子留一条浅底。
 * 悬停或点一根柱子，上方显示那天的次数（手机上点一下就行），默认显示今天。
 */
const props = defineProps<{ days: Day[] }>()

const layers = [
  { key: 'ok', state: 'ok', label: '正常' },
  { key: 'suspect', state: 'suspect', label: '疑似' },
  { key: 'degraded', state: 'degraded', label: '降智' },
  { key: 'unknown', state: 'unknown', label: '未判定' },
  { key: 'failed', state: 'failed', label: '失败' },
] as const

const height = 96
const gap = 3
const selected = ref(props.days.length - 1)
watch(() => props.days.length, n => { selected.value = n - 1 })
const max = computed(() => Math.max(1, ...props.days.map(total)))
const barWidth = computed(() => (600 - gap * (props.days.length - 1)) / Math.max(1, props.days.length))
const bars = computed(() => props.days.map((d, i) => {
  let y = height
  const segments = layers.flatMap(({ key, state }) => {
    const n = d[key] ?? 0
    if (!n)
      return []
    const h = Math.max(1.5, (n / max.value) * height)
    y -= h
    return [{ state, y, h }]
  })
  return { x: i * (barWidth.value + gap), segments, empty: !total(d) }
}))
const current = computed(() => props.days[selected.value])
const summary = computed(() => {
  const d = current.value
  if (!d)
    return null
  const j = judged(d)
  return {
    label: selected.value === props.days.length - 1 ? `今天（${day(d.start_ms)}）` : day(d.start_ms),
    items: countItems(d),
    rate: j ? `${Math.round(((d.ok ?? 0) / j) * 100)}%` : '',
  }
})
const ticks = computed(() => {
  const n = props.days.length
  return n ? [0, Math.floor(n / 2), n - 1].map(i => ({ i, label: i === n - 1 ? '今天' : day(props.days[i]!.start_ms) })) : []
})
</script>

<template>
  <figure class="trend-chart">
    <figcaption class="trend-head">
      <span class="text-cp-xs text-cp-text-tertiary">最近 {{ days.length }} 天</span>
      <span v-if="summary" class="trend-summary" aria-live="polite">
        <strong>{{ summary.label }}</strong>
        <template v-if="summary.items.length">
          <span v-for="c in summary.items" :key="c.label" class="count-chip"><i class="state-dot" :data-state="c.state" />{{ c.label }} {{ c.count }}</span>
          <span v-if="summary.rate" class="text-cp-text-secondary">正常率 {{ summary.rate }}</span>
        </template>
        <span v-else class="text-cp-text-tertiary">没有探测</span>
      </span>
    </figcaption>
    <svg :viewBox="`0 0 600 ${height}`" preserveAspectRatio="none" role="img" :aria-label="`最近 ${days.length} 天每天的探测结果`">
      <g
        v-for="(b, i) in bars" :key="i" class="trend-day" :data-selected="i === selected"
        @mouseenter="selected = i" @click="selected = i"
      >
        <rect v-if="b.empty" :x="b.x" :y="height - 3" :width="barWidth" height="3" rx="1" class="trend-empty" />
        <rect v-for="(s, j) in b.segments" :key="j" :x="b.x" :y="s.y" :width="barWidth" :height="s.h" :data-state="s.state" class="trend-bar" />
        <rect :x="b.x - gap / 2" y="0" :width="barWidth + gap" :height="height" class="trend-hit" />
      </g>
    </svg>
    <div class="trend-axis">
      <span v-for="t in ticks" :key="t.i">{{ t.label }}</span>
    </div>
    <div class="trend-legend">
      <span v-for="l in layers" :key="l.key"><i class="legend-cell" :data-state="l.state" />{{ l.label }}</span>
    </div>
  </figure>
</template>
