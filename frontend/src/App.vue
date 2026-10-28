<script setup lang="ts">
import type { Method, Models, Overview, OverviewAccount } from './api'
import { ZButton, ZCheckbox, ZConfigProvider, ZEmpty, ZInput, ZSegmented, ZTag } from '@codex-proxy/ui'
import { zhHans } from '@codex-proxy/ui/locale'
import { Activity, RefreshCw, Search, Settings2 } from '@lucide/vue'
import { computed, onMounted, ref, shallowRef, watch } from 'vue'
import { api, methodLabel, methods } from './api'
import AccountDetail from './components/AccountDetail.vue'
import BatchConfigDialog from './components/BatchConfigDialog.vue'
import ProbeDialog from './components/ProbeDialog.vue'
import ProbeStrip from './components/ProbeStrip.vue'
import { dateTime, rate } from './format'
import { accountState, stateLabel } from './timeline'

const overview = shallowRef<Overview | null>(null)
const models = shallowRef<Models | null>(null)
const loading = ref(false)
const error = ref('')
const syncing = ref(false)
const selected = ref<{ id: string, run?: string } | null>(null)
const dialog = ref(false)
const dialogAccount = shallowRef<OverviewAccount | null>(null)
const hours = ref('24')
const search = ref('')
const selectedIds = ref<string[]>([])
const batchOpen = ref(false)
const notice = ref('')
const selectable = computed(() => accounts.value.filter(a => a.supported).map(a => a.id))
const allSelected = computed({
  get: () => !!selectable.value.length && selectable.value.every(id => selectedIds.value.includes(id)),
  set: value => {
    const visible = new Set(selectable.value)
    selectedIds.value = value
      ? [...new Set([...selectedIds.value, ...selectable.value])].slice(0, 200)
      : selectedIds.value.filter(id => !visible.has(id))
  },
})
function selectAccount(id: string, value: boolean) {
  selectedIds.value = value ? [...new Set([...selectedIds.value, id])].slice(0, 200) : selectedIds.value.filter(s => s !== id)
}
function batchDone(count: number, method: Method) {
  notice.value = `已更新 ${count} 个账号的${methodLabel[method]}配置`
  load()
}
const ranges = [{ label: '24 小时', value: '24' }, { label: '7 天', value: '168' }, { label: '30 天', value: '720' }]
const rangeLabel = computed(() => ranges.find(r => r.value === hours.value)!.label)
const accounts = computed(() => (overview.value?.accounts ?? []).filter(a => `${a.email ?? ''} ${a.name}`.toLowerCase().includes(search.value.trim().toLowerCase())))
const totals = computed(() => {
  const states = (overview.value?.accounts ?? []).map(accountState)
  return [
    { state: 'ok', label: '正常', count: states.filter(s => s === 'ok').length },
    { state: 'suspect', label: '需要关注', count: states.filter(s => s === 'suspect' || s === 'degraded').length },
    { state: 'unknown', label: '未判定', count: states.filter(s => s === 'unknown' || s === 'running').length },
    { state: 'empty', label: '没有探测', count: states.filter(s => s === 'empty').length },
  ]
})
let loadId = 0
async function load() {
  const id = ++loadId
  loading.value = true
  error.value = ''
  try {
    const result = await api.overview(Number(hours.value))
    if (id === loadId) {
      overview.value = result
      selectedIds.value = selectedIds.value.filter(id => result.accounts.some(a => a.id === id && a.supported))
    }
  }
  catch (e) {
    if (id === loadId)
      error.value = (e as Error).message
  }
  finally {
    if (id === loadId)
      loading.value = false
  }
}
async function loadModels() {
  try { models.value = await api.models() }
  catch { models.value = null }
}
async function syncBank() {
  syncing.value = true
  try {
    await api.syncBank()
    await Promise.all([load(), loadModels()])
  }
  catch (e) { error.value = (e as Error).message }
  finally { syncing.value = false }
}
function probe(account: OverviewAccount) {
  dialogAccount.value = account
  dialog.value = true
}
function defaultMethod(a: OverviewAccount): Method {
  return a.methods.modeltrace.schedule.enabled && !a.methods.turnstate.schedule.enabled ? 'modeltrace' : 'turnstate'
}
watch(hours, () => {
  overview.value = null
  load()
})
onMounted(() => Promise.all([load(), loadModels()]))
</script>

<template>
  <ZConfigProvider :locale="zhHans">
    <main class="monitor-page font-sans text-cp-text">
      <AccountDetail v-if="selected" :account-id="selected.id" :initial-run="selected.run" :models="models" @back="selected = null; load()" />
      <template v-else>
        <header class="monitor-header">
          <div class="flex min-w-0 items-center gap-3">
            <span class="monitor-logo"><Activity :size="22" aria-hidden="true" /></span>
            <div>
              <h1 class="m-0 text-xl font-semibold tracking-tight text-cp-text">账号探测</h1>
              <p class="m-0 mt-1 text-cp-xs text-cp-text-tertiary">OpenAI OAuth · ModelTrace &amp; 测票</p>
            </div>
          </div>
          <div class="flex items-center gap-2">
            <ZTag :type="overview?.config.probe_enabled ? 'success' : 'default'" size="small">{{ overview?.config.probe_enabled ? '真实探测已开启' : '真实探测未开启' }}</ZTag>
            <ZButton size="small" :loading="loading" aria-label="刷新账号" @click="load"><RefreshCw :size="14" aria-hidden="true" /> 刷新</ZButton>
          </div>
        </header>

        <section class="monitor-summary" aria-label="所选时间段的账号状态">
          <div v-for="item in totals" :key="item.state" class="summary-item">
            <span class="state-dot" :data-state="item.state" />
            <span class="text-cp-xs text-cp-text-secondary">{{ item.label }}</span>
            <strong class="summary-number">{{ overview ? item.count : '—' }}</strong>
          </div>
        </section>

        <section class="monitor-list" :aria-busy="loading">
          <div class="monitor-toolbar">
            <ZInput v-model="search" placeholder="搜索名称或邮箱" clearable class="monitor-search" aria-label="搜索账号"><template #prefix><Search :size="15" aria-hidden="true" /></template></ZInput>
            <ZSegmented v-model="hours" :options="ranges" size="small" aria-label="时间段筛选" />
          </div>
          <div class="monitor-selection">
            <ZCheckbox v-model="allSelected" :indeterminate="!allSelected && selectedIds.length > 0" :disabled="!selectable.length" label="全选当前筛选" />
            <span class="text-cp-xs text-cp-text-tertiary">已选 {{ selectedIds.length }} 个<span v-if="selectedIds.length >= 200">（单批上限 200）</span></span>
            <ZButton size="small" :disabled="!selectedIds.length" @click="batchOpen = true">批量配置</ZButton>
            <button v-if="selectedIds.length" type="button" class="text-cp-xs text-cp-text-secondary" @click="selectedIds = []">清空选择</button>
          </div>
          <p v-if="notice" class="mx-4 text-cp-xs text-cp-success" role="status">{{ notice }}</p>
          <div class="monitor-axis"><span>{{ accounts.length }} 个账号</span><span>最近 {{ rangeLabel }} <span class="mx-1">→</span> 现在</span></div>
          <p v-if="error" class="mx-4 rounded-cp bg-cp-error-container px-3 py-2 text-cp-sm text-cp-error-on-container">{{ error }}</p>
          <div v-if="loading && !overview" class="monitor-loading">正在读取探测记录…</div>
          <ZEmpty v-else-if="!accounts.length" class="py-12" :title="search ? '没有匹配的账号' : 'CPR 里还没有 OpenAI 账号'" />
          <article v-for="a in accounts" :key="a.id" class="monitor-row" :data-account="a.id">
            <div class="monitor-account">
              <ZCheckbox :model-value="selectedIds.includes(a.id)" :disabled="!a.supported || (selectedIds.length >= 200 && !selectedIds.includes(a.id))" :aria-label="`选择 ${a.email || a.name}`" @update:model-value="value => selectAccount(a.id, value)" />
              <span class="state-dot account-dot" :data-state="accountState(a)" />
              <button type="button" class="min-w-0 flex-1 text-left" @click="selected = { id: a.id }">
                <span class="block truncate text-cp-sm font-semibold text-cp-text hover:underline">{{ a.email || a.name }}</span>
                <span class="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-cp-xs text-cp-text-tertiary">
                  <span v-if="a.email && a.name !== a.email" class="max-w-32 truncate">{{ a.name }}</span>
                  <span v-if="a.plan_type" class="account-plan">{{ a.plan_type }}</span>
                  <span :data-state="accountState(a)" class="account-state">{{ stateLabel[accountState(a)] }}</span>
                  <span v-if="!a.enabled">已停用</span><span v-if="!a.supported">不支持</span>
                </span>
              </button>
            </div>
            <div class="monitor-tracks">
              <div v-for="m in methods" :key="m" class="monitor-track">
                <span class="track-label">{{ methodLabel[m] }}</span>
                <ProbeStrip :slots="a.methods[m].slots" @open="run => selected = { id: a.id, run }" />
                <span class="track-rate" :class="{ 'rate-empty': !a.methods[m].rate.judged }" :title="`${rangeLabel}内 ${a.methods[m].rate.ok} / ${a.methods[m].rate.judged} 次有效判定为正常；失败、未判定和无探测不计入`">{{ rate(a.methods[m].rate) }}</span>
              </div>
            </div>
            <div class="monitor-actions">
              <ZButton size="small" :disabled="!a.supported" @click="probe(a)">探测</ZButton>
              <button type="button" class="account-details" :aria-label="`配置 ${a.email || a.name}`" title="按账号配置" @click="selected = { id: a.id }"><Settings2 :size="17" aria-hidden="true" /></button>
            </div>
          </article>
          <footer class="monitor-legend">
            <span v-for="s in (['ok', 'suspect', 'degraded', 'unknown', 'empty', 'running'] as const)" :key="s" class="inline-flex items-center gap-1.5"><i class="legend-cell" :data-state="s" />{{ stateLabel[s] }}</span>
            <span class="legend-note">空白与未判定不计入正常率 · 悬停或点按查看记录</span>
          </footer>
        </section>

        <footer class="monitor-footer">
          <span v-if="overview">指纹库 {{ overview.bank.commit.slice(0, 10) }} · {{ overview.bank.models.length }} 个模型<span v-if="overview.bank.checked_ms"> · {{ dateTime(overview.bank.checked_ms) }} 检查</span></span>
          <button type="button" class="sync-link" :disabled="syncing" @click="syncBank"><RefreshCw :size="12" :class="{ 'animate-spin': syncing }" aria-hidden="true" />{{ syncing ? '同步中…' : '同步指纹库' }}</button>
          <span v-if="overview?.bank.error" class="w-full text-cp-warning-text">{{ overview.bank.error }}</span>
        </footer>
        <BatchConfigDialog v-model="batchOpen" :account-ids="selectedIds" :models="models" @done="batchDone" />
        <ProbeDialog v-if="dialogAccount" v-model="dialog" :account-id="dialogAccount.id" :account-name="dialogAccount.email || dialogAccount.name" :method="defaultMethod(dialogAccount)" :saved-models="{ turnstate: dialogAccount.methods.turnstate.schedule.model, modeltrace: dialogAccount.methods.modeltrace.schedule.model }" :models="models" @done="load" />
      </template>
    </main>
  </ZConfigProvider>
</template>
