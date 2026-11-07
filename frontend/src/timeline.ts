import type { Cell, Counts, Level, OverviewAccount, Range } from './api'

export type CellState = Level | 'empty' | 'running'

/** 时间条上的一段：起止时间和这段时间的次数 */
export interface Span {
  start_ms: number
  end_ms: number
  counts: Counts
}

const keys = ['ok', 'suspect', 'degraded', 'unknown', 'failed', 'running'] as const

export function total(c: Counts): number {
  return keys.reduce((n, k) => n + (c[k] ?? 0), 0)
}

/** 有效判定（正常、疑似、降智）的次数 */
export function judged(c: Counts): number {
  return (c.ok ?? 0) + (c.suspect ?? 0) + (c.degraded ?? 0)
}

export function merge(a: Counts, b: Counts): Counts {
  const out: Counts = {}
  for (const k of keys) {
    const n = (a[k] ?? 0) + (b[k] ?? 0)
    if (n)
      out[k] = n
  }
  return out
}

/** 最严重的结果；失败和未判定算 unknown；只有进行中的没有结果 */
export function worst(c: Counts): Level | null {
  if (c.degraded)
    return 'degraded'
  if (c.suspect)
    return 'suspect'
  if (c.ok)
    return 'ok'
  if (c.unknown || c.failed)
    return 'unknown'
  return null
}

export function cellState(c: Counts): CellState {
  if (!total(c))
    return 'empty'
  return worst(c) ?? 'running'
}

/** 按范围展开成完整的时间条；接口只列出有探测的格子 */
export function expand(range: Range, cells: Cell[]): Span[] {
  const spans: Span[] = Array.from({ length: range.slots }, (_, i) => ({
    start_ms: range.start_ms + i * range.slot_ms,
    end_ms: range.start_ms + (i + 1) * range.slot_ms,
    counts: {},
  }))
  for (const { i, ...counts } of cells) {
    if (spans[i])
      spans[i].counts = counts
  }
  return spans
}

/** 按可用宽度合并相邻时段，保留完整范围和全部次数；空白时段不会稀释一次真实判定 */
export function fitSpans(spans: Span[], width: number, cellWidth = 11): Span[] {
  if (!spans.length)
    return []
  const count = Math.max(1, Math.min(spans.length, Math.floor(width / cellWidth)))
  return Array.from({ length: count }, (_, index) => {
    const group = spans.slice(Math.floor(index * spans.length / count), Math.floor((index + 1) * spans.length / count))
    return {
      start_ms: group[0]!.start_ms,
      end_ms: group[group.length - 1]!.end_ms,
      counts: group.reduce<Counts>((c, s) => merge(c, s.counts), {}),
    }
  })
}

/**
 * 账号的当前状态：优先测票的当前判定（连续 3 次确认，和所选时间范围无关）；
 * 没有测票结果时用最近一次 ModelTrace。
 */
export function accountState(account: OverviewAccount): CellState {
  if (account.running)
    return 'running'
  const health = account.health
  if (health?.last_run_id)
    return health.level
  const latest = account.latest.modeltrace ?? account.latest.turnstate
  if (!latest)
    return 'empty'
  return latest.status === 'done' ? latest.level ?? 'unknown' : 'unknown'
}

/** 列表排序：问题最严重的在前 */
export const severity: Record<CellState, number> = {
  degraded: 0, suspect: 1, unknown: 2, running: 3, ok: 4, empty: 5,
}

/** 探测受阻（凭据失效等）已确认（持续 30 分钟）；不是判定，但要排在前面、算作需要关注 */
export function isBlocked(account: Pick<OverviewAccount, 'blocked'>): boolean {
  return !!account.blocked?.confirmed_ms
}

/** 排序用的严重程度：确认受阻的排在疑似之后、未判定之前（降智和疑似仍在它前面） */
export function rank(account: OverviewAccount): number {
  const s = severity[accountState(account)]
  return isBlocked(account) ? Math.min(s, 1.5) : s
}

/** 需要关注：降智、疑似，或探测受阻 */
export function needsAttention(account: OverviewAccount): boolean {
  const s = accountState(account)
  return s === 'degraded' || s === 'suspect' || isBlocked(account)
}

/** 受阻原因的短说明 */
export function blockedLabel(code: string): string {
  switch (code) {
    case 'credential_state': return '凭据失效'
    case 'token_expired': return '令牌过期'
    case 'no_credentials': return '缺少账号 ID'
    default: return '探测受阻'
  }
}

/** 判定正在变化：候选判定和当前不同，已连续确认了几次（满 3 次改变当前判定） */
export function pendingChange(account: OverviewAccount): { to: Level, streak: number } | null {
  const h = account.health
  if (!h?.candidate || !h.streak || h.candidate === h.level)
    return null
  return { to: h.candidate, streak: h.streak }
}

/** 待确认判定的说明，如“转降智中 2/3”“恢复中 1/3” */
export function pendingLabel(level: Level, to: Level, streak: number): string {
  if (to === 'ok')
    return `${level === 'unknown' ? '确认正常中' : '恢复中'} ${streak}/3`
  return `转${stateLabel[to]}中 ${streak}/3`
}

export const stateLabel: Record<CellState, string> = {
  ok: '正常', suspect: '疑似', degraded: '降智', unknown: '未判定', empty: '没有探测', running: '探测中',
}
