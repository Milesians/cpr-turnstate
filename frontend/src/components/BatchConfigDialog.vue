<script setup lang="ts">
import type { Method, Models, SchedulePatch } from '../api'
import { ZButton, ZCheckbox, ZDialog, ZInputNumber, ZSegmented, ZSelect } from '@codex-proxy/ui'
import { computed, ref, watch } from 'vue'
import { api, methodLabel, methods } from '../api'
import { defaultModels } from '../probe-models'

const props = defineProps<{ accountIds: string[], models: Models | null }>()
const emit = defineEmits<{ done: [count: number, method: Method] }>()
const open = defineModel<boolean>({ default: false })
const method = ref<Method>('turnstate')
const enabled = ref('keep')
const model = ref('')
const changeInterval = ref(false)
const interval = ref(60)
const saving = ref(false)
const error = ref('')
const methodOptions = methods.map(value => ({ label: methodLabel[value], value }))
const enabledOptions = [{ label: '保持原值', value: 'keep' }, { label: '开启', value: 'on' }, { label: '关闭', value: 'off' }]
const modelOptions = computed(() => {
  const ids = new Set([defaultModels[method.value], ...(props.models?.cpr ?? []), ...(props.models?.bank ?? []).map(m => m.id)])
  return [{ label: '保持各账号原模型', value: '' }, ...[...ids].sort().map(value => ({ label: value, value }))]
})
const changed = computed(() => enabled.value !== 'keep' || !!model.value || changeInterval.value)
function reset() {
  enabled.value = 'keep'
  model.value = ''
  changeInterval.value = false
  interval.value = 60
  error.value = ''
}
watch(open, value => { if (value) reset() })
watch(method, reset)
async function save() {
  saving.value = true
  error.value = ''
  const patch: SchedulePatch = {}
  if (enabled.value !== 'keep') patch.enabled = enabled.value === 'on'
  if (model.value) patch.model = model.value
  if (changeInterval.value) patch.interval_minutes = interval.value
  try {
    const result = await api.batchSchedule(props.accountIds, method.value, patch)
    open.value = false
    emit('done', result.updated, method.value)
  }
  catch (e) { error.value = (e as Error).message }
  finally { saving.value = false }
}
</script>

<template>
  <ZDialog v-model="open" title="批量配置探测" width="32rem" :close-on-click-modal="!saving" :close-on-press-escape="!saving" :before-close="() => !saving">
    <div class="flex flex-col gap-5 text-cp-sm">
      <p class="m-0 text-cp-text-secondary">已选择 {{ accountIds.length }} 个账号。只修改下方指定的项目，其他配置保持各账号原值。</p>
      <ZSegmented v-model="method" :options="methodOptions" aria-label="配置的探测方法" :disabled="saving" />
      <label class="flex flex-col gap-2"><span class="font-medium">自动探测</span><ZSegmented v-model="enabled" :options="enabledOptions" aria-label="批量自动探测开关" :disabled="saving" /></label>
      <label class="flex flex-col gap-2"><span class="font-medium">探测模型</span><ZSelect v-model="model" :options="modelOptions" filterable :disabled="saving" aria-label="批量探测模型" /><span class="text-cp-xs text-cp-text-tertiary">{{ methodLabel[method] }}默认模型：{{ defaultModels[method] }}</span></label>
      <div class="flex flex-wrap items-center gap-3"><ZCheckbox v-model="changeInterval" label="修改自动探测间隔" :disabled="saving" /><ZInputNumber v-if="changeInterval" v-model="interval" :min="1" :max="10080" unit="分钟" :disabled="saving" aria-label="批量探测间隔" /></div>
      <p v-if="error" class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-error-on-container" role="alert">{{ error }}</p>
    </div>
    <template #footer><ZButton :disabled="saving" @click="open = false">取消</ZButton><ZButton type="primary" :loading="saving" :disabled="!changed || !accountIds.length" @click="save">应用到 {{ accountIds.length }} 个账号</ZButton></template>
  </ZDialog>
</template>
