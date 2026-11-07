<script setup lang="ts">
import type { Overview } from '../api'
import { ZButton, ZSegmented, ZTag } from '@codex-proxy/ui'
import { ArrowLeft, RefreshCw } from '@lucide/vue'
import { ref } from 'vue'
import { api } from '../api'
import { dateTime } from '../format'
import GroupsPanel from './GroupsPanel.vue'

defineProps<{ overview: Overview | null }>()
const emit = defineEmits<{ back: [], changed: [] }>()
const tab = defineModel<'groups' | 'notify'>('tab', { default: 'groups' })

const tabs = [{ label: '自动分组', value: 'groups' }, { label: '通知与运行', value: 'notify' }]
const syncing = ref(false)
const testing = ref(false)
const notice = ref('')
const error = ref('')

async function syncBank() {
  syncing.value = true
  error.value = ''
  notice.value = ''
  try {
    await api.syncBank()
    notice.value = '指纹库已检查更新'
    emit('changed')
  }
  catch (e) { error.value = (e as Error).message }
  finally { syncing.value = false }
}
async function testTelegram() {
  testing.value = true
  error.value = ''
  notice.value = ''
  try {
    await api.testTelegram()
    notice.value = 'TG 测试通知已发送'
  }
  catch (e) { error.value = (e as Error).message }
  finally { testing.value = false }
}
</script>

<template>
  <div class="detail-page">
    <header class="detail-header">
      <ZButton variant="text" @click="emit('back')"><ArrowLeft class="size-4" aria-hidden="true" /> 返回列表</ZButton>
      <h1 class="m-0 text-lg font-semibold text-cp-text">设置</h1>
      <span class="flex-1" />
      <ZSegmented v-model="tab" :options="tabs" aria-label="设置分类" />
    </header>

    <section v-if="tab === 'groups'" class="detail-card">
      <GroupsPanel :overview="overview" />
    </section>

    <template v-else>
      <section class="detail-card">
        <h2 class="settings-title">TG 状态通知</h2>
        <div class="settings-row">
          <span>通知开关</span>
          <ZTag size="small" :type="overview?.config.telegram_enabled ? overview.config.telegram_configured ? 'success' : 'warning' : 'default'">
            {{ overview?.config.telegram_enabled ? overview.config.telegram_configured ? '已开启' : '已开启，配置不完整' : '未开启' }}
          </ZTag>
          <span class="flex-1" />
          <ZButton size="small" :loading="testing" :disabled="!overview?.config.telegram_configured" @click="testTelegram">发送测试通知</ZButton>
        </div>
        <p class="settings-note">账号的测票当前判定在正常和异常（疑似、降智）之间切换时发送一条通知。在 CPR 的插件详情 → 设置里配置开关、Chat ID 和话题 ID，在敏感配置里填写 Bot Token；测试通知不受开关影响。</p>
      </section>

      <section class="detail-card">
        <h2 class="settings-title">探测</h2>
        <div class="settings-row">
          <span>发出真实探测</span>
          <ZTag size="small" :type="overview?.config.probe_enabled ? 'success' : 'default'">{{ overview?.config.probe_enabled ? '已开启' : '未开启' }}</ZTag>
        </div>
        <div class="settings-row">
          <span>自动探测最短间隔</span>
          <span class="text-cp-text-secondary">{{ overview ? `${overview.config.min_interval_minutes} 分钟` : '—' }}</span>
        </div>
        <p class="settings-note">这两项在 CPR 的插件详情 → 设置里修改。关闭“发出真实探测”时手动和自动探测都发不出去；每个账号的自动探测开关、模型和间隔在账号详情或列表的批量配置里改。</p>
      </section>

      <section class="detail-card">
        <h2 class="settings-title">ModelTrace 指纹库</h2>
        <div class="settings-row">
          <span>当前版本</span>
          <span v-if="overview" class="font-mono text-cp-xs text-cp-text-secondary">{{ overview.bank.commit.slice(0, 10) }} · {{ overview.bank.source === 'github' ? 'GitHub' : '内置' }} · {{ overview.bank.models.length }} 个模型</span>
          <span class="flex-1" />
          <ZButton size="small" :loading="syncing" @click="syncBank"><RefreshCw :size="14" aria-hidden="true" /> 立即同步</ZButton>
        </div>
        <p v-if="overview?.bank.checked_ms" class="settings-note">上次检查 {{ dateTime(overview.bank.checked_ms) }}，按“指纹库同步间隔”（{{ overview.config.fingerprint_sync_hours ? `${overview.config.fingerprint_sync_hours} 小时` : '已关闭' }}）自动同步。</p>
        <p v-if="overview?.bank.error" class="m-0 text-cp-xs text-cp-warning-text">{{ overview.bank.error }}</p>
      </section>
      <p v-if="notice" class="m-0 text-cp-xs text-cp-success" role="status">{{ notice }}</p>
      <p v-if="error" class="m-0 rounded-cp bg-cp-error-container px-3 py-2 text-cp-sm text-cp-error-on-container" role="alert">{{ error }}</p>
    </template>
  </div>
</template>
