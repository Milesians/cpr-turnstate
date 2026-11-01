import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import { THEME_TOKEN_NAMES } from '@codex-proxy/ui/theme'

test('the host theme namespace matches the bundled UI library', () => {
  const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8')
  const prefix = html.match(/<meta\s+name="codex-proxy-ui-theme-prefix"\s+content="([a-z]+)"\s*\/?>/)?.[1]
  assert(prefix, 'CPR 3.37.1 defaults to z without an explicit theme prefix')
  assert(THEME_TOKEN_NAMES.every(name => name.startsWith(`--${prefix}-`)), 'The host must inject the tokens consumed by the bundled UI components')
})

test('the plugin viewport supports modern and legacy host bridges', () => {
  const css = readFileSync(new URL('../src/styles/index.css', import.meta.url), 'utf8')
  assert.match(css, /height:\s*var\(--plugin-viewport-height,\s*var\(--cp-plugin-viewport-height,\s*100dvh\)\)/)
})
