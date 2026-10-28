<script setup lang="ts">
import type { AccountDetail, Method, Models, Run, Schedule } from '../api'
import { ZButton, ZCard, ZDialog, ZEmpty, ZInputNumber, ZSelect, ZSwitch, ZTag } from '@codex-proxy/ui'
import { ArrowLeft } from '@lucide/vue'
import { computed, onMounted, ref, shallowRef } from 'vue'
import { api, methodLabel, methods } from '../api'
import { dateTime, levelTag, statusLabel } from '../format'
import ProbeDialog from './ProbeDialog.vue'
import RunView from './RunView.vue'

const props = defineProps<{ accountId: string, models: Models | null, initialRun?: string }>()
const emit = defineEmits<{ back: [] }>()

const detail = shallowRef<AccountDetail | null>(null)
const error = ref('')
const saving = ref<Method | null>(null)
const saveError = ref('')
const dialog = ref(false)
const dialogMethod = ref<Method>('turnstate')
const openRun = shallowRef<Run | null>(null)
const runOpen = ref(false)
const drafts = ref<Record<Method, Schedule>>({
  turnstate: { enabled: false, model: '', interval_minutes: 60 },
  modeltrace: { enabled: false, model: '', interval_minutes: 60 },
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

const modelOptions = computed(() => {
  const ids = new Set<string>([...(props.models?.cpr ?? []), ...(props.models?.bank ?? []).map(m => m.id)])
  for (const m of methods) {
    if (drafts.value[m].model)
      ids.add(drafts.value[m].model)
  }
  return [...ids].sort().map(id => ({ label: id, value: id }))
})

// 改了就存，没有保存按钮
async function save(method: Method) {
  const s = drafts.value[method]
  if (s.enabled && !s.model) {
    saveError.value = '开启自动探测前先选择模型'
    s.enabled = false
    return
  }
  saving.value = method
  saveError.value = ''
  try {
    await api.schedule(props.accountId, method, s)
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

function probe(method: Method) {
  dialogMethod.value = method
  dialog.value = true
}

onMounted(async () => {
  await load()
  if (props.initialRun)
    await showRun(props.initialRun)
})
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex items-center gap-3">
      <ZButton variant="text" @click="emit('back')">
        <ArrowLeft class="size-4" aria-hidden="true" /> 返回
      </ZButton>
      <template v-if="detail">
        <span class="text-cp-lg font-semibold text-cp-text-heading">{{ detail.account.name }}</span>
        <span class="text-cp-text-secondary">{{ detail.account.email }}</span>
        <ZTag v-if="detail.account.plan_type" size="small">
          {{ detail.account.plan_type }}
        </ZTag>
        <ZTag v-if="!detail.account.enabled" size="small" type="danger">
          已停用
        </ZTag>
      </template>
    </div>
    <p v-if="error" class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-sm text-cp-error-on-container">
      {{ error }}
    </p>
    <p v-if="saveError" class="m-0 rounded-cp bg-cp-warning-container px-3 py-2 text-cp-sm text-cp-warning-on-container">
      {{ saveError }}
    </p>
    <template v-if="detail">
      <ZCard v-for="m in methods" :key="m" :title="methodLabel[m]" padding="compact">
        <template #actions>
          <ZButton size="small" type="primary" :disabled="!detail.account.supported" @click="probe(m)">
            立即探测
          </ZButton>
        </template>
        <div class="flex flex-col gap-3 text-cp-sm">
          <div class="flex flex-wrap items-center gap-3">
            <ZSwitch v-model="drafts[m].enabled" label="自动探测" :disabled="saving === m" @change="save(m)" />
            <ZSelect v-model="drafts[m].model" :options="modelOptions" filterable placeholder="模型" class="w-56" :disabled="saving === m" @change="save(m)" />
            <span class="flex items-center gap-2">
              间隔
              <ZInputNumber v-model="drafts[m].interval_minutes" :min="1" :max="10080" unit="分钟" class="w-36" :disabled="saving === m" @change="save(m)" />
            </span>
          </div>
          <div v-if="detail.account.methods[m].last_attempt?.blockers?.length" class="text-cp-xs text-cp-text-tertiary">
            上次自动探测 {{ dateTime(detail.account.methods[m].last_attempt!.at_ms) }} 被拦截：{{ detail.account.methods[m].last_attempt!.blockers!.map(b => b.message).join('；') }}
          </div>
          <ZEmpty v-if="!detail.account.methods[m].runs.length" size="small" title="还没有探测记录" />
          <ul v-else class="m-0 flex max-h-96 list-none flex-col overflow-auto p-0">
            <li v-for="r in detail.account.methods[m].runs" :key="r.id" class="border-t border-cp-border-secondary first:border-t-0">
              <button
                type="button"
                class="flex w-full items-start gap-3 px-1 py-2 text-left hover:bg-cp-fill-quaternary disabled:cursor-default disabled:hover:bg-transparent"
                :disabled="!r.detail"
                @click="showRun(r.id)"
              >
                <ZTag size="small" :type="r.status === 'failed' ? 'default' : levelTag(r.level)">
                  {{ statusLabel(r.status, r.level) }}
                </ZTag>
                <span class="w-32 shrink-0 tabular-nums text-cp-text-secondary">{{ dateTime(r.started_ms) }}</span>
                <span class="w-36 shrink-0 truncate">{{ r.model }}</span>
                <span class="min-w-0 flex-1 text-cp-text-secondary">{{ r.reason }}</span>
                <span class="shrink-0 text-cp-xs text-cp-text-tertiary">{{ r.trigger === 'auto' ? '自动' : '手动' }}</span>
              </button>
            </li>
          </ul>
        </div>
      </ZCard>
      <ProbeDialog
        v-model="dialog"
        :account-id="accountId"
        :account-name="detail.account.name"
        :method="dialogMethod"
        :model="drafts[dialogMethod].model"
        :models="models"
        @done="load"
      />
    </template>
    <ZDialog v-model="runOpen" title="探测记录" width="44rem">
      <RunView v-if="openRun" :run="openRun" />
    </ZDialog>
  </div>
</template>
