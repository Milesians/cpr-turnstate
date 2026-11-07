<script setup lang="ts">
import type { CellState } from '../timeline'
import type { Overview, OverviewAccount } from '../api'
import { ZButton, ZCheckbox, ZEmpty, ZInput, ZSegmented, ZSelect } from '@codex-proxy/ui'
import { ChevronRight, Search } from '@lucide/vue'
import { computed, ref } from 'vue'
import { ago, rate } from '../format'
import { accountState, pendingChange, pendingLabel, severity, stateLabel } from '../timeline'
import ProbeStrip from './ProbeStrip.vue'

const props = defineProps<{ overview: Overview | null, loading: boolean }>()
const emit = defineEmits<{
  open: [id: string, run?: string]
  probe: [account: OverviewAccount]
  batchProbe: [accounts: OverviewAccount[]]
  batchConfig: [ids: string[]]
}>()
const hours = defineModel<string>('hours', { required: true })

const MAX_BATCH = 200
const search = ref('')
const filter = ref<'all' | 'attention' | CellState>('all')
const plan = ref('')
const sort = ref<'state' | 'name' | 'rate'>('state')
const selectedIds = ref<string[]>([])

const ranges = [{ label: '24 小时', value: '24' }, { label: '7 天', value: '168' }, { label: '30 天', value: '720' }]
const rangeLabel = computed(() => ranges.find(r => r.value === hours.value)?.label ?? '')
const sorts = [{ label: '问题优先', value: 'state' }, { label: '按名称', value: 'name' }, { label: '正常率低在前', value: 'rate' }]

const all = computed(() => props.overview?.accounts ?? [])
const states = computed(() => new Map(all.value.map(a => [a.id, accountState(a)])))
const stateOf = (a: OverviewAccount) => states.value.get(a.id) ?? 'empty'
const planOf = (a: OverviewAccount) => (a.plan_type || 'unknown').toLowerCase()
const plans = computed(() => [...new Set(all.value.map(planOf))].sort())
const planOptions = computed(() => [{ label: '全部类型', value: '' }, ...plans.value.map(p => ({ label: p === 'unknown' ? '未知类型' : p.toUpperCase(), value: p }))])

const filters = computed(() => {
  const count = (pred: (s: CellState) => boolean) => all.value.filter(a => pred(stateOf(a))).length
  return [
    { value: 'all', label: '全部', state: null, count: all.value.length },
    { value: 'attention', label: '需要关注', state: 'degraded', count: count(s => s === 'degraded' || s === 'suspect') },
    { value: 'degraded', label: '降智', state: 'degraded', count: count(s => s === 'degraded') },
    { value: 'suspect', label: '疑似', state: 'suspect', count: count(s => s === 'suspect') },
    { value: 'ok', label: '正常', state: 'ok', count: count(s => s === 'ok') },
    { value: 'unknown', label: '未判定', state: 'unknown', count: count(s => s === 'unknown' || s === 'running') },
    { value: 'empty', label: '没有探测', state: 'empty', count: count(s => s === 'empty') },
  ] as const
})

function matches(a: OverviewAccount) {
  const s = stateOf(a)
  if (filter.value === 'attention' && s !== 'degraded' && s !== 'suspect')
    return false
  if (filter.value === 'unknown' && s !== 'unknown' && s !== 'running')
    return false
  if (!['all', 'attention', 'unknown'].includes(filter.value) && s !== filter.value)
    return false
  if (plan.value && planOf(a) !== plan.value)
    return false
  const q = search.value.trim().toLowerCase()
  return !q || `${a.email ?? ''} ${a.name}`.toLowerCase().includes(q)
}
const rateValue = (a: OverviewAccount) => a.rate.judged ? a.rate.ok / a.rate.judged : 2
const name = (a: OverviewAccount) => a.email || a.name
const accounts = computed(() => all.value.filter(matches).sort((x, y) => {
  if (sort.value === 'rate')
    return rateValue(x) - rateValue(y) || name(x).localeCompare(name(y))
  if (sort.value === 'state')
    return severity[stateOf(x)] - severity[stateOf(y)] || Number(!x.supported) - Number(!y.supported) || name(x).localeCompare(name(y))
  return name(x).localeCompare(name(y))
}))

const selectable = computed(() => accounts.value.filter(a => a.supported).map(a => a.id))
const allSelected = computed({
  get: () => !!selectable.value.length && selectable.value.every(id => selectedIds.value.includes(id)),
  set: value => {
    const visible = new Set(selectable.value)
    selectedIds.value = value
      ? [...new Set([...selectedIds.value, ...selectable.value])].slice(0, MAX_BATCH)
      : selectedIds.value.filter(id => !visible.has(id))
  },
})
function select(id: string, value: boolean) {
  selectedIds.value = value ? [...new Set([...selectedIds.value, id])].slice(0, MAX_BATCH) : selectedIds.value.filter(s => s !== id)
}
const selectedAccounts = computed(() => all.value.filter(a => selectedIds.value.includes(a.id)))
/** 选中了但被当前筛选隐藏的账号：批量操作也会作用到它们，要说清楚 */
const hiddenSelected = computed(() => {
  const visible = new Set(accounts.value.map(a => a.id))
  return selectedIds.value.filter(id => !visible.has(id)).length
})
function keepVisible() {
  const visible = new Set(selectable.value)
  selectedIds.value = selectedIds.value.filter(id => visible.has(id))
}
defineExpose({ prune: () => { selectedIds.value = selectedIds.value.filter(id => all.value.some(a => a.id === id && a.supported)) } })

function lastProbe(a: OverviewAccount) {
  const l = a.latest.turnstate
  return l ? `${ago(l.finished_ms || l.started_ms, props.overview?.now_ms)}` : ''
}
function pendingText(a: OverviewAccount) {
  const p = pendingChange(a)
  return p && a.health ? pendingLabel(a.health.level, p.to, p.streak) : ''
}
/** 行内告警：自动探测被拦截、TG 通知发不出去 */
function warning(a: OverviewAccount): { text: string, detail: string } | null {
  const blockers = a.last_attempt?.blockers
  if (blockers?.length)
    return { text: '自动探测被拦截', detail: blockers.map(b => b.message).join('；') }
  if (a.health?.notification_error)
    return { text: 'TG 通知未送达', detail: a.health.notification_error }
  return null
}
function schedule(a: OverviewAccount) {
  const s = a.schedule.turnstate
  return s.enabled ? `每 ${s.interval_minutes} 分钟` : '自动探测关'
}
</script>

<template>
  <section class="filter-bar" aria-label="按当前判定筛选">
    <button
      v-for="f in filters" :key="f.value" type="button" class="filter-chip"
      :class="{ active: filter === f.value, emphasis: f.value === 'attention' && f.count > 0 }"
      :aria-pressed="filter === f.value" @click="filter = f.value"
    >
      <i v-if="f.state" class="state-dot" :data-state="f.state" />
      <span>{{ f.label }}</span>
      <strong>{{ overview ? f.count : '—' }}</strong>
    </button>
  </section>

  <section class="monitor-list" :aria-busy="loading">
    <div class="monitor-toolbar">
      <div class="flex min-w-0 flex-1 flex-wrap items-center gap-2">
        <ZInput v-model="search" placeholder="搜索名称或邮箱" clearable class="monitor-search" aria-label="搜索账号"><template #prefix><Search :size="15" aria-hidden="true" /></template></ZInput>
        <ZSelect v-if="plans.length > 1" v-model="plan" :options="planOptions" class="w-32" aria-label="账号类型" />
        <ZSelect v-model="sort" :options="sorts" class="w-36" aria-label="排序" />
      </div>
      <ZSegmented v-model="hours" :options="ranges" size="small" aria-label="时间条范围" />
    </div>
    <div class="monitor-head">
      <ZCheckbox v-model="allSelected" :indeterminate="!allSelected && selectable.some(id => selectedIds.includes(id))" :disabled="!selectable.length" :aria-label="`全选筛选出的 ${selectable.length} 个账号`" />
      <span class="head-account">{{ accounts.length }} 个账号<template v-if="accounts.length !== all.length"> / 共 {{ all.length }}</template></span>
      <span class="head-strip">测票 · 最近 {{ rangeLabel }} <span aria-hidden="true">→</span> 现在</span>
      <span class="head-rate">正常率</span>
      <span class="head-actions" />
    </div>
    <div v-if="loading && !overview" class="monitor-loading">正在读取探测记录…</div>
    <ZEmpty v-else-if="!accounts.length" class="py-12" :title="all.length ? '没有符合筛选的账号' : 'CPR 里还没有 OpenAI 账号'" />
    <article v-for="a in accounts" :key="a.id" class="monitor-row" :data-account="a.id" :data-state="stateOf(a)">
      <ZCheckbox :model-value="selectedIds.includes(a.id)" :disabled="!a.supported || (selectedIds.length >= MAX_BATCH && !selectedIds.includes(a.id))" :aria-label="`选择 ${name(a)}`" @update:model-value="(v: boolean) => select(a.id, v)" />
      <button type="button" class="monitor-account" @click="emit('open', a.id)">
        <span class="account-name">
          <span class="truncate">{{ name(a) }}</span>
          <span v-if="a.plan_type" class="account-plan">{{ a.plan_type }}</span>
        </span>
        <span class="account-meta">
          <span class="verdict-pill" :data-state="stateOf(a)"><i class="state-dot" :data-state="stateOf(a)" />{{ stateLabel[stateOf(a)] }}</span>
          <span v-if="pendingText(a)" class="pending-change" :title="`连续 3 次同类结果才改变当前判定，已 ${pendingChange(a)!.streak} 次`">{{ pendingText(a) }}</span>
          <span v-if="!a.supported">不支持</span>
          <span v-else-if="!a.enabled">已停用</span>
          <span v-else class="text-cp-text-tertiary">{{ lastProbe(a) ? `${lastProbe(a)}探测` : '还没探测' }} · {{ schedule(a) }}</span>
          <span v-if="warning(a)" class="row-warning" :title="warning(a)!.detail">{{ warning(a)!.text }}</span>
        </span>
      </button>
      <div class="monitor-strip">
        <ProbeStrip v-if="overview && a.supported" :range="overview.range" :cells="a.strip" :account-id="a.id" method="turnstate" @open="run => emit('open', a.id, run)" />
        <span v-else class="text-cp-xs text-cp-text-tertiary">非 OpenAI OAuth 账号，不探测</span>
      </div>
      <span class="track-rate" :class="{ 'rate-empty': !a.rate.judged }" :title="`${rangeLabel}内 ${a.rate.ok} / ${a.rate.judged} 次有效判定为正常；失败、未判定和无探测不计入`">{{ rate(a.rate) }}</span>
      <div class="monitor-actions">
        <ZButton size="small" :disabled="!a.supported" @click="emit('probe', a)">探测</ZButton>
        <button type="button" class="account-details" :aria-label="`查看 ${name(a)}`" title="详情与配置" @click="emit('open', a.id)"><ChevronRight :size="18" aria-hidden="true" /></button>
      </div>
    </article>
    <footer class="monitor-legend">
      <span v-for="s in (['ok', 'suspect', 'degraded', 'unknown', 'empty', 'running'] as const)" :key="s" class="inline-flex items-center gap-1.5"><i class="legend-cell" :data-state="s" />{{ stateLabel[s] }}</span>
      <span class="legend-note">账号状态是测票的当前判定（连续 3 次确认）· 时间条和正常率是每次的原始结果</span>
    </footer>
  </section>

  <Transition name="selection">
    <div v-if="selectedIds.length" class="selection-bar" role="region" aria-label="已选账号的操作">
      <span class="text-cp-sm">已选 <strong>{{ selectedIds.length }}</strong> 个<span v-if="selectedIds.length >= MAX_BATCH" class="text-cp-xs text-cp-text-tertiary">（单批上限 {{ MAX_BATCH }}）</span></span>
      <span v-if="hiddenSelected" class="text-cp-xs text-cp-warning-text">其中 {{ hiddenSelected }} 个不在当前筛选里，也会被操作</span>
      <button v-if="hiddenSelected" type="button" class="text-cp-xs text-cp-primary hover:underline" @click="keepVisible">只保留筛选出的</button>
      <span class="flex-1" />
      <ZButton size="small" type="primary" @click="emit('batchProbe', selectedAccounts)">批量探测</ZButton>
      <ZButton size="small" @click="emit('batchConfig', selectedIds)">批量配置</ZButton>
      <button type="button" class="text-cp-xs text-cp-text-secondary hover:underline" @click="selectedIds = []">清空</button>
    </div>
  </Transition>
</template>
