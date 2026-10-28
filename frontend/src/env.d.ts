import type { PluginHost } from './api'

declare global {
  interface Window {
    readonly codexProxyPlugin?: PluginHost
  }
}
