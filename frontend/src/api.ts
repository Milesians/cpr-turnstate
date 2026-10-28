// 页面只经宿主桥访问插件自己注册的管理路由，不碰 CPR 的管理接口、Cookie 或 Key 原文

export interface ManagementReply {
  status: number
  contentType: string
  body: ArrayBuffer
}

export interface PluginHost {
  readonly version: 2
  readonly theme: 'light' | 'dark'
  request: (input: {
    method: 'GET' | 'POST'
    path: string
    query?: string
    contentType?: string
    body?: string
  }) => Promise<ManagementReply>
}

export type Method = 'turnstate' | 'modeltrace'
export type Level = 'unknown' | 'ok' | 'suspect' | 'degraded'
export type Status = 'running' | 'done' | 'failed'

export const methods: Method[] = ['turnstate', 'modeltrace']
export const methodLabel: Record<Method, string> = { turnstate: '测票', modeltrace: 'ModelTrace' }

export interface Blocker {
  code: string
  message: string
}

export interface Schedule {
  enabled: boolean
  model: string
  interval_minutes: number
}

export interface LastAttempt {
  at_ms: number
  started: boolean
  blockers?: Blocker[]
}

export interface Summary {
  id: string
  model: string
  trigger: string
  started_ms: number
  finished_ms: number
  status: Status
  level?: Level
  reason: string
  detail: boolean
}

export interface SlotRun {
  id: string
  started_ms: number
  model: string
  status: Status
  level: Level | null
  reason: string
  detail: boolean
}

export interface Slot {
  start_ms: number
  end_ms: number
  level: Level | null
  runs: SlotRun[]
}

export interface Rate {
  judged: number
  ok: number
}

export interface MethodView {
  schedule: Schedule
  last_attempt: LastAttempt | null
  latest: Summary | null
  slots: Slot[]
  rate: Rate
  rate_24h: Rate
  rate_7d: Rate
}

export interface Account {
  id: string
  name: string
  email: string | null
  plan_type: string | null
  enabled: boolean
  credential_state: string
  supported: boolean
  token_expires_ms: number | null
  running: string | null
}

export interface OverviewAccount extends Account {
  methods: Record<Method, MethodView>
}

export interface BankInfo {
  commit: string
  source: 'embedded' | 'github'
  committed_at?: string
  models: string[]
  checked_ms: number
  error?: string
}

export interface Config {
  probe_enabled: boolean
  min_interval_minutes: number
  fingerprint_sync_hours: number
}

export interface Overview {
  now_ms: number
  range_hours: number
  config: Config
  bank: BankInfo
  accounts: OverviewAccount[]
}

export interface AccountDetail {
  account: Account & {
    methods: Record<Method, { schedule: Schedule, last_attempt: LastAttempt | null, runs: Summary[] }>
  }
  config: { probe_enabled: boolean }
}

export interface TurnStateStep {
  shot: number
  outcome: string
  sent_length?: number
  sent_fingerprint?: string
  got_length?: number
  got_fingerprint?: string
  duration_ms: number
}

export interface ModelTraceOutput {
  expected_count: number
  parsed_numbers: number
  accepted: boolean
  output_tokens: number
  reasoning_tokens: number
  duration_ms: number
  response_model?: string
  excerpt: string
}

export interface Candidate {
  model: string
  display_name: string
  family: string
  probability: number
}

export interface Run {
  id: string
  account_id: string
  method: Method
  model: string
  trigger: string
  started_ms: number
  finished_ms: number
  status: Status
  verdict?: { level: Level, reason: string }
  error?: { status?: number, kind: string, message: string }
  turnstate?: { model: string, steps: TurnStateStep[], minted_length: number, echoes: number, refreshed: number, accepted_at: number }
  modeltrace?: {
    model: string
    fingerprint?: string
    in_bank: boolean
    bank_commit: string
    used_outputs: number
    outputs: ModelTraceOutput[]
    prediction?: string
    prediction_name?: string
    probability: number
    probability_of_probed: number
    candidates: Candidate[]
  }
  progress?: { answered: number, total: number }
}

export interface Plan {
  blockers: Blocker[]
  request: Record<string, unknown>
}

export interface Models {
  bank: { id: string, display_name: string, family: string }[]
  cpr: string[] | null
  aliases: Record<string, string>
}

export class ApiError extends Error {
  constructor(message: string, readonly status: number, readonly blockers: Blocker[] = []) {
    super(message)
  }
}

function host(): PluginHost {
  const h = window.codexProxyPlugin
  if (!h || h.version !== 2)
    throw new Error('请从 CPR 的插件管理页面打开')
  return h
}

async function call<T>(method: 'GET' | 'POST', path: string, query?: Record<string, string>, data?: unknown): Promise<T> {
  const reply = await host().request({
    method,
    path,
    ...(query ? { query: new URLSearchParams(query).toString() } : {}),
    ...(data === undefined ? {} : { contentType: 'application/json', body: JSON.stringify(data) }),
  })
  let value: any
  try {
    value = JSON.parse(new TextDecoder().decode(reply.body))
  }
  catch {
    throw new ApiError(`插件返回了无效响应（HTTP ${reply.status}）`, reply.status)
  }
  if (reply.status < 200 || reply.status >= 300) {
    const e = value?.error
    throw new ApiError(typeof e?.message === 'string' ? e.message : `请求失败（HTTP ${reply.status}）`, reply.status, Array.isArray(e?.blockers) ? e.blockers : [])
  }
  return value as T
}

export const api = {
  overview: (hours = 24) => call<Overview>('GET', 'api/overview', { hours: String(hours) }),
  account: (id: string) => call<AccountDetail>('GET', 'api/account', { id }),
  run: (id: string, account: string) => call<{ run: Run }>('GET', 'api/run', { id, account }),
  models: () => call<Models>('GET', 'api/models'),
  plan: (account_id: string, method: Method, model: string) => call<Plan>('POST', 'api/plan', undefined, { account_id, method, model }),
  probe: (account_id: string, method: Method, model: string) => call<{ run: Run }>('POST', 'api/probe', undefined, { account_id, method, model }),
  step: (run_id: string, account_id: string) => call<{ run: Run }>('POST', 'api/probe/step', undefined, { run_id, account_id }),
  schedule: (account_id: string, method: Method, s: Schedule) => call<{ schedule: unknown }>('POST', 'api/schedule', undefined, { account_id, method, ...s }),
  syncBank: () => call<{ bank: BankInfo }>('POST', 'api/fingerprint/sync', undefined, {}),
}

/** 发起一次探测；ModelTrace 由页面逐题推进，直到完成 */
export async function runProbe(accountId: string, method: Method, model: string, onProgress: (run: Run) => void): Promise<Run> {
  let { run } = await api.probe(accountId, method, model)
  onProgress(run)
  while (run.status === 'running') {
    run = (await api.step(run.id, accountId)).run
    onProgress(run)
  }
  return run
}
