import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { OverviewAccount, Range } from '../src/api.ts'
import { accountState, blockedLabel, cellSpan, cellState, expand, floorBj, groupSpans, isBlocked, MIN_PITCH, needsAttention, pendingChange, pendingLabel, probeInterval, rank, severity, stripLayout, total, worst } from '../src/timeline.ts'

const range: Range = { hours: 24, start_ms: 0, slot_ms: 900000, slots: 96 }

test('sparse cells expand to the full range in place', () => {
  const spans = expand(range, [{ i: 0, ok: 1 }, { i: 95, degraded: 2, running: 1 }])
  assert.equal(spans.length, 96)
  assert.equal(spans[0]!.start_ms, 0)
  assert.equal(spans.at(-1)!.end_ms, 24 * 3600000)
  assert.deepEqual(spans[95]!.counts, { degraded: 2, running: 1 })
  assert.equal(total(spans[1]!.counts), 0)
})

const MIN = 60_000
const HOUR = 60 * MIN
// 北京时间 00:00 起的 24 小时（对齐）
const day0 = floorBj(1_794_026_096_000, 24 * HOUR)
const aligned: Range = { hours: 24, start_ms: day0, slot_ms: 15 * MIN, slots: 96 }
// 起点在 xx:15，不在 30 分钟边界上
const shifted: Range = { ...aligned, start_ms: day0 + 15 * MIN }

function check(out: ReturnType<typeof groupSpans>, r: Range, n: number) {
  assert.equal(out[0]!.start_ms, r.start_ms)
  assert.equal(out.at(-1)!.end_ms, r.start_ms + r.slots * r.slot_ms)
  assert(out.every((s, i) => !i || s.start_ms === out[i - 1]!.end_ms))
  assert.equal(out.reduce((c, s) => c + total(s.counts), 0), n)
}

test('responsive cells keep the whole range and every count', () => {
  for (const r of [aligned, shifted]) {
    const spans = expand(r, [{ i: 0, ok: 1 }, { i: 95, degraded: 1 }])
    for (const width of [180, 480, 1200]) {
      const out = groupSpans(spans, cellSpan(r, width))
      check(out, r, 2)
      assert(width / out.length >= MIN_PITCH)
      assert.equal(cellState(out.at(-1)!.counts), 'degraded')
    }
  }
  assert.equal(groupSpans(expand(aligned, []), cellSpan(aligned, 1200)).length, 96)
  assert.equal(groupSpans(expand(aligned, []), cellSpan(aligned, 600)).length, 48)
})

test('cells are no shorter than the probe interval, so every cell holds one probe', () => {
  // 每 30 分钟一次（整点和半点），宽度足够画 96 格时也按 30 分钟一格，没有空格
  for (const r of [aligned, shifted]) {
    const cells = Array.from({ length: 96 }, (_, i) => i)
      .filter(i => (r.start_ms + i * r.slot_ms - day0) % (30 * MIN) === 0)
      .map(i => ({ i, ok: 1 }))
    const span = cellSpan(r, 1300, 30 * MIN)
    assert.equal(span, 30 * MIN)
    const out = groupSpans(expand(r, cells), span)
    check(out, r, cells.length)
    assert(out.every(s => total(s.counts) >= 1), 'no empty cell')
    // 边界落在半点上；起点不对齐时开头不满一格的并进第一格
    assert(out.slice(1).every(s => floorBj(s.start_ms, 30 * MIN) === s.start_ms))
  }
  assert.equal(groupSpans(expand(shifted, []), 30 * MIN).length, 48)
  // 45 分钟能整除一天，也按 45 分钟；60 分钟、2 小时同理
  assert.equal(cellSpan(aligned, 1300, 45 * MIN), 45 * MIN)
  assert.equal(cellSpan(aligned, 1300, 60 * MIN), HOUR)
  assert.equal(cellSpan(aligned, 1300, 50 * MIN), HOUR)
  // 太窄时再加长
  assert(cellSpan(aligned, 150, 30 * MIN) > 30 * MIN)
  // 7 天范围本来就 2 小时一格
  const week: Range = { hours: 168, start_ms: day0, slot_ms: 2 * HOUR, slots: 84 }
  assert.equal(cellSpan(week, 1300, 30 * MIN), 2 * HOUR)
})

test('probe interval honours the global minimum', () => {
  const s = { enabled: true, model: 'm', interval_minutes: 15 }
  assert.equal(probeInterval({ schedule: { turnstate: s, modeltrace: s } }, 30), 30 * MIN)
  assert.equal(probeInterval({ schedule: { turnstate: { ...s, interval_minutes: 60 }, modeltrace: s } }, 30), HOUR)
})

test('strip layout snaps to device pixels and scales the gap', () => {
  for (const [count, width, dpr] of [[96, 1170, 2], [96, 1170.4, 1], [48, 600, 1], [32, 300, 1.5], [84, 700, 1], [60, 500, 2], [1, 50, 1]] as const) {
    const { gap, widths } = stripLayout(count, width, dpr)
    assert.equal(widths.length, count)
    const used = widths.reduce((a, b) => a + b, 0) + gap * (count - 1)
    assert(used <= width + 1e-9 && width - used < count / dpr + 1)
    for (const w of [...widths, gap])
      assert.equal(Math.abs(w * dpr - Math.round(w * dpr)) < 1e-9, true)
    assert(Math.max(...widths) - Math.min(...widths) <= 1 / dpr + 1e-9)
    if (count > 1)
      assert(gap >= 1 / dpr && gap <= 3)
  }
  assert.equal(stripLayout(96, 1170, 2).gap, 3)
})

test('no probe, in progress, failed and verdicts are different states', () => {
  assert.equal(cellState({}), 'empty')
  assert.equal(cellState({ running: 1 }), 'running')
  assert.equal(cellState({ failed: 1, running: 1 }), 'unknown')
  assert.equal(cellState({ unknown: 1 }), 'unknown')
  assert.equal(cellState({ ok: 2, running: 1 }), 'ok')
  assert.equal(worst({ ok: 5, suspect: 1 }), 'suspect')
  assert.equal(worst({ suspect: 5, degraded: 1 }), 'degraded')
})

test('merging empty time does not hide a worse verdict', () => {
  const spans = expand(range, [{ i: 0, ok: 1 }, { i: 1, failed: 1 }])
  assert.equal(cellState(groupSpans(spans, 24 * HOUR)[0]!.counts), 'ok')
  const worse = expand(range, [{ i: 0, ok: 1 }, { i: 2, degraded: 1 }])
  assert.equal(cellState(groupSpans(worse, 24 * HOUR)[0]!.counts), 'degraded')
})

function account(): OverviewAccount {
  const schedule = { enabled: true, model: 'm', interval_minutes: 30 }
  return {
    id: 'a', name: 'Account', email: null, plan_type: null, enabled: true,
    credential_state: 'ready', supported: true, token_expires_ms: null, running: null,
    health: {
      level: 'ok', model: 'm', candidate: null, streak: 0, changed_ms: 100, last_run_id: 'confirmed',
      notification_pending: 0, notification_error: null, notifications_dropped: 0,
    },
    blocked: null,
    schedule: { turnstate: schedule, modeltrace: schedule },
    last_attempt: null,
    latest: { turnstate: null, modeltrace: null },
    strip: [{ i: 0, degraded: 3 }],
    rate: { judged: 0, ok: 0 }, rate_24h: { judged: 0, ok: 0 }, rate_7d: { judged: 0, ok: 0 },
  }
}

test('the confirmed verdict wins over raw strip results', () => {
  const a = account()
  assert.equal(accountState(a), 'ok')
  a.health!.level = 'degraded'
  assert.equal(accountState(a), 'degraded')
  a.running = 'in-progress'
  assert.equal(accountState(a), 'running')
})

test('without a turnstate verdict use the latest ModelTrace', () => {
  const a = account()
  a.health!.last_run_id = ''
  assert.equal(accountState(a), 'empty')
  a.latest.modeltrace = {
    id: 'l', model: 'm', trigger: 'manual', started_ms: 1, finished_ms: 2, status: 'done', level: 'suspect', reason: '', detail: true,
  }
  assert.equal(accountState(a), 'suspect')
})

test('pending change shows whenever the candidate differs from the verdict', () => {
  const a = account()
  a.health!.candidate = 'ok'
  a.health!.streak = 2
  assert.equal(pendingChange(a), null, 'already ok')
  a.health!.candidate = 'degraded'
  assert.deepEqual(pendingChange(a), { to: 'degraded', streak: 2 })
  assert.equal(pendingLabel('ok', 'degraded', 2), '转降智中 2/3')
  // 疑似 → 降智也会改变当前判定（不发通知），同样要显示
  a.health!.level = 'suspect'
  assert.deepEqual(pendingChange(a), { to: 'degraded', streak: 2 })
  a.health!.candidate = 'ok'
  assert.equal(pendingLabel('suspect', 'ok', 1), '恢复中 1/3')
  assert.equal(pendingLabel('unknown', 'ok', 1), '确认正常中 1/3')
})

test('problems sort first', () => {
  const order = (['ok', 'empty', 'degraded', 'suspect', 'unknown'] as const).slice().sort((x, y) => severity[x] - severity[y])
  assert.deepEqual(order, ['degraded', 'suspect', 'unknown', 'ok', 'empty'])
})

test('a confirmed block sorts after suspect and before unknown, without changing the verdict', () => {
  const a = account()
  assert.equal(rank(a), severity.ok)
  a.blocked = { code: 'credential_state', message: '凭据已失效', since_ms: 1, confirmed_ms: 0 }
  assert.equal(isBlocked(a), false, 'not confirmed yet')
  assert.equal(needsAttention(a), false)
  a.blocked.confirmed_ms = 2
  assert.equal(isBlocked(a), true)
  assert.equal(needsAttention(a), true)
  assert.equal(accountState(a), 'ok', 'blocking is not a verdict')
  assert(rank(a) > severity.suspect && rank(a) < severity.unknown)
  a.health!.level = 'degraded'
  assert.equal(rank(a), severity.degraded, 'degraded stays first')
  assert.equal(blockedLabel('token_expired'), '令牌过期')
})
