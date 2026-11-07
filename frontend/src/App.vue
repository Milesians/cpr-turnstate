<script setup lang="ts">
import type { Models, Overview, OverviewAccount } from './api'
import { ZButton, ZConfigProvider, ZTag } from '@codex-proxy/ui'
import { zhHans } from '@codex-proxy/ui/locale'
import { Activity, RefreshCw, Settings } from '@lucide/vue'
import { computed, nextTick, onMounted, ref, shallowRef, watch } from 'vue'
import { api } from './api'
import AccountDetail from './components/AccountDetail.vue'
import AccountList from './components/AccountList.vue'
import BatchConfigDialog from './components/BatchConfigDialog.vue'
import BatchProbeDialog from './components/BatchProbeDialog.vue'
import ProbeDialog from './components/ProbeDialog.vue'
import SettingsPage from './components/SettingsPage.vue'
import { dateTime } from './format'
import { needsAttention } from './timeline'

type View = { name: 'list' } | { name: 'account', id: string, run?: string } | { name: 'settings' }

const view = ref<View>({ name: 'list' })
const overview = shallowRef<Overview | null>(null)
const models = shallowRef<Models | null>(null)
const loading = ref(false)
const error = ref('')
const notice = ref('')
const hours = ref('24')

const probeAccount = shallowRef<OverviewAccount | null>(null)
const probeOpen = ref(false)
const batchProbeAccounts = shallowRef<OverviewAccount[]>([])
const batchProbeOpen = ref(false)
const batchConfigIds = ref<string[]>([])
const batchConfigOpen = ref(false)

const attention = computed(() => overview.value?.accounts.filter(needsAttention).length ?? 0)

let loadId = 0
async function load() {
  const id = ++loadId
  loading.value = true
  error.value = ''
  try {
    const result = await api.overview(Number(hours.value))
    if (id === loadId) {
      overview.value = result
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

// 列表一直挂着（只是隐藏），返回时筛选、搜索、排序和选中都还在；滚动位置单独记下
let listScroll = 0
const app = () => document.querySelector<HTMLElement>('#app')
function leaveList(next: View) {
  if (view.value.name === 'list')
    listScroll = app()?.scrollTop ?? 0
  view.value = next
  nextTick(() => app()?.scrollTo({ top: 0 }))
}
function openAccount(id: string, run?: string) {
  leaveList({ name: 'account', id, run })
}
function back() {
  view.value = { name: 'list' }
  nextTick(() => app()?.scrollTo({ top: listScroll }))
  load()
}
function probe(account: OverviewAccount) {
  probeAccount.value = account
  probeOpen.value = true
}
function batchProbe(accounts: OverviewAccount[]) {
  batchProbeAccounts.value = accounts
  batchProbeOpen.value = true
}
function batchConfig(ids: string[]) {
  batchConfigIds.value = ids
  batchConfigOpen.value = true
}
function batchConfigDone(count: number) {
  notice.value = `已更新 ${count} 个账号的测票配置`
  load()
}

watch(hours, load)
let noticeTimer: ReturnType<typeof setTimeout> | undefined
watch(notice, (value) => {
  clearTimeout(noticeTimer)
  if (value)
    noticeTimer = setTimeout(() => { notice.value = '' }, 5000)
})
onMounted(() => Promise.all([load(), loadModels()]))
</script>

<template>
  <ZConfigProvider :locale="zhHans">
    <main class="monitor-page font-sans text-cp-text">
      <AccountDetail v-if="view.name === 'account'" :key="view.id" :account-id="view.id" :initial-run="view.run" :models="models" @back="back" />
      <SettingsPage v-else-if="view.name === 'settings'" :overview="overview" @back="back" @changed="() => { load(); loadModels() }" />
      <div v-show="view.name === 'list'">
        <header class="monitor-header">
          <div class="flex min-w-0 items-center gap-3">
            <span class="monitor-logo"><Activity :size="22" aria-hidden="true" /></span>
            <div class="min-w-0">
              <h1 class="m-0 text-xl font-semibold tracking-tight text-cp-text">账号探测</h1>
              <p class="m-0 mt-1 text-cp-xs text-cp-text-tertiary">
                <template v-if="overview">{{ attention ? `${attention} 个账号需要关注` : '没有异常账号' }} · 更新于 {{ dateTime(overview.now_ms) }}</template>
                <template v-else>OpenAI OAuth · 测票与 ModelTrace</template>
              </p>
            </div>
          </div>
          <div class="flex items-center gap-2">
            <ZTag v-if="overview && !overview.config.probe_enabled" type="warning" size="small" title="在 CPR 插件详情的设置里开启">真实探测未开启</ZTag>
            <ZTag v-if="overview?.config.telegram_enabled && !overview.config.telegram_configured" type="warning" size="small">TG 配置不完整</ZTag>
            <ZButton size="small" :loading="loading" aria-label="刷新" @click="load"><RefreshCw :size="14" aria-hidden="true" /> 刷新</ZButton>
            <ZButton size="small" aria-label="设置" @click="leaveList({ name: 'settings' })"><Settings :size="14" aria-hidden="true" /> 设置</ZButton>
          </div>
        </header>
        <p v-if="error" class="banner banner-error" role="alert">{{ error }}</p>
        <p v-if="notice" class="banner banner-ok" role="status">{{ notice }}</p>
        <AccountList
          v-model:hours="hours" :overview="overview" :loading="loading"
          @open="openAccount" @probe="probe" @batch-probe="batchProbe" @batch-config="batchConfig"
        />
      </div>

      <ProbeDialog
        v-if="probeAccount" v-model="probeOpen" :account-id="probeAccount.id" :account-name="probeAccount.email || probeAccount.name" method="turnstate"
        :saved-models="{ turnstate: probeAccount.schedule.turnstate.model, modeltrace: probeAccount.schedule.modeltrace.model }" :models="models" @done="load"
      />
      <BatchProbeDialog v-model="batchProbeOpen" :accounts="batchProbeAccounts" :models="models" @done="load" />
      <BatchConfigDialog v-model="batchConfigOpen" :account-ids="batchConfigIds" :models="models" @done="batchConfigDone" />
    </main>
  </ZConfigProvider>
</template>
