import type { Level, OverviewAccount, Slot } from './api'

const severity: Level[] = ['unknown', 'ok', 'suspect', 'degraded']
export type CellState = Level | 'empty' | 'running'

export function cellState(slot: Slot): CellState {
  if (!slot.runs.length)
    return 'empty'
  if (slot.level)
    return slot.level
  return slot.runs.every(r => r.status === 'running') ? 'running' : 'unknown'
}

/** 合并相邻时间段，保留所有记录；空白时段不会把一次真实判定稀释掉。 */
export function fitSlots(slots: Slot[], width: number): Slot[] {
  const count = Math.max(1, Math.min(slots.length, Math.floor(width / 11)))
  return Array.from({ length: count }, (_, index) => {
    const group = slots.slice(Math.floor(index * slots.length / count), Math.floor((index + 1) * slots.length / count))
    const levels = group.flatMap(s => s.level ? [s.level] : [])
    return {
      start_ms: group[0]!.start_ms,
      end_ms: group[group.length - 1]!.end_ms,
      level: levels.length ? levels.reduce((a, b) => severity.indexOf(a) >= severity.indexOf(b) ? a : b) : null,
      runs: group.flatMap(s => s.runs).sort((a, b) => b.started_ms - a.started_ms),
    }
  })
}

export function accountState(account: OverviewAccount): CellState {
  if (account.running)
    return 'running'
  const slots = Object.values(account.methods).flatMap(m => m.slots)
  const states = slots.map(cellState).filter(s => s !== 'empty' && s !== 'running') as Level[]
  if (states.length)
    return states.reduce((a, b) => severity.indexOf(a) >= severity.indexOf(b) ? a : b)
  return slots.some(s => cellState(s) === 'running') ? 'running' : 'empty'
}

export const stateLabel: Record<CellState, string> = {
  ok: '正常', suspect: '疑似', degraded: '降智', unknown: '未判定', empty: '没有探测', running: '探测中',
}
