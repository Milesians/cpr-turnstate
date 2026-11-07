import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { OverviewAccount, Slot } from '../src/api.ts'
import { accountState, cellState, fitSlots } from '../src/timeline.ts'

function slots(): Slot[] {
  return Array.from({ length: 96 }, (_, i) => ({ start_ms: i * 900000, end_ms: (i + 1) * 900000, level: null, count: 0, running: 0 }))
}

test('responsive cells preserve the entire time range and every count', () => {
  const input = slots()
  input[0]!.level = 'ok'
  input[0]!.count = 1
  input[95]!.level = 'degraded'
  input[95]!.count = 1
  for (const width of [180, 480, 1200]) {
    const output = fitSlots(input, width)
    assert.equal(output.length, Math.min(96, Math.floor(width / 11)))
    assert.equal(output[0]!.start_ms, 0)
    assert.equal(output.at(-1)!.end_ms, 24 * 3600000)
    assert.equal(output.reduce((n, s) => n + s.count, 0), 2)
    assert.equal(cellState(output.at(-1)!), 'degraded')
    assert(output.every((s, i) => !i || s.start_ms === output[i - 1]!.end_ms))
  }
})

test('no probe, in progress, failed and unknown are different states', () => {
  const empty = slots()[0]!
  assert.equal(cellState(empty), 'empty')
  assert.equal(cellState({ ...empty, count: 1, running: 1 }), 'running')
  assert.equal(cellState({ ...empty, count: 2, running: 1 }), 'unknown')
  assert.equal(cellState({ ...empty, level: 'unknown', count: 1 }), 'unknown')
  assert.equal(cellState({ ...empty, level: 'ok', count: 2, running: 1 }), 'ok')
})

test('empty time does not replace real results or hide a worse verdict', () => {
  const input = slots()
  input[0]!.level = 'ok'; input[0]!.count = 1
  input[1]!.level = 'unknown'; input[1]!.count = 1
  assert.equal(fitSlots(input, 11)[0]!.level, 'ok')
  input[2]!.level = 'degraded'; input[2]!.count = 1
  assert.equal(fitSlots(input, 11)[0]!.level, 'degraded')
})

function monitoredAccount(): OverviewAccount {
  const method = () => ({
    schedule: { enabled: true, model: 'test-model', interval_minutes: 30 },
    last_attempt: null, latest: null, slots: slots(),
    rate: { judged: 0, ok: 0 }, rate_24h: { judged: 0, ok: 0 }, rate_7d: { judged: 0, ok: 0 },
  })
  return {
    id: 'a', name: 'Account', email: null, plan_type: null, enabled: true,
    credential_state: 'ready', supported: true, token_expires_ms: null, running: null,
    methods: {
      turnstate: { ...method(), health: {
        level: 'ok', model: 'test-model', candidate: null, streak: 0,
        changed_ms: 100, last_run_id: 'confirmed', notification_pending: 0,
        notification_error: null, notifications_dropped: 0,
      } },
      modeltrace: method(),
    },
  }
}

test('confirmed recovery overrides old degraded slots and is independent of time range', () => {
  const account = monitoredAccount()
  for (const method of Object.values(account.methods)) {
    method.slots[0]!.level = 'degraded'
    method.slots[0]!.count = 1
  }
  assert.equal(accountState(account), 'ok')
  account.methods.turnstate.slots = slots()
  account.methods.modeltrace.slots = slots()
  assert.equal(accountState(account), 'ok')
})

test('unconfirmed recovery and regression keep the previous verdict', () => {
  const account = monitoredAccount()
  const health = account.methods.turnstate.health!
  health.level = 'degraded'; health.candidate = 'ok'; health.streak = 2
  assert.equal(accountState(account), 'degraded')
  health.level = 'ok'; health.candidate = 'suspect'; health.streak = 2
  assert.equal(accountState(account), 'ok')
  health.level = 'unknown'
  assert.equal(accountState(account), 'unknown')
  account.running = 'in-progress'
  assert.equal(accountState(account), 'running')
})

test('without any turnstate result use latest ModelTrace rather than its worst historical slot', () => {
  const account = monitoredAccount()
  account.methods.turnstate.health!.last_run_id = ''
  account.methods.turnstate.health!.level = 'unknown'
  assert.equal(accountState(account), 'empty')
  account.methods.modeltrace.latest = {
    id: 'latest', model: 'modeltrace-model', trigger: 'manual', started_ms: 1, finished_ms: 2,
    status: 'done', level: 'ok', reason: 'normal', detail: true,
  }
  account.methods.modeltrace.slots[0]!.level = 'degraded'
  account.methods.modeltrace.slots[0]!.count = 1
  assert.equal(accountState(account), 'ok')
})
