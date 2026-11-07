<script setup lang="ts">
import type { AccountDetail, Counts, Method, Models, Run, Schedule, Summary } from '../api'
import { ZButton, ZDialog, ZEmpty, ZInputNumber, ZSegmented, ZSelect, ZSwitch, ZTag } from '@codex-proxy/ui'
import { ArrowLeft } from '@lucide/vue'
import { computed, onMounted, ref, shallowRef } from 'vue'
import { api, methodLabel, methods, scheduledMethods } from '../api'
import { ago, dateTime, levelLabel, levelTag, rate, statusLabel } from '../format'
import { defaultModels } from '../probe-models'
import { judged, merge } from '../timeline'
import ProbeDialog from './ProbeDialog.vue'
import RunView from './RunView.vue'
import TrendChart from './TrendChart.vue'

const props = defineProps<{ accountId: string, models: Models | null, initialRun?: string }>()
const emit = defineEmits<{ back: [] }>()

const detail = shallowRef<AccountDetail | null>(null)
const error = ref('')
const saving = ref<Method | null>(null)
const saveError = ref('')
const dialog = ref(false)
const tab = ref<Method>('turnstate')
const runFilter = ref<'all' | 'abnormal' | 'failed'>('all')
const openRun = shallowRef<Run | null>(null)
const runOpen = ref(false)
const drafts = ref<Record<Method, Schedule>>({
  turnstate: { enabled: false, model: defaultModels.turnstate, interval_minutes: 60 },
  modeltrace: { enabled: false, model: defaultModels.modeltrace, interval_minutes: 60 },
})

async function load() {
  try {
    detail.value = await api.account(props.accountId)
    for (const m of methods)
      drafts.value[m] = { ...detail.value.account.methods[m].schedule }
  }
  catch (e) {
    error.value = (e as Error).message
  }
}

const method = computed(() => detail.value?.account.methods[tab.value])
const health = computed(() => detail.value?.account.methods.turnstate.health ?? null)
const tabs = methods.map(m => ({ label: methodLabel[m], value: m }))
const runFilters = [{ label: '全部', value: 'all' }, { label: '疑似和降智', value: 'abnormal' }, { label: '失败', value: 'failed' }]
const runs = computed<Summary[]>(() => (method.value?.runs ?? []).filter((r) => {
  if (runFilter.value === 'abnormal')
    return r.status === 'done' && (r.level === 'suspect' || r.level === 'degraded')
  if (runFilter.value === 'failed')
    return r.status === 'failed'
  return true
}))
const rates = computed(() => {
  const days = method.value?.daily ?? []
  const sum = (n: number): Counts => days.slice(-n).reduce<Counts>((c, d) => merge(c, d), {})
  return [
    { label: '今天', counts: sum(1) },
    { label: '7 天', counts: sum(7) },
    { label: '30 天', counts: sum(30) },
  ].map(r => ({ ...r, rate: { judged: judged(r.counts), ok: r.counts.ok ?? 0 } }))
})

const modelOptions = computed(() => {
  const ids = new Set<string>([...(props.models?.cpr ?? []), ...(props.models?.bank ?? []).map(m => m.id)])
  for (const m of methods) {
    if (drafts.value[m].model)
      ids.add(drafts.value[m].model)
  }
  return [...ids].sort().map(id => ({ label: id, value: id }))
})

// 改了就存，没有保存按钮
async function save(m: Method) {
  const s = drafts.value[m]
  if (s.enabled && !s.model) {
    saveError.value = '开启自动探测前先选择模型'
    s.enabled = false
    return
  }
  saving.value = m
  saveError.value = ''
  try {
    await api.schedule(props.accountId, m, s)
  }
  catch (e) {
    saveError.value = (e as Error).message
    await load()
  }
  finally {
    saving.value = null
  }
}

async function showRun(id: string) {
  try {
    openRun.value = (await api.run(id, props.accountId)).run
    runOpen.value = true
  }
  catch (e) {
    error.value = (e as Error).message
  }
}

const streakDots = computed(() => Array.from({ length: 3 }, (_, i) => i < (health.value?.streak ?? 0)))

onMounted(async () => {
  await load()
  if (props.initialRun)
    await showRun(props.initialRun)
})
</script>

<template>
  <div class="detail-page">
    <header class="detail-header">
      <ZButton variant="text" @click="emit('back')">
        <ArrowLeft class="size-4" aria-hidden="true" /> 返回列表
      </ZButton>
      <template v-if="detail">
        <div class="min-w-0">
          <h1 class="m-0 truncate text-lg font-semibold text-cp-text">{{ detail.account.email || detail.account.name }}</h1>
          <p class="m-0 mt-0.5 flex flex-wrap items-center gap-2 text-cp-xs text-cp-text-tertiary">
            <span v-if="detail.account.email && detail.account.name !== detail.account.email">{{ detail.account.name }}</span>
            <span v-if="detail.account.plan_type" class="account-plan">{{ detail.account.plan_type }}</span>
            <span v-if="!detail.account.enabled" class="text-cp-error">已停用</span>
            <span v-if="!detail.account.supported">不支持探测</span>
          </p>
        </div>
      </template>
    </header>
    <p v-if="error" class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-sm text-cp-error-on-container">{{ error }}</p>
    <p v-if="saveError" class="m-0 rounded-cp bg-cp-warning-container px-3 py-2 text-cp-sm text-cp-warning-on-container">{{ saveError }}</p>
    <div v-if="!detail && !error" class="monitor-loading">正在读取…</div>

    <template v-if="detail">
      <section class="verdict-panel" :data-state="health?.level ?? 'empty'" aria-live="polite">
        <div class="verdict-main">
          <span class="text-cp-xs text-cp-text-tertiary">测票当前判定</span>
          <span class="verdict-big" :data-state="health?.level ?? 'empty'"><i class="state-dot" :data-state="health?.level ?? 'empty'" />{{ health?.last_run_id ? levelLabel[health.level] : '还没有判定' }}</span>
          <span v-if="health?.changed_ms" class="text-cp-xs text-cp-text-secondary">{{ ago(health.changed_ms) }}确认 · {{ dateTime(health.changed_ms) }}</span>
        </div>
        <div v-if="health?.candidate && health.streak && health.candidate !== health.level" class="verdict-streak">
          <span class="text-cp-xs text-cp-text-tertiary">{{ health.candidate === 'ok' ? health.level === 'unknown' ? '正在确认正常' : '正在确认恢复' : `正在确认${levelLabel[health.candidate]}` }}</span>
          <span class="streak-dots" :aria-label="`已连续 ${health.streak} 次，满 3 次改变判定`"><i v-for="(on, i) in streakDots" :key="i" :data-on="on" :data-state="health.candidate" /></span>
          <span class="text-cp-xs text-cp-text-secondary">{{ health.streak }} / 3 次{{ levelLabel[health.candidate] }}</span>
        </div>
        <div v-else class="verdict-streak">
          <span class="text-cp-xs text-cp-text-tertiary">判定规则</span>
          <span class="text-cp-xs text-cp-text-secondary">连续 3 次正常才恢复；连续 3 次疑似或降智才转异常（其中有降智就判降智）。只有正常和异常之间切换才发 TG 通知</span>
        </div>
        <div class="verdict-notes">
          <span v-if="health?.model" class="text-cp-xs text-cp-text-tertiary">按 {{ health.model }} 判定</span>
          <span v-if="health?.notification_pending" class="text-cp-xs text-cp-text-secondary">TG 待发送 {{ health.notification_pending }} 条</span>
          <span v-if="health?.notification_error" class="text-cp-xs text-cp-warning-text">{{ health.notification_error }}</span>
          <span v-if="health?.notifications_dropped" class="text-cp-xs text-cp-warning-text">通知长期未送达，已丢弃 {{ health.notifications_dropped }} 条最旧通知</span>
        </div>
      </section>

      <section class="detail-card">
        <div class="detail-card-head">
          <ZSegmented v-model="tab" :options="tabs" aria-label="探测方法" />
          <span class="flex-1" />
          <ZButton size="small" type="primary" :disabled="!detail.account.supported" @click="dialog = true">立即探测</ZButton>
        </div>

        <div class="rate-tiles">
          <div v-for="r in rates" :key="r.label" class="rate-tile">
            <span class="text-cp-xs text-cp-text-tertiary">{{ r.label }}正常率</span>
            <strong :class="{ 'rate-empty': !r.rate.judged }">{{ rate(r.rate) }}</strong>
            <span class="text-cp-xs text-cp-text-tertiary">{{ r.rate.judged ? `${r.rate.ok} / ${r.rate.judged} 次` : '没有有效判定' }}</span>
          </div>
        </div>
        <div v-if="method" class="flex flex-col gap-1">
          <TrendChart :days="method.daily" />
        </div>

        <div class="detail-config">
          <template v-if="scheduledMethods.includes(tab)">
            <ZSwitch v-model="drafts[tab].enabled" label="自动探测" :disabled="saving === tab" @change="save(tab)" />
            <ZSelect v-model="drafts[tab].model" :options="modelOptions" filterable placeholder="模型" class="w-56" :disabled="saving === tab" aria-label="自动探测的模型" @change="save(tab)" />
            <span class="flex items-center gap-2 text-cp-sm">
              间隔
              <ZInputNumber v-model="drafts[tab].interval_minutes" :min="1" :max="10080" unit="分钟" class="w-36" :disabled="saving === tab" aria-label="自动探测间隔" @change="save(tab)" />
            </span>
            <span v-if="saving === tab" class="text-cp-xs text-cp-text-tertiary">保存中…</span>
          </template>
          <p v-else class="m-0 text-cp-xs text-cp-text-tertiary">ModelTrace 只支持手动探测，点“立即探测”在弹窗里选模型。</p>
          <p v-if="method?.last_attempt?.blockers?.length" class="m-0 w-full text-cp-xs text-cp-warning-text">
            上次自动探测 {{ dateTime(method.last_attempt.at_ms) }} 被拦截：{{ method.last_attempt.blockers.map(b => b.message).join('；') }}
          </p>
        </div>

        <div class="flex flex-col gap-2">
          <div class="flex flex-wrap items-center gap-3">
            <span class="text-cp-sm font-medium">探测记录</span>
            <ZSegmented v-model="runFilter" :options="runFilters" size="small" aria-label="筛选探测记录" />
            <span class="flex-1" />
            <span v-if="method?.raw_since" class="text-cp-xs text-cp-text-tertiary">逐条记录从 {{ dateTime(method.raw_since) }} 起，更早的只保留每天的次数</span>
          </div>
          <ZEmpty v-if="!runs.length" size="small" :title="method?.runs.length ? '没有符合筛选的记录' : '还没有探测记录'" />
          <ul v-else class="run-list">
            <li v-for="r in runs" :key="r.id">
              <button type="button" class="run-row" :disabled="!r.detail" :title="r.detail ? '查看详情' : '详情只保留最近 50 条'" @click="showRun(r.id)">
                <ZTag size="small" :type="r.status === 'failed' ? 'default' : levelTag(r.level)">{{ statusLabel(r.status, r.level) }}</ZTag>
                <span class="run-time">{{ dateTime(r.started_ms) }}</span>
                <span class="run-model">{{ r.model }}</span>
                <span class="run-reason">{{ r.reason }}</span>
                <span class="run-trigger">{{ r.trigger === 'auto' ? '自动' : '手动' }}</span>
              </button>
            </li>
          </ul>
        </div>
      </section>

      <ProbeDialog
        v-model="dialog"
        :account-id="accountId"
        :account-name="detail.account.email || detail.account.name"
        :method="tab"
        :saved-models="{ turnstate: drafts.turnstate.model, modeltrace: drafts.modeltrace.model }"
        :models="models"
        @done="load"
      />
    </template>
    <ZDialog v-model="runOpen" title="探测记录" width="44rem">
      <RunView v-if="openRun" :run="openRun" />
    </ZDialog>
  </div>
</template>
