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

/** 时间条怎么画：几格、格间距和每格宽度（CSS 像素，按设备像素取整） */
export interface StripLayout {
  count: number
  gap: number
  widths: number[]
}

/** 一格（含间距）最窄几个 CSS 像素；更窄就合并相邻时段 */
export const MIN_PITCH = 7

const DAY_MS = 86_400_000
/** 北京时间相对 UTC 的偏移；格子和自动探测都按北京时间对齐 */
const OFFSET_MS = 8 * 3_600_000

/** 把 t 按北京时间向下对齐到 step 的整数倍（和后端 series::floor 一样） */
export function floorBj(t: number, step: number): number {
  return Math.floor((t + OFFSET_MS) / step) * step - OFFSET_MS
}

/**
 * 把时段按北京时间对齐合并成每格 span 长：边界落在 span 的整数倍上（自动探测也按同样的段发起，
 * 每格正好一次）。范围起点不在边界上时，开头不满一格的并进下一格；最后一格是现在所在的一段，可能还不满。
 * 保留完整范围和全部次数；空白时段不会稀释一次真实判定。
 */
export function groupSpans(spans: Span[], span: number): Span[] {
  const out: Span[] = []
  let key = Number.NaN
  for (const s of spans) {
    const at = floorBj(s.start_ms, span)
    const last = out.at(-1)
    if (last && at === key) {
      last.end_ms = s.end_ms
      last.counts = merge(last.counts, s.counts)
    }
    else {
      out.push({ ...s })
      key = at
    }
  }
  const [head, next] = out
  if (head && next && floorBj(head.start_ms, span) !== head.start_ms)
    out.splice(0, 2, { start_ms: head.start_ms, end_ms: next.end_ms, counts: merge(head.counts, next.counts) })
  return out
}

/**
 * 每格多长：时段的整数倍，并且能整除一天（或是整天的倍数），格子边界落在整点。
 * 不短于探测间隔——每 30 分钟探测一次就 30 分钟一格，不会一格有、一格空；
 * 宽度不够时再加长，每格（含间距）不窄于 MIN_PITCH。
 */
export function cellSpan(range: Range, width: number, intervalMs = 0, dpr = 1): number {
  const ratio = dpr > 0 ? dpr : 1
  const device = Math.max(0, Math.floor(width * ratio))
  const spans = expand(range, [])
  let fallback = range.slot_ms * Math.max(1, range.slots)
  for (let k = 1; k <= range.slots; k++) {
    const span = k * range.slot_ms
    if (DAY_MS % span !== 0 && span % DAY_MS !== 0)
      continue
    if (span < intervalMs)
      continue
    const count = groupSpans(spans, span).length
    if (count <= 1 || device / count >= MIN_PITCH * ratio)
      return span
    fallback = span
  }
  return fallback
}

/**
 * 按可用宽度画 count 格：间距随格宽变（约四分之一，1–3 像素）；宽度按设备像素取整，
 * 余下的像素均匀分给各格，避免小数像素让有的间距看不见、有的格子粘在一起。
 */
export function stripLayout(count: number, width: number, dpr = 1): StripLayout {
  const ratio = dpr > 0 ? dpr : 1
  const device = Math.max(0, Math.floor(width * ratio))
  count = Math.max(1, Math.floor(count))
  const pitch = device / count
  const gap = count > 1 ? Math.min(3 * ratio, Math.max(Math.round(ratio), Math.round(pitch / 4))) : 0
  const free = Math.max(count, device - (count - 1) * gap)
  const widths = Array.from({ length: count }, (_, i) => (Math.floor((i + 1) * free / count) - Math.floor(i * free / count)) / ratio)
  return { count, gap: gap / ratio, widths }
}

/** 账号自动测票的实际间隔（毫秒）：设置的间隔，不短于全局最短间隔；关了自动探测也按它（历史多半是它发的） */
export function probeInterval(account: Pick<OverviewAccount, 'schedule'>, minIntervalMinutes: number): number {
  return Math.max(account.schedule.turnstate.interval_minutes, minIntervalMinutes) * 60_000
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
