import assert from 'node:assert/strict'
import { test } from 'node:test'
import type { Slot } from '../src/api.ts'
import { cellState, fitSlots } from '../src/timeline.ts'

function slots(): Slot[] {
  return Array.from({ length: 96 }, (_, i) => ({ start_ms: i * 900000, end_ms: (i + 1) * 900000, level: null, runs: [] }))
}
function run(status: 'done' | 'failed' | 'running', level: 'ok' | 'degraded' | null) {
  return { id: 'run', started_ms: 1, model: 'gpt-test', status, level, reason: '', detail: true }
}

test('responsive cells preserve the entire time range and every record', () => {
  const input = slots()
  input[0]!.level = 'ok'
  input[0]!.runs = [run('done', 'ok')]
  input[95]!.level = 'degraded'
  input[95]!.runs = [run('done', 'degraded')]
  for (const width of [180, 480, 1200]) {
    const output = fitSlots(input, width)
    assert.equal(output.length, Math.min(96, Math.floor(width / 11)))
    assert.equal(output[0]!.start_ms, 0)
    assert.equal(output.at(-1)!.end_ms, 24 * 3600000)
    assert.equal(output.flatMap(s => s.runs).length, 2)
    assert.equal(cellState(output.at(-1)!), 'degraded')
    assert(output.every((s, i) => !i || s.start_ms === output[i - 1]!.end_ms))
  }
})

test('no probe, in progress, failed and unknown are different states', () => {
  const empty = slots()[0]!
  assert.equal(cellState(empty), 'empty')
  assert.equal(cellState({ ...empty, runs: [run('running', null)] }), 'running')
  assert.equal(cellState({ ...empty, runs: [run('failed', null)] }), 'unknown')
  assert.equal(cellState({ ...empty, level: 'unknown', runs: [run('done', null)] }), 'unknown')
})

test('empty time does not replace real results or hide a worse verdict', () => {
  const input = slots()
  input[0]!.level = 'ok'; input[0]!.runs = [run('done', 'ok')]
  input[1]!.level = 'unknown'; input[1]!.runs = [run('failed', null)]
  assert.equal(fitSlots(input, 11)[0]!.level, 'ok')
  input[2]!.level = 'degraded'; input[2]!.runs = [run('done', 'degraded')]
  assert.equal(fitSlots(input, 11)[0]!.level, 'degraded')
})
