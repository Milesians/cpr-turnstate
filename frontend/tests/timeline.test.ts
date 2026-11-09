import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { OverviewAccount, Range } from '../src/api.ts'
import { accountState, blockedLabel, cellState, expand, fitSpans, isBlocked, MIN_PITCH, needsAttention, pendingChange, pendingLabel, rank, severity, stripLayout, total, worst } from '../src/timeline.ts'

const range: Range = { hours: 24, start_ms: 0, slot_ms: 900000, slots: 96 }

test('sparse cells expand to the full range in place', () => {
  const spans = expand(range, [{ i: 0, ok: 1 }, { i: 95, degraded: 2, running: 1 }])
  assert.equal(spans.length, 96)
  assert.equal(spans[0]!.start_ms, 0)
  assert.equal(spans.at(-1)!.end_ms, 24 * 3600000)
  assert.deepEqual(spans[95]!.counts, { degraded: 2, running: 1 })
  assert.equal(total(spans[1]!.counts), 0)
})

test('responsive cells keep the whole range and every count', () => {
  const spans = expand(range, [{ i: 0, ok: 1 }, { i: 95, degraded: 1 }])
  for (const width of [180, 480, 1200]) {
    const { count } = stripLayout(96, width)
    const out = fitSpans(spans, count)
    assert.equal(out.length, count)
    assert.equal(out[0]!.start_ms, 0)
    assert.equal(out.at(-1)!.end_ms, 24 * 3600000)
    assert.equal(out.reduce((n, s) => n + total(s.counts), 0), 2)
    assert.equal(cellState(out.at(-1)!.counts), 'degraded')
    assert(out.every((s, i) => !i || s.start_ms === out[i - 1]!.end_ms))
    // 每格是同样长的一段时间
    assert(out.every(s => s.end_ms - s.start_ms === out[0]!.end_ms - out[0]!.start_ms))
  }
})

test('strip layout picks an even split, snaps to device pixels and scales the gap', () => {
  for (const [slots, width, dpr] of [[96, 1170, 2], [96, 1170.4, 1], [96, 600, 1], [96, 300, 1.5], [84, 700, 1], [120, 500, 2], [120, 50, 1]] as const) {
    const { count, gap, widths } = stripLayout(slots, width, dpr)
    assert.equal(slots % count, 0, `${slots}@${width}`)
    assert(width / count >= MIN_PITCH || count === 1)
    // 再多一档就太挤
    const next = Array.from({ length: slots }, (_, i) => i + 1).find(n => n > count && slots % n === 0)
    if (next)
      assert(Math.floor(width * dpr) / next < MIN_PITCH * dpr)
    assert.equal(widths.length, count)
    const used = widths.reduce((a, b) => a + b, 0) + gap * (count - 1)
    assert(used <= width + 1e-9 && width - used < count / dpr + 1)
    for (const w of [...widths, gap])
      assert.equal(Math.abs(w * dpr - Math.round(w * dpr)) < 1e-9, true)
    assert(Math.max(...widths) - Math.min(...widths) <= 1 / dpr + 1e-9)
    assert(gap >= 1 / dpr && gap <= 3)
  }
  assert.equal(stripLayout(96, 1170, 2).count, 96)
  assert.equal(stripLayout(96, 1170, 2).gap, 3)
  assert.equal(stripLayout(96, 600, 1).count, 48)
  assert.equal(stripLayout(96, 300, 1).count, 32)
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
  assert.equal(cellState(fitSpans(spans, 1)[0]!.counts), 'ok')
  const worse = expand(range, [{ i: 0, ok: 1 }, { i: 2, degraded: 1 }])
  assert.equal(cellState(fitSpans(worse, 1)[0]!.counts), 'degraded')
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
