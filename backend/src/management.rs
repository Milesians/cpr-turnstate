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
use crate::scheduler::LastAttempt;
use crate::store::{self, Method, MethodSchedule, Run, Status, Summary};
use crate::verdict::Level;

const JSON: &str = "application/json";
const MAX_BODY: usize = 64 * 1024;
const HOUR_MS: i64 = 3_600_000;

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
            route("GET", "api/models"),
            route("POST", "api/plan"),
            route("POST", "api/probe"),
            route("POST", "api/probe/step"),
            route("POST", "api/schedule"),
            route("POST", "api/fingerprint/sync"),
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
            if ![24, 168, 720].contains(&hours) {
                return Err(ApiError::invalid("时间范围只支持 24 小时、7 天或 30 天"));
            }
            overview(app, host, hours).await
        }
        ("GET", "api/account") => account_detail(app, host, required(&query, "id")?).await,
        ("GET", "api/run") => {
            run_detail(host, required(&query, "id")?, required(&query, "account")?).await
        }
        ("GET", "api/models") => models(app, host).await,
        ("POST", "api/plan") => plan(app, host, decode(call)?).await,
        ("POST", "api/probe") => probe(app, host, decode(call)?).await,
        ("POST", "api/probe/step") => step(app, host, decode(call)?).await,
        ("POST", "api/schedule") => schedule(host, decode(call)?).await,
        ("POST", "api/fingerprint/sync") => {
            app.runner.bank.sync(host).await;
            Ok(json!({"bank": app.runner.bank.current().info}))
        }
        _ => Err(ApiError::new(404, "not_found", "没有这个接口")),
    }
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

#[derive(Serialize)]
struct SlotRun {
    id: String,
    started_ms: i64,
    model: String,
    status: Status,
    level: Option<Level>,
    reason: String,
    detail: bool,
}

#[derive(Serialize)]
struct Slot {
    start_ms: i64,
    end_ms: i64,
    /// 最严重的判定；探测了但都没判定（失败）是 unknown，没探测为空
    level: Option<Level>,
    runs: Vec<SlotRun>,
}

#[derive(Serialize, Default)]
struct Rate {
    judged: u32,
    ok: u32,
}

#[derive(Serialize)]
struct MethodView {
    schedule: MethodSchedule,
    last_attempt: Option<LastAttempt>,
    latest: Option<Summary>,
    slots: Vec<Slot>,
    rate: Rate,
    rate_24h: Rate,
    rate_7d: Rate,
}

fn judged_level(s: &Summary) -> Option<Level> {
    match s.status {
        Status::Running => None,
        Status::Failed => Some(Level::Unknown),
        Status::Done => Some(s.level.unwrap_or(Level::Unknown)),
    }
}

fn rate(history: &[Summary], since: i64) -> Rate {
    let mut r = Rate::default();
    for s in history.iter().filter(|s| s.started_ms >= since) {
        match judged_level(s) {
            Some(Level::Unknown) | None => {}
            Some(level) => {
                r.judged += 1;
                if level == Level::Ok {
                    r.ok += 1;
                }
            }
        }
    }
    r
}

fn slots(history: &[Summary], now: i64, hours: i64) -> Vec<Slot> {
    // 固定基础精度；前端按可用宽度合并相邻格，保留完整的时间范围和记录。
    let count = match hours {
        168 => 84,
        720 => 120,
        _ => 96,
    };
    let end = now + 1;
    let first = end - hours * HOUR_MS;
    let step = hours * HOUR_MS / count;
    let mut out: Vec<Slot> = (0..count)
        .map(|i| Slot {
            start_ms: first + i * step,
            end_ms: first + (i + 1) * step,
            level: None,
            runs: Vec::new(),
        })
        .collect();
    for s in history {
        if s.started_ms < first || s.started_ms >= end {
            continue;
        }
        let index = usize::try_from((s.started_ms - first) / step).unwrap_or_default();
        let slot = &mut out[index];
        if let Some(level) = judged_level(s) {
            slot.level = Some(slot.level.map_or(level, |cur| cur.max(level)));
        }
        slot.runs.push(SlotRun {
            id: s.id.clone(),
            started_ms: s.started_ms,
            model: s.model.clone(),
            status: s.status,
            level: s.level,
            reason: s.reason.clone(),
            detail: s.detail,
        });
    }
    out
}

async fn overview(app: &App, host: &HostClient, hours: i64) -> Result<Value, ApiError> {
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
        let mut methods = serde_json::Map::new();
        for method in Method::ALL {
            let history = if supported(a) {
                store::history(host, &a.account_id, method).await?
            } else {
                Vec::new()
            };
            let view = MethodView {
                schedule: schedule.get(method).clone(),
                last_attempt: app.scheduler.last(&a.account_id, method),
                latest: history
                    .iter()
                    .rev()
                    .find(|s| s.status != Status::Running)
                    .cloned(),
                slots: slots(&history, now, hours),
                rate: rate(&history, now - hours * HOUR_MS),
                rate_24h: rate(&history, now - 24 * HOUR_MS),
                rate_7d: rate(&history, now - 7 * 24 * HOUR_MS),
            };
            methods.insert(
                method.as_str().into(),
                serde_json::to_value(view).unwrap_or(Value::Null),
            );
        }
        let mut view = serde_json::to_value(AccountView::new(app, a)).unwrap_or(Value::Null);
        view["methods"] = Value::Object(methods);
        views.push(view);
    }
    Ok(json!({
        "now_ms": now,
        "range_hours": hours,
        "config": {
            "probe_enabled": app.runner.config.probe_enabled,
            "min_interval_minutes": app.runner.config.min_interval_minutes,
            "fingerprint_sync_hours": app.runner.config.fingerprint_sync_hours,
        },
        "bank": app.runner.bank.current().info,
        "accounts": views,
    }))
}

async fn account_detail(app: &App, host: &HostClient, id: &str) -> Result<Value, ApiError> {
    let account = host::account(host, id).await?;
    if account.provider_id != "openai" {
        return Err(ApiError::new(404, "not_found", "账号不存在"));
    }
    let (schedules, _) = store::schedules(host).await?;
    let schedule = schedules.accounts.get(id).cloned().unwrap_or_default();
    let mut methods = serde_json::Map::new();
    for method in Method::ALL {
        let mut history = store::history(host, id, method).await?;
        history.reverse();
        methods.insert(
            method.as_str().into(),
            json!({
                "schedule": schedule.get(method),
                "last_attempt": app.scheduler.last(id, method),
                "runs": history,
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
    let cpr = match app.runner.ensure_resources(host).await {
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
            "path": "经 CPR 自己的模型链（指定这个账号），用插件维护的 Key“账号探测（ModelTrace）”，计入它的用量",
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
