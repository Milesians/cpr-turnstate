<script setup lang="ts">
import type { Run } from '../api'
import { ZTag } from '@codex-proxy/ui'
import { methodLabel } from '../api'
import { dateTime, duration, levelTag, percent, statusLabel } from '../format'

defineProps<{ run: Run }>()

const outcomeLabel: Record<string, string> = {
  minted: '拿到票',
  no_ticket: '没有发票',
  accepted: '票被接受',
  refreshed: '换了新票',
  route_changed: '换了线路',
}
</script>

<template>
  <div class="flex flex-col gap-4 text-cp-sm">
    <div class="flex flex-wrap items-center gap-2">
      <ZTag :type="run.status === 'failed' ? 'default' : levelTag(run.verdict?.level)">
        {{ statusLabel(run.status, run.verdict?.level) }}
      </ZTag>
      <span class="text-cp-text-secondary">
        {{ methodLabel[run.method] }} · {{ run.model }} · {{ run.trigger === 'auto' ? '自动' : '手动' }} · {{ dateTime(run.started_ms) }}
        <template v-if="run.finished_ms"> · 用时 {{ duration(run.finished_ms - run.started_ms) }}</template>
      </span>
    </div>
    <p v-if="run.verdict" class="m-0 text-cp-text">
      {{ run.verdict.reason }}
    </p>
    <p v-if="run.error" class="m-0 rounded-cp bg-cp-fill-tertiary px-3 py-2 text-cp-text-secondary">
      {{ run.error.message }}<template v-if="run.error.status"> （HTTP {{ run.error.status }}）</template>
    </p>
    <p v-if="run.progress" class="m-0 text-cp-text-secondary">
      已答 {{ run.progress.answered }} / {{ run.progress.total }} 道题
    </p>

    <table v-if="run.turnstate" class="w-full border-collapse text-left">
      <thead class="text-cp-xs text-cp-text-tertiary">
        <tr>
          <th class="py-1 font-normal">
            第几次
          </th>
          <th class="py-1 font-normal">
            结果
          </th>
          <th class="py-1 font-normal">
            带回的票
          </th>
          <th class="py-1 font-normal">
            收到的票
          </th>
          <th class="py-1 font-normal">
            用时
          </th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="s in run.turnstate.steps" :key="s.shot" class="border-t border-cp-border-secondary">
          <td class="py-1.5">
            {{ s.shot === 0 ? '拿票' : `带回 ${s.shot}` }}
          </td>
          <td class="py-1.5">
            {{ outcomeLabel[s.outcome] ?? s.outcome }}
          </td>
          <td class="py-1.5 font-mono text-cp-xs">
            {{ s.sent_length ? `${s.sent_length} 字符 · ${s.sent_fingerprint}` : '—' }}
          </td>
          <td class="py-1.5 font-mono text-cp-xs">
            {{ s.got_length ? `${s.got_length} 字符 · ${s.got_fingerprint}` : '—' }}
          </td>
          <td class="py-1.5">
            {{ duration(s.duration_ms) }}
          </td>
        </tr>
      </tbody>
    </table>

    <template v-if="run.modeltrace">
      <div class="text-cp-xs text-cp-text-tertiary">
        指纹库 {{ run.modeltrace.bank_commit.slice(0, 10) }}<template v-if="run.modeltrace.fingerprint">
          · 按 {{ run.modeltrace.fingerprint }} 的指纹判定
        </template>
      </div>
      <div v-if="run.modeltrace.candidates.length" class="flex flex-col gap-1">
        <div class="text-cp-xs text-cp-text-tertiary">
          最像的模型
        </div>
        <div v-for="c in run.modeltrace.candidates" :key="c.model" class="flex items-center gap-2">
          <span class="w-40 truncate">{{ c.display_name || c.model }}</span>
          <span class="h-2 flex-1 overflow-hidden rounded-full bg-cp-fill-tertiary">
            <span class="block h-full rounded-full bg-cp-primary" :style="{ width: percent(c.probability) }" />
          </span>
          <span class="w-12 text-right tabular-nums">{{ percent(c.probability) }}</span>
        </div>
      </div>
      <div class="flex flex-col gap-2">
        <div class="text-cp-xs text-cp-text-tertiary">
          每道题的回答
        </div>
        <div v-for="(o, i) in run.modeltrace.outputs" :key="i" class="rounded-cp bg-cp-fill-quaternary px-3 py-2">
          <div class="flex flex-wrap gap-x-3 text-cp-xs text-cp-text-secondary">
            <span>要 {{ o.expected_count }} 个，读到 {{ o.parsed_numbers }} 个</span>
            <span>{{ o.accepted ? '有效' : '无效' }}</span>
            <span>输出 {{ o.output_tokens }} tokens（推理 {{ o.reasoning_tokens }}）</span>
            <span>{{ duration(o.duration_ms) }}</span>
            <span v-if="o.response_model">上游自称 {{ o.response_model }}</span>
          </div>
          <div class="mt-1 break-all font-mono text-cp-xs text-cp-text">
            {{ o.excerpt || '（空）' }}
          </div>
        </div>
      </div>
    </template>
  </div>
</template>
