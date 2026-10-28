import assert from 'node:assert/strict'
import { test } from 'node:test'
import { probeModels } from '../src/probe-models.ts'

test('each method has its own default when an account has no model', () => {
  assert.deepEqual(probeModels(), { turnstate: 'gpt-6-luna', modeltrace: 'gpt-6.1-sol' })
  assert.deepEqual(probeModels({ turnstate: '', modeltrace: '' }), probeModels())
})
test('existing account choices are retained independently', () => {
  assert.deepEqual(probeModels({ turnstate: 'ticket-custom' }), { turnstate: 'ticket-custom', modeltrace: 'gpt-6.1-sol' })
  assert.deepEqual(probeModels({ modeltrace: 'trace-custom' }), { turnstate: 'gpt-6-luna', modeltrace: 'trace-custom' })
})
