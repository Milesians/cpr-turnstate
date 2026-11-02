<script setup lang="ts">
import type { CprGroup, GroupRule, GroupsView, Level } from '../api'
import { ZButton, ZCheckbox, ZDialog, ZInput, ZSegmented, ZSelect, ZSwitch } from '@codex-proxy/ui'
import { Plus, Trash2 } from '@lucide/vue'
import { computed, ref, shallowRef, watch } from 'vue'
import { api } from '../api'
import { dateTime, levelLabel } from '../format'

const open = defineModel<boolean>({ default: false })

const levels: Level[] = ['ok', 'suspect', 'degraded', 'unknown']
const colors = [
  { label: '绿', value: '#22C55EFF' },
  { label: '红', value: '#EF4444FF' },
  { label: '黄', value: '#EAB308FF' },
  { label: '蓝', value: '#3B82F6FF' },
  { label: '紫', value: '#8B5CF6FF' },
  { label: '灰', value: '#6B7280FF' },
]
const kinds = [{ label: '插件新建分组', value: 'owned' }, { label: 'CPR 现有分组', value: 'existing' }]
// 没配过时给一份建议，保存后才生效
const suggested = (): GroupRule[] => [
  { name: '测票正常', color: '#22C55EFF', levels: ['ok'] },
  { name: '测票异常', color: '#EF4444FF', levels: ['suspect', 'degraded'] },
]

const view = shallowRef<GroupsView | null>(null)
const enabled = ref(false)
const rules = ref<GroupRule[]>([])
const cprGroups = shallowRef<CprGroup[]>([])
const cprError = ref('')
const loading = ref(false)
const saving = ref(false)
const error = ref('')
const notice = ref('')

const groupOptions = computed(() => cprGroups.value.map(g => ({
  label: g.plugin ? `${g.name}（插件维护，不能选）` : g.enabled ? g.name : `${g.name}（已停用）`,
  value: g.id,
  disabled: g.plugin,
})))

function kind(rule: GroupRule) {
  return rule.group_id === undefined ? 'owned' : 'existing'
}
function setKind(rule: GroupRule, value: string) {
  if (value === 'existing')
    rule.group_id = ''
  else
    delete rule.group_id
}

function fill(v: GroupsView) {
  view.value = v
  enabled.value = v.enabled
  rules.value = v.rules.length ? v.rules.map(r => ({ ...r, levels: [...r.levels] })) : suggested()
}

async function loadCprGroups() {
  cprError.value = ''
  if (view.value?.admin_error) {
    cprGroups.value = []
    return
  }
  try { cprGroups.value = (await api.cprGroups()).groups }
  catch (e) { cprError.value = (e as Error).message }
}

async function load() {
  loading.value = true
  error.value = ''
  notice.value = ''
  try {
    fill(await api.groups())
    await loadCprGroups()
  }
  catch (e) { error.value = (e as Error).message }
  finally { loading.value = false }
}
watch(open, value => { if (value) load() })

function toggle(rule: GroupRule, level: Level, on: boolean) {
  rule.levels = on ? [...new Set([...rule.levels, level])] : rule.levels.filter(l => l !== level)
}
function add() {
  rules.value.push({ name: '', color: '#3B82F6FF', levels: [] })
}

async function save() {
  error.value = ''
  notice.value = ''
  if (rules.value.some(r => r.group_id === '')) {
    error.value = '请为“CPR 现有分组”的规则选择分组'
    return
  }
  saving.value = true
  try {
    const body = rules.value.map(r => r.group_id === undefined
      ? { id: r.id, name: r.name.trim(), color: r.color, levels: r.levels }
      : { id: r.id, group_id: r.group_id, name: r.name, color: r.color, levels: r.levels })
    fill(await api.saveGroups(enabled.value, body))
    notice.value = enabled.value ? '已保存，并按当前测票判定同步了分组' : '已保存；自动分组关闭，分组成员保持不变'
  }
  catch (e) { error.value = (e as Error).message }
  finally { saving.value = false }
}
</script>

<template>
  <ZDialog v-model="open" title="自动分组" width="42rem" :close-on-click-modal="!saving" :close-on-press-escape="!saving" :before-close="() => !saving">
    <div class="flex flex-col gap-4 text-cp-sm">
      <p class="m-0 text-cp-text-secondary">
        按每个账号的<strong>测票当前判定</strong>（连续 3 次确认）自动调整 CPR 分组的成员，Client Key 绑定“测票正常”这类分组就只调度正常账号。
      </p>
      <ZSwitch v-model="enabled" label="开启自动分组" :disabled="saving || loading" />
      <div v-if="loading && !view" class="text-cp-text-tertiary">正在读取…</div>
      <div v-for="(rule, i) in rules" :key="rule.id ?? `new-${i}`" class="flex flex-col gap-2 rounded-cp border border-cp-border-secondary p-3" :data-rule="rule.id ?? 'new'">
        <div class="flex flex-wrap items-center gap-2">
          <ZSegmented v-if="!rule.id" :model-value="kind(rule)" :options="kinds" size="small" :disabled="saving" :aria-label="`第 ${i + 1} 个分组的类型`" @update:model-value="(v: string) => setKind(rule, v)" />
          <span v-else class="text-cp-xs text-cp-text-tertiary">{{ kind(rule) === 'owned' ? '插件新建分组' : 'CPR 现有分组' }}</span>
          <span class="flex-1" />
          <button type="button" class="inline-flex items-center gap-1 text-cp-xs text-cp-text-secondary hover:text-cp-error" :disabled="saving" :aria-label="`删除分组 ${rule.name || i + 1}`" @click="rules.splice(i, 1)">
            <Trash2 :size="14" aria-hidden="true" /> 删除
          </button>
        </div>
        <div v-if="kind(rule) === 'owned'" class="flex flex-wrap items-center gap-2">
          <ZInput v-model="rule.name" placeholder="分组名" class="min-w-40 flex-1" :disabled="saving || !!rule.id" :aria-label="`第 ${i + 1} 个分组的名称`" />
          <span class="inline-block size-3 shrink-0 rounded-full" :style="{ backgroundColor: rule.color }" aria-hidden="true" />
          <ZSelect v-model="rule.color" :options="colors" class="w-24" :disabled="saving || !!rule.id" :aria-label="`第 ${i + 1} 个分组的颜色`" />
        </div>
        <div v-else class="flex flex-col gap-1">
          <ZSelect v-if="!rule.id" v-model="rule.group_id" :options="groupOptions" filterable placeholder="选择 CPR 分组" :disabled="saving || !!view?.admin_error" :aria-label="`第 ${i + 1} 个规则的 CPR 分组`" />
          <span v-else class="flex items-center gap-2"><span class="inline-block size-3 rounded-full" :style="{ backgroundColor: rule.color }" aria-hidden="true" />{{ rule.name }}</span>
          <span class="text-cp-xs text-cp-text-tertiary">经 CPR 管理接口调整：不符合的原有成员会被移出；删除规则后不再调整，成员保持当时的状态</span>
        </div>
        <div class="flex flex-wrap items-center gap-3">
          <span class="text-cp-xs text-cp-text-secondary">判定为</span>
          <ZCheckbox v-for="l in levels" :key="l" :model-value="rule.levels.includes(l)" :label="levelLabel[l]" :disabled="saving" @update:model-value="(on: boolean) => toggle(rule, l, on)" />
          <span class="text-cp-xs text-cp-text-tertiary">的账号进这个组</span>
        </div>
        <p v-if="rule.id && view?.status.rules[rule.id]" class="m-0 text-cp-xs" :class="view.status.rules[rule.id]!.error ? 'text-cp-warning-text' : 'text-cp-text-tertiary'">
          <template v-if="view.status.rules[rule.id]!.error">{{ view.status.rules[rule.id]!.error }}</template>
          <template v-else>CPR 分组“{{ view.status.rules[rule.id]!.group_name }}” · {{ view.status.rules[rule.id]!.members }} 个账号</template>
        </p>
        <p v-else-if="!rule.id && kind(rule) === 'owned'" class="m-0 text-cp-xs text-cp-text-tertiary">保存后在 CPR 新建这个分组；名称和颜色之后在 CPR 的分组管理里改</p>
      </div>
      <p v-if="view?.admin_error && rules.some(r => r.group_id !== undefined)" class="m-0 rounded-cp bg-cp-warning-container px-3 py-2 text-cp-warning-on-container">{{ view.admin_error }}</p>
      <p v-else-if="cprError" class="m-0 rounded-cp bg-cp-warning-container px-3 py-2 text-cp-warning-on-container">读不到 CPR 的分组：{{ cprError }}</p>
      <div class="flex flex-wrap items-center gap-3">
        <ZButton size="small" :disabled="saving || rules.length >= (view?.max_rules ?? 8)" @click="add">
          <Plus :size="14" aria-hidden="true" /> 添加分组
        </ZButton>
        <span v-if="view?.status.synced_ms" class="text-cp-xs text-cp-text-tertiary">上次同步 {{ dateTime(view.status.synced_ms) }}</span>
      </div>
      <p class="m-0 text-cp-xs text-cp-text-tertiary">
        刚测完的账号下一轮维护（约 30 秒）调整，每 5 分钟全量核对一次并纠正 CPR 里的手动改动；不支持探测的账号不进任何组；关闭后成员保持不变。
        插件新建的分组删除规则时会清空成员，分组本身请在 CPR 里删除<span v-if="view?.retired">（还有 {{ view.retired }} 个待清空）</span>。
        选 CPR 现有分组需要在插件设置里填写 CPR 地址和管理 API Key（该 Key 拥有全部管理权限，插件只用它读分组和改账号的分组）。
      </p>
      <p v-if="view?.status.error" class="m-0 rounded-cp bg-cp-warning-container px-3 py-2 text-cp-warning-on-container">{{ view.status.error }}</p>
      <p v-if="error" class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-error-on-container" role="alert">{{ error }}</p>
      <p v-if="notice" class="m-0 text-cp-xs text-cp-success" role="status">{{ notice }}</p>
    </div>
    <template #footer>
      <ZButton :disabled="saving" @click="open = false">关闭</ZButton>
      <ZButton type="primary" :loading="saving" :disabled="loading" @click="save">保存</ZButton>
    </template>
  </ZDialog>
</template>
