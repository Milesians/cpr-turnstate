import type { Level, Rate, Status } from './api'

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

export function dateTime(ms: number): string {
  if (!ms)
    return '—'
  return new Date(ms).toLocaleString('zh-CN', { hour12: false, month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

export function hour(ms: number): string {
  return new Date(ms).toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit' })
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
