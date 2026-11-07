import type { Counts, Level, Rate, Status } from './api'

export const levelLabel: Record<Level, string> = {
  ok: '正常',
  suspect: '疑似',
  degraded: '降智',
  unknown: '未判定',
}

export type TagType = 'default' | 'success' | 'warning' | 'danger' | 'info'

export function levelTag(level: Level | null | undefined): TagType {
  switch (level) {
    case 'ok': return 'success'
    case 'suspect': return 'warning'
    case 'degraded': return 'danger'
    default: return 'default'
  }
}

/** 格子颜色：未判定（探测了但失败、跳过、没发票）是深灰，没有探测是极浅的同尺寸竖条 */
export function levelBar(level: Level | null | undefined): string {
  switch (level) {
    case 'ok': return 'bg-cp-success'
    case 'suspect': return 'bg-cp-warning'
    case 'degraded': return 'bg-cp-error'
    case 'unknown': return 'bg-cp-text-tertiary'
    default: return 'bg-cp-fill-secondary'
  }
}

export function statusLabel(status: Status, level?: Level | null): string {
  if (status === 'running')
    return '进行中'
  if (status === 'failed')
    return '失败'
  return levelLabel[level ?? 'unknown']
}

/** 和后端的天、时间条边界一致，也和 TG 通知一致：一律按北京时间显示 */
const TZ = 'Asia/Shanghai'

export function dateTime(ms: number): string {
  if (!ms)
    return '—'
  return new Date(ms).toLocaleString('zh-CN', { timeZone: TZ, hour12: false, month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

export function hour(ms: number): string {
  return new Date(ms).toLocaleTimeString('zh-CN', { timeZone: TZ, hour12: false, hour: '2-digit', minute: '2-digit' })
}

export function duration(ms: number): string {
  if (ms < 1000)
    return `${ms} 毫秒`
  const s = Math.round(ms / 1000)
  return s < 60 ? `${s} 秒` : `${Math.floor(s / 60)} 分 ${s % 60} 秒`
}

export function rate(r: Rate): string {
  return r.judged ? `${Math.round((r.ok / r.judged) * 100)}%` : '—'
}

export function percent(p: number): string {
  return `${(p * 100).toFixed(0)}%`
}

export function interval(minutes: number): string {
  if (minutes % 1440 === 0)
    return `每 ${minutes / 1440} 天`
  if (minutes % 60 === 0)
    return `每 ${minutes / 60} 小时`
  return `每 ${minutes} 分钟`
}

/** 多久以前；超过 7 天显示日期 */
export function ago(ms: number, now = Date.now()): string {
  if (!ms)
    return '—'
  const s = Math.max(0, Math.round((now - ms) / 1000))
  if (s < 60)
    return '刚刚'
  if (s < 3600)
    return `${Math.floor(s / 60)} 分钟前`
  if (s < 86400)
    return `${Math.floor(s / 3600)} 小时前`
  if (s < 7 * 86400)
    return `${Math.floor(s / 86400)} 天前`
  return dateTime(ms)
}

export function day(ms: number): string {
  return new Date(ms).toLocaleDateString('zh-CN', { timeZone: TZ, month: 'numeric', day: 'numeric' })
}

const countLabels = [['ok', '正常'], ['suspect', '疑似'], ['degraded', '降智'], ['unknown', '未判定'], ['failed', '失败'], ['running', '进行中']] as const

/** 次数的文字说明，如“正常 10 · 降智 2” */
export function countsText(c: Counts): string {
  return countLabels.filter(([k]) => c[k]).map(([k, label]) => `${label} ${c[k]}`).join(' · ')
}

/** 次数的各项，用于小图例 */
export function countItems(c: Counts): { state: keyof Counts, label: string, count: number }[] {
  return countLabels
    .filter(([k]) => c[k])
    .map(([k, label]) => ({ state: k, label, count: c[k]! }))
}

/** 持续了多久：“25 分钟”“2 小时 5 分钟”“3 天 4 小时” */
export function span(ms: number): string {
  const minutes = Math.max(1, Math.floor(ms / 60_000))
  if (minutes < 60)
    return `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  if (hours < 24)
    return minutes % 60 ? `${hours} 小时 ${minutes % 60} 分钟` : `${hours} 小时`
  return hours % 24 ? `${Math.floor(hours / 24)} 天 ${hours % 24} 小时` : `${Math.floor(hours / 24)} 天`
}
