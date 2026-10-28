<script setup lang="ts">
import type { Blocker, Method, Models, Plan, Run } from '../api'
import { ZButton, ZDialog, ZSegmented, ZSelect } from '@codex-proxy/ui'
import { computed, ref, shallowRef, watch } from 'vue'
import { api, ApiError, methodLabel, methods, runProbe } from '../api'
import { probeModels } from '../probe-models'
import RunView from './RunView.vue'

const props = defineProps<{
  accountId: string
  accountName: string
  method: Method
  savedModels: Record<Method, string>
  models: Models | null
}>()
const emit = defineEmits<{ done: [] }>()
const open = defineModel<boolean>({ default: false })

const method = ref<Method>(props.method)
const choices = ref(probeModels(props.savedModels))
const model = computed({
  get: () => choices.value[method.value],
  set: value => { choices.value[method.value] = value },
})
const plan = shallowRef<Plan | null>(null)
const showRequest = ref(false)
const running = ref(false)
const run = shallowRef<Run | null>(null)
const error = ref('')
const blockers = shallowRef<Blocker[]>([])

const methodOptions = methods.map(m => ({ label: methodLabel[m], value: m }))
const modelOptions = computed(() => {
  const ids = new Set<string>()
  for (const id of props.models?.cpr ?? [])
    ids.add(id)
  for (const m of props.models?.bank ?? [])
    ids.add(m.id)
  if (model.value)
    ids.add(model.value)
  const inBank = new Set((props.models?.bank ?? []).map(m => m.id))
  const aliases = props.models?.aliases ?? {}
  return [...ids].sort().map(id => ({
    label: id,
    value: id,
    description: inBank.has(id) ? '指纹库有' : aliases[id] ? `按 ${aliases[id]} 的指纹` : undefined,
  }))
})

watch(open, (v) => {
  if (v) {
    method.value = props.method
    choices.value = probeModels(props.savedModels)
    run.value = null
    error.value = ''
    blockers.value = []
    showRequest.value = false
  }
})

// 切换方法或模型只重新预览（不发送），不自动探测
let previewId = 0
watch([open, method, model], async () => {
  const id = ++previewId
  plan.value = null
  error.value = ''
  if (!open.value || !model.value)
    return
  try {
    const result = await api.plan(props.accountId, method.value, model.value)
    if (id === previewId) plan.value = result
  }
  catch (e) {
    if (id === previewId) error.value = (e as Error).message
  }
}, { immediate: true })

async function start() {
  running.value = true
  error.value = ''
  blockers.value = []
  run.value = null
  try {
    await runProbe(props.accountId, method.value, model.value, (r) => {
      run.value = r
    })
  }
  catch (e) {
    error.value = (e as Error).message
    if (e instanceof ApiError)
      blockers.value = e.blockers
  }
  finally {
    running.value = false
    emit('done')
  }
}
</script>

<template>
  <ZDialog v-model="open" :title="`探测 ${accountName}`" width="44rem" :close-on-click-modal="!running">
    <div class="flex flex-col gap-4 text-cp-sm">
      <div class="flex flex-wrap items-center gap-3">
        <ZSegmented v-model="method" :options="methodOptions" aria-label="探测方法" :disabled="running" />
        <ZSelect v-model="model" :options="modelOptions" filterable placeholder="选择模型" class="min-w-56 flex-1" :disabled="running" />
      </div>
      <p class="m-0 text-cp-text-secondary">
        <template v-if="method === 'turnstate'">
          测票：直连 Codex 后端，走账号在 CPR 绑定的代理，拿一张票再最多带回 3 次，看上游是否接受。
        </template>
        <template v-else>
          ModelTrace：经 CPR 的模型链（指定这个账号）发 3 道“选整数”的题，按数字分布和指纹库比对；计入插件 Key 的用量。
        </template>
      </p>
      <div v-if="plan?.blockers.length && !run" class="flex flex-col gap-1 rounded-cp bg-cp-warning-container px-3 py-2 text-cp-warning-on-container">
        <span v-for="b in plan.blockers" :key="b.code">{{ b.message }}</span>
      </div>
      <div v-if="blockers.length" class="flex flex-col gap-1 rounded-cp bg-cp-warning-container px-3 py-2 text-cp-warning-on-container">
        <span v-for="b in blockers" :key="b.code">{{ b.message }}</span>
      </div>
      <p v-else-if="error" class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-error-on-container">
        {{ error }}
      </p>
      <RunView v-if="run" :run="run" />
      <div v-if="plan">
        <button type="button" class="text-cp-xs text-cp-text-tertiary underline-offset-2 hover:underline" @click="showRequest = !showRequest">
          {{ showRequest ? '收起请求详情' : '请求详情' }}
        </button>
        <pre v-if="showRequest" class="mt-2 max-h-72 overflow-auto rounded-cp bg-cp-fill-quaternary p-3 text-cp-xs">{{ JSON.stringify(plan.request, null, 2) }}</pre>
      </div>
    </div>
    <template #footer>
      <ZButton :disabled="running" @click="open = false">
        关闭
      </ZButton>
      <ZButton type="primary" :loading="running" :disabled="!model" @click="start">
        立即探测
      </ZButton>
    </template>
  </ZDialog>
</template>
