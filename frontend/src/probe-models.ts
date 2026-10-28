import type { Method } from './api'

export const defaultModels: Readonly<Record<Method, string>> = {
  turnstate: 'gpt-6-luna',
  modeltrace: 'gpt-6.1-sol',
}

export function probeModels(saved: Partial<Record<Method, string>> = {}): Record<Method, string> {
  return {
    turnstate: saved.turnstate || defaultModels.turnstate,
    modeltrace: saved.modeltrace || defaultModels.modeltrace,
  }
}
