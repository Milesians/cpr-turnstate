import assert from 'node:assert/strict'
import { test } from 'node:test'
import { dateTime, day, hour } from '../src/format.ts'

// 2026-11-07 00:00 北京时间，即 2026-11-06 16:00 UTC：后端每天的计数从这一刻开始
const beijingMidnight = Date.UTC(2026, 10, 6, 16, 0, 0)

test('labels use Beijing time regardless of the browser timezone', () => {
  assert.equal(day(beijingMidnight), '11/7')
  assert.equal(hour(beijingMidnight), '00:00')
  assert.match(dateTime(beijingMidnight), /^11\/07 00:00:00$/)
})
