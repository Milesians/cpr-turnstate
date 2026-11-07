<script setup lang="ts">
import type { Method, Models, OverviewAccount, Run } from '../api'
import { ZButton, ZDialog, ZSegmented, ZSelect } from '@codex-proxy/ui'
import { computed, ref, watch } from 'vue'
import { ApiError, methodLabel, methods, runProbe } from '../api'
import { statusLabel } from '../format'
import { defaultModels, probeModels } from '../probe-models'

/** 选中的账号逐个发起立即探测：同时最多 3 个，插件自己还会限制同账号串行、全局 8 个 */
const props = defineProps<{ accounts: OverviewAccount[], models: Models | null }>()
const emit = defineEmits<{ done: [] }>()
const open = defineModel<boolean>({ default: false })

const CONCURRENCY = 3
type Item = { account: OverviewAccount, state: 'queued' | 'running' | 'done' | 'failed' | 'skipped', run?: Run, message?: string }
const method = ref<Method>('turnstate')
/** 空表示用各账号已选的模型（没选过用默认模型） */
const model = ref('')
const modelOptions = computed(() => {
  const ids = new Set([defaultModels[method.value], ...(props.models?.cpr ?? []), ...(props.models?.bank ?? []).map(m => m.id)])
  return [{ label: method.value === 'turnstate' ? '各账号已选的模型' : `默认（${defaultModels.modeltrace}）`, value: '' }, ...[...ids].sort().map(value => ({ label: value, value }))]
})
watch(method, () => { model.value = '' })
const items = ref<Item[]>([])
const running = ref(false)
let cancelled = false

const methodOptions = methods.map(m => ({ label: methodLabel[m], value: m }))
const finished = computed(() => items.value.filter(i => i.state !== 'queued' && i.state !== 'running').length)
const summary = computed(() => {
  const by = (l: string) => items.value.filter(i => i.run?.status === 'done' && i.run.verdict?.level === l).length
  return { ok: by('ok'), suspect: by('suspect'), degraded: by('degraded'), failed: items.value.filter(i => i.state === 'failed').length }
})

watch(open, (v) => {
  if (v && !running.value) {
    method.value = 'turnstate'
    model.value = ''
    items.value = props.accounts.map(account => ({ account, state: 'queued' }))
  }
})

async function probeOne(item: Item) {
  item.state = 'running'
  const chosen = model.value || probeModels({ turnstate: item.account.schedule.turnstate.model, modeltrace: item.account.schedule.modeltrace.model })[method.value]
  try {
    item.run = await runProbe(item.account.id, method.value, chosen, (r) => { item.run = r })
    item.state = item.run.status === 'failed' ? 'failed' : 'done'
    item.message = item.run.error?.message ?? item.run.verdict?.reason
  }
  catch (e) {
    item.state = e instanceof ApiError && e.blockers.length ? 'skipped' : 'failed'
    item.message = (e as Error).message
  }
}

async function start() {
  running.value = true
  cancelled = false
  for (const i of items.value) {
    if (i.state !== 'queued') {
      i.state = 'queued'
      i.run = undefined
      i.message = undefined
    }
  }
  const queue = [...items.value]
  const worker = async () => {
    while (!cancelled) {
      const next = queue.shift()
      if (!next)
        return
      await probeOne(next)
    }
  }
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, queue.length) }, worker))
  running.value = false
  emit('done')
}

function stop() {
  cancelled = true
}

function itemLabel(i: Item) {
  if (i.state === 'queued')
    return cancelled && !running.value ? '已取消' : '等待中'
  if (i.state === 'running')
    return '探测中'
  if (i.state === 'skipped')
    return '被拦截'
  return i.run ? statusLabel(i.run.status, i.run.verdict?.level) : '失败'
}
function itemState(i: Item) {
  if (i.state === 'running')
    return 'running'
  if (i.state === 'queued')
    return 'empty'
  if (i.state === 'done' && i.run?.verdict)
    return i.run.verdict.level
  return i.state === 'failed' ? 'failed' : 'unknown'
}
</script>

<template>
  <ZDialog v-model="open" title="批量探测" width="40rem" :close-on-click-modal="!running" :close-on-press-escape="!running" :before-close="() => !running">
    <div class="flex flex-col gap-4 text-cp-sm">
      <div class="flex flex-wrap items-center gap-3">
        <ZSegmented v-model="method" :options="methodOptions" size="small" :disabled="running || finished > 0" aria-label="批量探测的方法" />
        <ZSelect v-model="model" :options="modelOptions" filterable class="min-w-48 flex-1" :disabled="running" aria-label="批量探测的模型" />
        <span class="w-full text-cp-xs text-cp-text-tertiary">同时探测 {{ CONCURRENCY }} 个；只探测，不修改账号的自动探测配置</span>
      </div>
      <p v-if="method === 'modeltrace'" class="m-0 text-cp-xs text-cp-text-secondary">ModelTrace 每个账号发 3 道题，用插件设置里配置的 Client Key，计入它的用量。</p>
      <div v-if="finished" class="flex flex-wrap gap-3 text-cp-xs text-cp-text-secondary" role="status">
        <span>已完成 {{ finished }} / {{ items.length }}</span>
        <span v-if="summary.ok">正常 {{ summary.ok }}</span>
        <span v-if="summary.suspect" class="text-cp-warning-text">疑似 {{ summary.suspect }}</span>
        <span v-if="summary.degraded" class="text-cp-error">降智 {{ summary.degraded }}</span>
        <span v-if="summary.failed">失败 {{ summary.failed }}</span>
      </div>
      <ul class="batch-list">
        <li v-for="i in items" :key="i.account.id" class="batch-item">
          <span class="state-dot" :data-state="itemState(i)" />
          <span class="min-w-0 flex-1">
            <span class="block truncate text-cp-text">{{ i.account.email || i.account.name }}</span>
            <span v-if="i.message" class="block truncate text-cp-xs text-cp-text-tertiary" :title="i.message">{{ i.message }}</span>
          </span>
          <span class="shrink-0 text-cp-xs text-cp-text-secondary">{{ itemLabel(i) }}</span>
        </li>
      </ul>
    </div>
    <template #footer>
      <ZButton v-if="running" @click="stop">停止排队</ZButton>
      <ZButton v-else @click="open = false">关闭</ZButton>
      <ZButton type="primary" :loading="running" :disabled="!items.length" @click="start">{{ finished ? '重新探测' : `开始探测 ${items.length} 个账号` }}</ZButton>
    </template>
  </ZDialog>
</template>
