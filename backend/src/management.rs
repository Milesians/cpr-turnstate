//! 管理页面与接口（只有 CPR 的管理员能打开，页面在 CPR 的隔离 iframe 里）

use std::collections::HashMap;

use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    call::{
        host::{AuthRuntimeAccount, ModelListRequest, ModelListResult},
        management::{
            ManagementPage, ManagementRegistration, ManagementRequest, ManagementResource,
            ManagementResponse, ManagementRoute,
        },
    },
    client::{HostClient, TypedCall, TypedReply},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::App;
use crate::clock::now_ms;
use crate::host;
use crate::runner::{Blocker, Trigger, supported};
use crate::series::{self, Counts, HOUR_MS, Range};
use crate::store::{self, Method, MethodSchedule, Run, Status, Summary};

const JSON: &str = "application/json";
const MAX_BODY: usize = 64 * 1024;

pub fn registration() -> ManagementRegistration {
    let route = |method: &str, path: &str| ManagementRoute {
        method: method.to_owned(),
        path: path.to_owned(),
        request_content_types: if method == "GET" {
            Vec::new()
        } else {
            vec![JSON.to_owned()]
        },
        response_content_types: vec![JSON.to_owned()],
    };
    ManagementRegistration {
        routes: vec![
            route("GET", "api/overview"),
            route("GET", "api/account"),
            route("GET", "api/run"),
            route("GET", "api/runs"),
            route("GET", "api/models"),
            route("POST", "api/plan"),
            route("POST", "api/probe"),
            route("POST", "api/probe/step"),
            route("POST", "api/schedule"),
            route("POST", "api/schedules/batch"),
            route("POST", "api/fingerprint/sync"),
            route("POST", "api/telegram/test"),
            route("GET", "api/groups"),
            route("GET", "api/groups/cpr"),
            route("POST", "api/groups"),
        ],
        resources: ["web/index.html", "web/app.js", "web/app.css"]
            .into_iter()
            .map(|path| ManagementResource {
                path: path.to_owned(),
                public: false,
            })
            .collect(),
        pages: vec![ManagementPage {
            id: "account-probe".to_owned(),
            title: "账号探测".to_owned(),
            description: Some("ModelTrace 指纹与测票".to_owned()),
            entry: "web/index.html".to_owned(),
            icon: None,
        }],
        callbacks: Vec::new(),
    }
}

struct ApiError {
    status: u16,
    code: &'static str,
    message: String,
    blockers: Vec<Blocker>,
}

impl ApiError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            blockers: Vec::new(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, "invalid_request", message)
    }
}

impl From<PluginFault> for ApiError {
    fn from(error: PluginFault) -> Self {
        match error.code {
            ErrorCode::Conflict => Self::new(409, "conflict", "记录刚被修改过，请重试"),
            ErrorCode::Rejected => Self::new(404, "not_found", "账号不存在"),
            _ => Self::new(502, "host_callback", "宿主操作未完成，请稍后重试"),
        }
    }
}

type Reply = TypedReply<ManagementResponse>;

fn reply(status: u16, body: &impl Serialize) -> Result<Reply, PluginFault> {
    let payload =
        serde_json::to_vec(body).map_err(|_| PluginFault::new(ErrorCode::Fault, "响应编码失败"))?;
    Ok(TypedReply::new(ManagementResponse {
        headers: Vec::new(),
        status,
        content_type: JSON.to_owned(),
    })
    .with_payload(payload))
}

pub async fn handle(app: &App, call: TypedCall<ManagementRequest>) -> Result<Reply, PluginFault> {
    match route(app, &call).await {
        Ok(body) => reply(200, &body),
        Err(e) => reply(
            e.status,
            &json!({"error": {"code": e.code, "message": e.message, "blockers": e.blockers}}),
        ),
    }
}

async fn route(app: &App, call: &TypedCall<ManagementRequest>) -> Result<Value, ApiError> {
    if call.payload.len() > MAX_BODY {
        return Err(ApiError::invalid("请求正文太大"));
    }
    let host = &call.host;
    let query = parse_query(&call.request.query);
    match (call.request.method.as_str(), call.request.path.as_str()) {
        ("GET", "api/overview") => {
            let hours = query
                .get("hours")
                .map_or(Ok(24), |v| v.parse::<i64>())
                .map_err(|_| ApiError::invalid("时间范围无效"))?;
            let range = Range::new(hours, now_ms())
                .ok_or_else(|| ApiError::invalid("时间范围只支持 24 小时、7 天或 30 天"))?;
            overview(app, host, range).await
        }
        ("GET", "api/account") => account_detail(app, host, required(&query, "id")?).await,
        ("GET", "api/run") => {
            run_detail(host, required(&query, "id")?, required(&query, "account")?).await
        }
        ("GET", "api/runs") => {
            let method: Method =
                serde_json::from_value(Value::String(required(&query, "method")?.to_owned()))
                    .map_err(|_| ApiError::invalid("探测方法无效"))?;
            let bound = |name: &str| {
                required(&query, name)?
                    .parse::<i64>()
                    .map_err(|_| ApiError::invalid("时间范围无效"))
            };
            runs_between(
                host,
                required(&query, "account")?,
                method,
                bound("from")?,
                bound("to")?,
            )
            .await
        }
        ("GET", "api/models") => models(app, host).await,
        ("POST", "api/plan") => plan(app, host, decode(call)?).await,
        ("POST", "api/probe") => probe(app, host, decode(call)?).await,
        ("POST", "api/probe/step") => step(app, host, decode(call)?).await,
        ("POST", "api/schedule") => schedule(host, decode(call)?).await,
        ("POST", "api/schedules/batch") => batch_schedule(host, decode(call)?).await,
        ("POST", "api/fingerprint/sync") => {
            app.runner.bank.sync(host).await;
            Ok(json!({"bank": app.runner.bank.current().info}))
        }
        ("POST", "api/telegram/test") => {
            crate::telegram::send(
                host,
                &app.runner.config,
                "CPR 账号探测：TG 测试通知，连接成功。",
            )
            .await
            .map_err(|e| ApiError::new(502, "telegram_failed", e.message))?;
            Ok(json!({"sent": true}))
        }
        ("GET", "api/groups") => groups_view(app, host).await,
        ("GET", "api/groups/cpr") => cpr_groups(app, host).await,
        ("POST", "api/groups") => save_groups(app, host, decode(call)?).await,
        _ => Err(ApiError::new(404, "not_found", "没有这个接口")),
    }
}

async fn groups_view(app: &App, host: &HostClient) -> Result<Value, ApiError> {
    let (config, _) = store::group_config(host).await?;
    // 页面的账号类型选项：CPR 里可探测账号现有的套餐
    let plans: std::collections::BTreeSet<String> = host::openai_accounts(host)
        .await?
        .iter()
        .filter(|a| supported(a))
        .map(crate::groups::plan_of)
        .collect();
    Ok(json!({
        "enabled": config.enabled,
        "rules": config.rules,
        "max_rules": crate::groups::MAX_RULES,
        "status": app.groups.status(),
        "admin_error": crate::admin::base_url(&app.runner.config).err(),
        "plans": plans,
    }))
}

/// 页面下拉框用的 CPR 现有分组；插件自己维护成员的标成不可选
async fn cpr_groups(app: &App, host: &HostClient) -> Result<Value, ApiError> {
    let admin = crate::admin::Admin::new(host, &app.runner.config)
        .map_err(|reason| ApiError::new(409, "admin_not_configured", reason))?;
    let groups = admin
        .groups()
        .await
        .map_err(|reason| ApiError::new(502, "admin_failed", reason))?;
    let forbidden = app.groups.plugin_group_ids();
    let groups: Vec<Value> = groups
        .into_iter()
        .map(|g| {
            let plugin = forbidden.contains(&g.id);
            json!({"id": g.id, "name": g.name, "color": g.color, "enabled": g.enabled, "plugin": plugin})
        })
        .collect();
    Ok(json!({"groups": groups}))
}

async fn save_groups(
    app: &App,
    host: &HostClient,
    input: crate::groups::ConfigInput,
) -> Result<Value, ApiError> {
    // 选了现有分组时先读 CPR 的分组，核对分组存在
    let groups = if input
        .rules
        .iter()
        .any(|r| r.group_id.as_deref().is_some_and(|g| !g.is_empty()))
    {
        let admin =
            crate::admin::Admin::new(host, &app.runner.config).map_err(ApiError::invalid)?;
        Some(
            admin
                .groups()
                .await
                .map_err(|reason| ApiError::new(502, "admin_failed", reason))?,
        )
    } else {
        None
    };
    let forbidden = app.groups.plugin_group_ids();
    let existing = crate::groups::Existing {
        groups: groups.as_deref(),
        forbidden: &forbidden,
    };
    let mut was_enabled = false;
    store::update_group_config(host, |config| {
        was_enabled = config.enabled;
        *config = crate::groups::apply_input(config, input.clone(), &existing)?;
        Ok(())
    })
    .await?
    .map_err(ApiError::invalid)?;
    // 保存不改任何成员；从关闭切到开启时按当前判定重新记起点
    let accounts = host::openai_accounts(host).await?;
    app.groups
        .after_save(&app.runner, host, &accounts, input.enabled && !was_enabled)
        .await?;
    groups_view(app, host).await
}

fn parse_query(query: &str) -> HashMap<String, String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_owned(), percent_decode(v)))
        .collect()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match std::str::from_utf8(&bytes[i + 1..i + 3])
                    .ok()
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn required<'a>(query: &'a HashMap<String, String>, name: &str) -> Result<&'a str, ApiError> {
    query
        .get(name)
        .map(String::as_str)
        .filter(|v| !v.is_empty() && v.len() <= 256)
        .ok_or_else(|| ApiError::invalid(format!("缺少参数 {name}")))
}

fn decode<T: for<'de> Deserialize<'de>>(
    call: &TypedCall<ManagementRequest>,
) -> Result<T, ApiError> {
    let json = call
        .request
        .content_type
        .as_deref()
        .and_then(|c| c.split(';').next())
        .is_some_and(|c| c.trim().eq_ignore_ascii_case(JSON));
    if !json {
        return Err(ApiError::invalid("需要 application/json"));
    }
    serde_json::from_slice(&call.payload).map_err(|_| ApiError::invalid("请求正文不是有效的 JSON"))
}

// ---------------------------------------------------------------------------
// 账号列表
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct AccountView {
    id: String,
    name: String,
    email: Option<String>,
    plan_type: Option<String>,
    enabled: bool,
    credential_state: String,
    supported: bool,
    token_expires_ms: Option<i64>,
    running: Option<String>,
}

impl AccountView {
    fn new(app: &App, a: &AuthRuntimeAccount) -> Self {
        Self {
            id: a.account_id.clone(),
            name: a.name.clone(),
            email: a.email.clone(),
            plan_type: a.plan_type.clone(),
            enabled: a.enabled,
            credential_state: a.credential_state.clone(),
            supported: supported(a),
            token_expires_ms: a.access_token_expires_at_ms,
            running: app.runner.running(&a.account_id),
        }
    }
}

#[derive(Serialize, Default)]
struct Rate {
    judged: u32,
    ok: u32,
}

impl From<Counts> for Rate {
    fn from(c: Counts) -> Self {
        Self {
            judged: c.judged(),
            ok: c.ok,
        }
    }
}

fn latest(runs: &[Summary]) -> Option<&Summary> {
    runs.iter().rev().find(|s| s.status != Status::Running)
}

/// 列表：每个账号一行，带测票的当前判定、时间条和正常率，以及两种方法最近一次的结果。
///
/// 时间条只列出有探测的格子和各结果的次数（格子位置由 `range` 算），记录在打开格子时用
/// `api/runs` 再取；这样账号再多、范围再长，正文也远在宿主管理响应 1 MiB 的上限以内
async fn overview(app: &App, host: &HostClient, range: Range) -> Result<Value, ApiError> {
    let accounts = host::openai_accounts(host).await?;
    let (schedules, _) = store::schedules(host).await?;
    let now = now_ms();
    let mut views = Vec::with_capacity(accounts.len());
    for a in &accounts {
        let schedule = schedules
            .accounts
            .get(&a.account_id)
            .cloned()
            .unwrap_or_default();
        let (turnstate, modeltrace) = if supported(a) {
            (
                store::history_view(host, &a.account_id, Method::Turnstate).await?,
                store::history_view(host, &a.account_id, Method::Modeltrace).await?,
            )
        } else {
            Default::default()
        };
        let t = &turnstate;
        let rate_since =
            |from: i64| Rate::from(series::between(from, i64::MAX, &t.runs, &t.buckets));
        let mut view = serde_json::to_value(AccountView::new(app, a)).unwrap_or(Value::Null);
        view["health"] = json!(turnstate.health);
        view["schedule"] = json!(schedule);
        view["last_attempt"] = json!(app.scheduler.last(&a.account_id, Method::Turnstate));
        view["latest"] = json!({
            "turnstate": latest(&turnstate.runs),
            "modeltrace": latest(&modeltrace.runs),
        });
        view["strip"] = json!(series::strip(&range, &t.runs, &t.buckets));
        view["rate"] = json!(rate_since(range.start_ms));
        view["rate_24h"] = json!(rate_since(now - 24 * HOUR_MS));
        view["rate_7d"] = json!(rate_since(now - 7 * 24 * HOUR_MS));
        views.push(view);
    }
    Ok(json!({
        "now_ms": now,
        "range": range,
        "config": {
            "probe_enabled": app.runner.config.probe_enabled,
            "min_interval_minutes": app.runner.config.min_interval_minutes,
            "fingerprint_sync_hours": app.runner.config.fingerprint_sync_hours,
            "telegram_enabled": app.runner.config.telegram_enabled,
            "telegram_configured": crate::telegram::configuration_error(&app.runner.config).is_none(),
        },
        "bank": app.runner.bank.current().info,
        "accounts": views,
    }))
}

/// 时间条一格（或合并后的几格）里的记录，新的在前。更早的只剩按小时汇总的次数，
/// `complete` 为假表示这段时间有一部分只有次数、没有逐条记录
async fn runs_between(
    host: &HostClient,
    account_id: &str,
    method: Method,
    from: i64,
    to: i64,
) -> Result<Value, ApiError> {
    if from >= to {
        return Err(ApiError::invalid("时间范围无效"));
    }
    supported_account(host, account_id).await?;
    let view = store::history_view(host, account_id, method).await?;
    let mut runs: Vec<&Summary> = view
        .runs
        .iter()
        .filter(|s| s.started_ms >= from && s.started_ms < to)
        .collect();
    runs.reverse();
    Ok(json!({
        "runs": runs,
        "counts": series::between(from, to, &view.runs, &view.buckets),
        "complete": !series::has_buckets(from, to, &view.buckets),
    }))
}

/// 账号详情：每种方法的设置、当前判定、最近的逐条记录（新的在前）和 30 天每天的次数
async fn account_detail(app: &App, host: &HostClient, id: &str) -> Result<Value, ApiError> {
    let account = host::account(host, id).await?;
    if account.provider_id != "openai" {
        return Err(ApiError::new(404, "not_found", "账号不存在"));
    }
    let (schedules, _) = store::schedules(host).await?;
    let schedule = schedules.accounts.get(id).cloned().unwrap_or_default();
    let now = now_ms();
    let mut methods = serde_json::Map::new();
    for method in Method::ALL {
        let view = store::history_view(host, id, method).await?;
        let daily = series::daily(store::HISTORY_DAYS, now, &view.runs, &view.buckets);
        // 更早的只剩次数：页面据此说明逐条记录从什么时候开始
        let raw_since = (!view.buckets.is_empty())
            .then(|| view.runs.first().map(|s| s.started_ms))
            .flatten();
        let runs: Vec<&Summary> = view.runs.iter().rev().collect();
        methods.insert(
            method.as_str().into(),
            json!({
                "schedule": schedule.get(method),
                "last_attempt": app.scheduler.last(id, method),
                "health": view.health,
                "runs": runs,
                "daily": daily,
                "raw_since": raw_since,
            }),
        );
    }
    let mut view = serde_json::to_value(AccountView::new(app, &account)).unwrap_or(Value::Null);
    view["methods"] = Value::Object(methods);
    Ok(json!({"account": view, "config": {"probe_enabled": app.runner.config.probe_enabled}}))
}

/// 进行中的 ModelTrace 不把题目和回答原文发给页面
fn public_run(mut run: Run) -> Value {
    let progress = run
        .pending
        .take()
        .map(|p| json!({"answered": p.completions.len(), "total": p.challenges.len()}));
    let mut value = serde_json::to_value(run).unwrap_or(Value::Null);
    if let Some(progress) = progress {
        value["progress"] = progress;
    }
    value
}

async fn run_detail(host: &HostClient, id: &str, account: &str) -> Result<Value, ApiError> {
    match store::run(host, id).await? {
        Some((run, _)) if run.account_id == account => Ok(json!({"run": public_run(run)})),
        _ => Err(ApiError::new(
            404,
            "not_found",
            "没有这条记录，或它的详情已经清理",
        )),
    }
}

async fn models(app: &App, host: &HostClient) -> Result<Value, ApiError> {
    let current = app.runner.bank.current();
    let bank: Vec<Value> = current
        .bank
        .models
        .iter()
        .map(|m| json!({"id": m.id, "display_name": m.display_name, "family": m.family_name}))
        .collect();
    // 经 CPR 能请求的模型（ModelTrace 走 CPR 的模型链，名字以它为准）；失败不影响其他
    let cpr = match app.runner.client_key() {
        Ok(key) => host
            .call(
                "host.models.list",
                serde_json::to_value(ModelListRequest {
                    client_key_id: key,
                    protocol: "openai".into(),
                    client_version: "cpr-account-probe/0.1.0".into(),
                })
                .unwrap_or(Value::Null),
                Vec::new(),
            )
            .await
            .ok()
            .and_then(|r| serde_json::from_value::<ModelListResult>(r.result).ok())
            .map(|r| r.models),
        Err(_) => None,
    };
    Ok(json!({"bank": bank, "cpr": cpr, "aliases": {"gpt-6.1-sol": "gpt-6-astra"}}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeRequest {
    account_id: String,
    method: Method,
    model: String,
}

async fn supported_account(host: &HostClient, id: &str) -> Result<AuthRuntimeAccount, ApiError> {
    let account = host::account(host, id).await?;
    if account.provider_id != "openai" {
        return Err(ApiError::new(404, "not_found", "账号不存在"));
    }
    Ok(account)
}

/// 预览：现在会不会被拦截、会发什么。什么也不发送
async fn plan(app: &App, host: &HostClient, req: ProbeRequest) -> Result<Value, ApiError> {
    let account = supported_account(host, &req.account_id).await?;
    let blockers = app.runner.blockers(&account, req.method, &req.model);
    let request = match req.method {
        Method::Turnstate => {
            let identity = host::codex_identity(host).await.map_err(|_| {
                ApiError::new(502, "host_identity", "读取 CPR 的 Codex 客户端身份失败")
            })?;
            json!({
                "path": "直连 Codex 后端，经账号在 CPR 绑定的代理（没绑就直连）；代理不可用时失败，不改为直连",
                "method": "POST",
                "url": crate::codex::ENDPOINT,
                "headers": {
                    "Authorization": "Bearer ‹hidden›",
                    "Chatgpt-Account-Id": "‹hidden›",
                    "Originator": identity.originator,
                    "User-Agent": identity.user_agent,
                    "Version": identity.codex_version,
                    "Session_id": "‹随机，同一次探测内不变›",
                    crate::codex::TURN_STATE_HEADER: "‹上一次响应里的票，第一次不带›",
                },
                "body": {"model": req.model, "input": crate::turnstate::ECHO_PROMPT, "reasoning_effort": "low", "stream": true, "store": false},
                "count": format!("1 次拿票 + 最多 {} 次带回", crate::turnstate::ECHO_ATTEMPTS),
            })
        }
        Method::Modeltrace => json!({
            "path": "经 CPR 自己的模型链（指定这个账号），用插件设置里配置的 Client Key，计入它的用量",
            "body": {"model": req.model, "input": "‹一道“选 N 个 1 到 355 的整数”的题›", "store": false},
            "count": format!("{} 道题，依次发送", crate::modeltrace::CHALLENGE_COUNT),
            "fingerprint": crate::fingerprint::fingerprint_of(&req.model),
            "in_bank": app.runner.bank.current().bank.has_model(crate::fingerprint::fingerprint_of(&req.model)),
        }),
    };
    Ok(json!({"blockers": blockers, "request": request}))
}

async fn probe(app: &App, host: &HostClient, req: ProbeRequest) -> Result<Value, ApiError> {
    let account = supported_account(host, &req.account_id).await?;
    match app
        .runner
        .start(host, &account, req.method, &req.model, Trigger::Manual)
        .await?
    {
        Ok(run) => {
            app.scheduler
                .note_started(&run.account_id, run.method, run.started_ms);
            Ok(json!({"run": public_run(run)}))
        }
        Err(blockers) => Err(ApiError {
            status: 409,
            code: "blocked",
            message: blockers
                .iter()
                .map(|b| b.message.as_str())
                .collect::<Vec<_>>()
                .join("；"),
            blockers,
        }),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepRequest {
    run_id: String,
    account_id: String,
}

async fn step(app: &App, host: &HostClient, req: StepRequest) -> Result<Value, ApiError> {
    let Some((run, version)) = store::run(host, &req.run_id).await? else {
        return Err(ApiError::new(404, "not_found", "没有这条记录"));
    };
    if run.account_id != req.account_id {
        return Err(ApiError::new(404, "not_found", "没有这条记录"));
    }
    let run = app.runner.advance(host, run, version).await?;
    Ok(json!({"run": public_run(run)}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleRequest {
    account_id: String,
    method: Method,
    enabled: bool,
    model: String,
    interval_minutes: u32,
}

async fn schedule(host: &HostClient, req: ScheduleRequest) -> Result<Value, ApiError> {
    supported_account(host, &req.account_id).await?;
    if !req.model.is_empty() && !crate::codex::valid_model(&req.model) {
        return Err(ApiError::invalid("模型名无效"));
    }
    if req.enabled && !req.method.schedulable() {
        return Err(ApiError::invalid("ModelTrace 只支持手动探测"));
    }
    if req.enabled && req.model.is_empty() {
        return Err(ApiError::invalid("开启自动探测前先选择模型"));
    }
    if !(1..=10_080).contains(&req.interval_minutes) {
        return Err(ApiError::invalid("间隔必须在 1 分钟到 7 天之间"));
    }
    let saved = store::update_schedule(
        host,
        &req.account_id,
        req.method,
        MethodSchedule {
            enabled: req.enabled,
            model: req.model,
            interval_minutes: req.interval_minutes,
        },
    )
    .await?;
    Ok(json!({"schedule": saved}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchScheduleRequest {
    account_ids: Vec<String>,
    method: Method,
    enabled: Option<bool>,
    model: Option<String>,
    interval_minutes: Option<u32>,
}

async fn batch_schedule(host: &HostClient, req: BatchScheduleRequest) -> Result<Value, ApiError> {
    if req.account_ids.is_empty() || req.account_ids.len() > 200 {
        return Err(ApiError::invalid("每次请选择 1 到 200 个账号"));
    }
    let ids: std::collections::BTreeSet<_> = req.account_ids.iter().collect();
    if ids.len() != req.account_ids.len() || ids.iter().any(|id| id.is_empty() || id.len() > 256) {
        return Err(ApiError::invalid("账号列表无效或有重复"));
    }
    let patch = store::SchedulePatch {
        enabled: req.enabled,
        model: req.model,
        interval_minutes: req.interval_minutes,
    };
    if patch.enabled == Some(true) && !req.method.schedulable() {
        return Err(ApiError::invalid("ModelTrace 只支持手动探测"));
    }
    if patch.enabled.is_none() && patch.model.is_none() && patch.interval_minutes.is_none() {
        return Err(ApiError::invalid("请至少选择一项要修改的配置"));
    }
    if patch
        .model
        .as_deref()
        .is_some_and(|m| !crate::codex::valid_model(m))
    {
        return Err(ApiError::invalid("模型名无效"));
    }
    if patch
        .interval_minutes
        .is_some_and(|i| !(1..=10_080).contains(&i))
    {
        return Err(ApiError::invalid("间隔必须在 1 分钟到 7 天之间"));
    }
    let accounts = host::openai_accounts(host).await?;
    if ids.iter().any(|id| {
        !accounts
            .iter()
            .any(|a| &a.account_id == *id && supported(a))
    }) {
        return Err(ApiError::invalid(
            "所选账号已不存在或不支持探测，请刷新列表",
        ));
    }
    store::update_schedules(host, &req.account_ids, req.method, &patch).await?;
    Ok(json!({"updated": req.account_ids.len()}))
}
