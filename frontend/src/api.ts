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
/** 只有测票能自动探测、在列表显示探针块；ModelTrace 只手动探测，记录在详情页 */
export const scheduledMethods: Method[] = ['turnstate']

export interface Blocker {
  code: string
  message: string
}

export interface Schedule {
  enabled: boolean
  model: string
  interval_minutes: number
}

export interface SchedulePatch {
  enabled?: boolean
  model?: string
  interval_minutes?: number
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

/** 一段时间里各结果的次数；为 0 的项不返回 */
export interface Counts {
  ok?: number
  suspect?: number
  degraded?: number
  /** 探测完成但没有判定 */
  unknown?: number
  failed?: number
  running?: number
}

/** 时间条的范围：从 start_ms 起 slots 格，每格 slot_ms（按北京时间对齐） */
export interface Range {
  hours: number
  start_ms: number
  slot_ms: number
  slots: number
}

/** 时间条里有探测的一格：第 i 格和它的次数；没列出的格子没有探测 */
export interface Cell extends Counts {
  i: number
}

export interface Rate {
  judged: number
  ok: number
}

export interface Health {
  level: Level
  model: string
  candidate: Level | null
  streak: number
  changed_ms: number
  last_run_id: string
  notification_pending: number
  notification_error: string | null
  notifications_dropped: number
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

/** 自动测票因为账号本身的问题（凭据失效、令牌过期等）发不出去；不是判定 */
export interface Blocked {
  code: string
  message: string
  since_ms: number
  /** 持续 30 分钟后确认（并发 TG 通知）的时间；0 表示还没确认 */
  confirmed_ms: number
}

/** 列表的一行：测票的当前判定、时间条与正常率，两种方法最近一次的结果 */
export interface OverviewAccount extends Account {
  health: Health | null
  blocked: Blocked | null
  schedule: Record<Method, Schedule>
  last_attempt: LastAttempt | null
  latest: Record<Method, Summary | null>
  strip: Cell[]
  rate: Rate
  rate_24h: Rate
  rate_7d: Rate
}

export interface Day extends Counts {
  start_ms: number
}

export interface MethodDetail {
  schedule: Schedule
  last_attempt: LastAttempt | null
  health: Health | null
  /** 最近的逐条记录，新的在前 */
  runs: Summary[]
  /** 30 天每天的次数，旧的在前 */
  daily: Day[]
  /** 更早的只剩次数时，逐条记录从这个时间开始 */
  raw_since: number | null
}

/** 一段时间里的记录；complete 为假表示有一部分只剩次数 */
export interface RunsPage {
  runs: Summary[]
  counts: Counts
  complete: boolean
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
  telegram_enabled: boolean
  telegram_configured: boolean
}

export interface Overview {
  now_ms: number
  range: Range
  config: Config
  bank: BankInfo
  accounts: OverviewAccount[]
}

export interface AccountDetail {
  account: Account & { methods: Record<Method, MethodDetail>, blocked: Blocked | null }
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

export interface GroupRule {
  /** 新规则没有 id；保存后由插件分配 */
  id?: string
  /** CPR 现有分组的 ID；没有就是插件新建的分组 */
  group_id?: string
  name: string
  color: string
  levels: Level[]
  /** 账号类型（套餐，小写；没有套餐是 unknown）；不填表示不限 */
  plans?: string[]
}

export interface GroupRuleStatus {
  group_id: string
  group_name: string
  error?: string
}

export interface GroupsView {
  enabled: boolean
  rules: GroupRule[]
  max_rules: number
  status: { synced_ms: number, error?: string, rules: Record<string, GroupRuleStatus>, changes: GroupChange[] }
  /** CPR 里可探测账号现有的账号类型 */
  plans: string[]
}

/** 一次按判定变化做的调整 */
export interface GroupChange {
  account_id: string
  from: Level
  to: Level
  at_ms: number
  added: string[]
  removed: string[]
}

export interface CprGroup {
  id: string
  name: string
  color: string
  enabled: boolean
  /** 插件新建的分组，不能选 */
  plugin: boolean
}

export const api = {
  overview: (hours = 24) => call<Overview>('GET', 'api/overview', { hours: String(hours) }),
  account: (id: string) => call<AccountDetail>('GET', 'api/account', { id }),
  run: (id: string, account: string) => call<{ run: Run }>('GET', 'api/run', { id, account }),
  /** 一段时间里的记录，新的在前 */
  runs: (account: string, method: Method, from: number, to: number) => call<RunsPage>('GET', 'api/runs', { account, method, from: String(from), to: String(to) }),
  models: () => call<Models>('GET', 'api/models'),
  plan: (account_id: string, method: Method, model: string) => call<Plan>('POST', 'api/plan', undefined, { account_id, method, model }),
  probe: (account_id: string, method: Method, model: string) => call<{ run: Run }>('POST', 'api/probe', undefined, { account_id, method, model }),
  step: (run_id: string, account_id: string) => call<{ run: Run }>('POST', 'api/probe/step', undefined, { run_id, account_id }),
  schedule: (account_id: string, method: Method, s: Schedule) => call<{ schedule: unknown }>('POST', 'api/schedule', undefined, { account_id, method, ...s }),
  batchSchedule: (account_ids: string[], method: Method, patch: SchedulePatch) => call<{ updated: number }>('POST', 'api/schedules/batch', undefined, { account_ids, method, ...patch }),
  syncBank: () => call<{ bank: BankInfo }>('POST', 'api/fingerprint/sync', undefined, {}),
  testTelegram: () => call<{ sent: boolean }>('POST', 'api/telegram/test', undefined, {}),
  groups: () => call<GroupsView>('GET', 'api/groups'),
  saveGroups: (enabled: boolean, rules: GroupRule[]) => call<GroupsView>('POST', 'api/groups', undefined, { enabled, rules }),
  cprGroups: () => call<{ groups: CprGroup[] }>('GET', 'api/groups/cpr'),
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
