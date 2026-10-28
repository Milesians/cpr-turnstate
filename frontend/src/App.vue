<script setup lang="ts">
import type { Method, Models, Overview, OverviewAccount } from './api'
import { ZButton, ZCard, ZConfigProvider, ZEmpty, ZTag } from '@codex-proxy/ui'
import { zhHans } from '@codex-proxy/ui/locale'
import { RefreshCw } from '@lucide/vue'
import { onMounted, ref, shallowRef } from 'vue'
import { api, methodLabel, methods } from './api'
import AccountDetail from './components/AccountDetail.vue'
import ProbeDialog from './components/ProbeDialog.vue'
import ProbeStrip from './components/ProbeStrip.vue'
import { dateTime, levelBar, levelLabel, levelTag, rate } from './format'

const overview = shallowRef<Overview | null>(null)
const models = shallowRef<Models | null>(null)
const loading = ref(false)
const error = ref('')
const syncing = ref(false)
const selected = ref<{ id: string, run?: string } | null>(null)
const dialog = ref(false)
const dialogAccount = shallowRef<OverviewAccount | null>(null)

async function load() {
  loading.value = true
  error.value = ''
  try {
    overview.value = await api.overview()
  }
  catch (e) {
    error.value = (e as Error).message
  }
  finally {
    loading.value = false
  }
}

async function loadModels() {
  try {
    models.value = await api.models()
  }
  catch {
    models.value = null
  }
}

async function syncBank() {
  syncing.value = true
  try {
    await api.syncBank()
    await Promise.all([load(), loadModels()])
  }
  finally {
    syncing.value = false
  }
}

function probe(account: OverviewAccount) {
  dialogAccount.value = account
  dialog.value = true
}

function defaultMethod(a: OverviewAccount): Method {
  return a.methods.turnstate.schedule.model || !a.methods.modeltrace.schedule.model ? 'turnstate' : 'modeltrace'
}

function worst(a: OverviewAccount) {
  const order = ['unknown', 'ok', 'suspect', 'degraded'] as const
  let level: (typeof order)[number] | null = null
  for (const m of methods) {
    const l = a.methods[m].latest?.status === 'done' ? a.methods[m].latest?.level ?? 'unknown' : null
    if (l && (level === null || order.indexOf(l) > order.indexOf(level)))
      level = l
  }
  return level
}

onMounted(() => Promise.all([load(), loadModels()]))
</script>

<template>
  <ZConfigProvider :locale="zhHans">
    <main class="flex min-w-0 flex-col gap-4 font-sans text-cp-text" style="min-height: inherit">
      <AccountDetail
        v-if="selected"
        :account-id="selected.id"
        :initial-run="selected.run"
        :models="models"
        @back="selected = null; load()"
      />
      <template v-else>
        <ZCard padding="compact">
          <div class="flex flex-wrap items-center gap-x-4 gap-y-2 text-cp-sm">
            <ZTag :type="overview?.config.probe_enabled ? 'success' : 'warning'">
              {{ overview?.config.probe_enabled ? '真实探测已开启' : '真实探测未开启' }}
            </ZTag>
            <span v-if="overview" class="text-cp-text-secondary">
              自动探测最短间隔 {{ overview.config.min_interval_minutes }} 分钟
            </span>
            <span v-if="overview" class="text-cp-text-secondary">
              指纹库 {{ overview.bank.commit.slice(0, 10) }}（{{ overview.bank.source === 'github' ? 'GitHub' : '内置' }}，{{ overview.bank.models.length }} 个模型）
              <template v-if="overview.bank.checked_ms">· 检查于 {{ dateTime(overview.bank.checked_ms) }}</template>
            </span>
            <span v-if="overview?.bank.error" class="text-cp-warning-text">{{ overview.bank.error }}</span>
            <span class="flex-1" />
            <ZButton size="small" :loading="syncing" @click="syncBank">
              同步指纹库
            </ZButton>
            <ZButton size="small" :loading="loading" @click="load">
              <RefreshCw class="size-4" aria-hidden="true" /> 刷新
            </ZButton>
          </div>
          <p v-if="overview && !overview.config.probe_enabled" class="m-0 mt-2 text-cp-xs text-cp-text-tertiary">
            在插件设置里打开“发出真实探测”后才会发送请求。探测会消耗账号的真实额度。
          </p>
        </ZCard>

        <p v-if="error" class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-sm text-cp-error-on-container">
          {{ error }}
        </p>

        <div class="flex flex-wrap items-center gap-4 text-cp-xs text-cp-text-secondary">
          <span v-for="l in (['ok', 'suspect', 'degraded', 'unknown'] as const)" :key="l" class="flex items-center gap-1.5">
            <span class="inline-block h-3 w-1.5 rounded-[2px]" :class="levelBar(l)" />{{ levelLabel[l] }}
          </span>
          <span class="flex items-center gap-1.5">
            <span class="inline-block h-3 w-1.5 rounded-[2px]" :class="levelBar(null)" />没有探测
          </span>
          <span>每格一小时，最近 24 小时；末尾是 24 小时正常率</span>
        </div>

        <ZEmpty v-if="overview && !overview.accounts.length" title="CPR 里还没有 OpenAI 账号" />
        <ZCard v-for="a in overview?.accounts ?? []" :key="a.id" padding="compact">
          <div class="flex flex-col gap-3 lg:flex-row lg:items-center">
            <button type="button" class="flex w-full min-w-0 flex-col items-start gap-1 text-left lg:w-64 lg:shrink-0" @click="selected = { id: a.id }">
              <span class="max-w-full truncate font-medium text-cp-text-heading hover:underline">{{ a.email || a.name }}</span>
              <span class="flex flex-wrap items-center gap-1">
                <span v-if="a.email" class="max-w-40 truncate text-cp-xs text-cp-text-tertiary">{{ a.name }}</span>
                <ZTag v-if="a.plan_type" size="small">{{ a.plan_type }}</ZTag>
                <ZTag v-if="!a.enabled" size="small" type="danger">已停用</ZTag>
                <ZTag v-if="!a.supported" size="small">不支持</ZTag>
                <ZTag v-if="a.running" size="small" type="info">探测中</ZTag>
                <ZTag v-if="worst(a)" size="small" :type="levelTag(worst(a))">{{ levelLabel[worst(a)!] }}</ZTag>
              </span>
            </button>
            <div class="flex min-w-0 flex-1 flex-col gap-1.5">
              <div v-for="m in methods" :key="m" class="flex min-w-0 items-center gap-2">
                <span class="w-20 shrink-0 text-cp-xs text-cp-text-secondary">{{ methodLabel[m] }}</span>
                <ProbeStrip :slots="a.methods[m].slots" @open="run => selected = { id: a.id, run }" />
                <span class="w-10 shrink-0 text-right text-cp-xs tabular-nums text-cp-text-secondary" :title="`7 天 ${rate(a.methods[m].rate_7d)}`">
                  {{ rate(a.methods[m].rate_24h) }}
                </span>
              </div>
            </div>
            <ZButton size="small" :disabled="!a.supported" @click="probe(a)">
              探测
            </ZButton>
          </div>
        </ZCard>

        <ProbeDialog
          v-if="dialogAccount"
          v-model="dialog"
          :account-id="dialogAccount.id"
          :account-name="dialogAccount.email || dialogAccount.name"
          :method="defaultMethod(dialogAccount)"
          :model="dialogAccount.methods[defaultMethod(dialogAccount)].schedule.model"
          :models="models"
          @done="load"
        />
      </template>
    </main>
  </ZConfigProvider>
</template>
