//! 运行器：所有安全闸都在这里，手动和自动探测走同一套
//!
//! - 测票经 `host.http.account` 直连 Codex 后端：走账号在 CPR 绑定的代理，代理不可用就失败、
//!   不回退直连；认证头由插件按当前凭据填写，不刷新令牌
//! - ModelTrace 经 `host.model.execute` 走 CPR 自己的模型链，指定账号，用插件自己维护的
//!   Client Key（计入它的用量）

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    call::{
        host::{
            AccountHttpRequest, AuthRuntimeAccount, HttpRequest, ModelExecuteRequest,
            ModelOperation,
        },
        model::{CanonicalEvent, ContentKind},
        resources::{GroupEnsureRequest, GroupMembersChange, KeyEnsureRequest},
    },
    client::HostClient,
};
use serde::Serialize;
use serde_json::Value;

use crate::bank_sync::BankStore;
use crate::clock::now_ms;
use crate::codex::{self, CookieJar, Credentials, StreamReader};
use crate::config::Config;
use crate::modeltrace::{self, Completion};
use crate::store::{self, Method, Pending, Run, Status};
use crate::turnstate::{self, EchoSession, Shot};
use crate::verdict::{UpstreamError, one_line};

/// 全局同时运行的探测上限
pub const MAX_CONCURRENT: usize = 8;
/// 进行中的记录超过这么久没有完成，就当作中断（页面关了或插件重启）
pub const STALE_MS: i64 = 20 * 60 * 1000;
const RESOURCE_KEY: &str = "probe";
const MAX_ERROR_BODY: usize = 4096;

/// 现在不能发真实探测的原因
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub code: &'static str,
    pub message: String,
}

impl Blocker {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// 只是“现在不行，稍后再试”
    #[must_use]
    pub fn busy(&self) -> bool {
        matches!(self.code, "busy" | "busy_all")
    }
}

/// 手动还是自动
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Manual,
    Auto,
}

impl Trigger {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

pub struct Runner {
    pub config: Config,
    pub bank: BankStore,
    /// 账号 → 正在运行的记录 ID 与开始时间；同账号串行
    busy: Mutex<HashMap<String, (String, i64)>>,
    /// ModelTrace 用的 Client Key ID
    key: Mutex<Option<String>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 只探测 OpenAI OAuth 账号
#[must_use]
pub fn supported(account: &AuthRuntimeAccount) -> bool {
    account.provider_id == "openai" && account.authentication_kind == "oauth"
}

impl Runner {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self {
            config,
            bank: BankStore::default(),
            busy: Mutex::new(HashMap::new()),
            key: Mutex::new(None),
        }
    }

    /// 账号正在运行的记录
    #[must_use]
    pub fn running(&self, account_id: &str) -> Option<String> {
        lock(&self.busy).get(account_id).map(|(id, _)| id.clone())
    }

    /// 超时未完成的进行中记录
    #[must_use]
    pub fn stale(&self) -> Vec<(String, String)> {
        let cutoff = now_ms() - STALE_MS;
        lock(&self.busy)
            .iter()
            .filter(|(_, (_, started))| *started < cutoff)
            .map(|(account, (id, _))| (account.clone(), id.clone()))
            .collect()
    }

    fn acquire(&self, account_id: &str, run_id: &str) -> Result<(), Blocker> {
        let mut busy = lock(&self.busy);
        if busy.contains_key(account_id) {
            return Err(Blocker::new("busy", "这个账号已有一个探测正在运行"));
        }
        if busy.len() >= MAX_CONCURRENT {
            return Err(Blocker::new(
                "busy_all",
                format!("同时运行的探测已达上限（{MAX_CONCURRENT} 个），稍后再试"),
            ));
        }
        busy.insert(account_id.to_owned(), (run_id.to_owned(), now_ms()));
        Ok(())
    }

    pub fn release(&self, account_id: &str, run_id: &str) {
        let mut busy = lock(&self.busy);
        if busy.get(account_id).is_some_and(|(id, _)| id == run_id) {
            busy.remove(account_id);
        }
    }

    /// 发真实探测前的全部拦截条件（不含自动探测的最短间隔，见 [`Self::too_soon`]）
    #[must_use]
    pub fn blockers(
        &self,
        account: &AuthRuntimeAccount,
        method: Method,
        model: &str,
    ) -> Vec<Blocker> {
        let mut out = Vec::new();
        if !self.config.probe_enabled {
            out.push(Blocker::new(
                "probe_disabled",
                "真实探测未开启（插件设置“发出真实探测”）：现在不会发送任何东西",
            ));
        }
        if !supported(account) {
            out.push(Blocker::new(
                "unsupported_account",
                "只支持 OpenAI OAuth 账号",
            ));
        }
        if !account.enabled {
            out.push(Blocker::new(
                "account_inactive",
                "账号在 CPR 已停用：只探测启用中的账号",
            ));
        }
        if matches!(
            account.credential_state.as_str(),
            "expired" | "banned" | "invalid"
        ) {
            out.push(Blocker::new(
                "credential_state",
                format!("账号凭据状态为 {}，不探测", account.credential_state),
            ));
        }
        if account
            .access_token_expires_at_ms
            .is_some_and(|exp| exp <= now_ms() + 30_000)
        {
            out.push(Blocker::new(
                "token_expired",
                "访问令牌已过期或即将过期。插件不刷新令牌（刷新令牌会轮换），等 CPR 自动刷新后再试",
            ));
        }
        if method == Method::Turnstate
            && account
                .upstream_account_id
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        {
            out.push(Blocker::new("no_credentials", "账号缺少 ChatGPT 账号 ID"));
        }
        if !codex::valid_model(model) {
            out.push(Blocker::new("invalid_model", "请选择要探测的模型"));
        }
        let busy = lock(&self.busy);
        if busy.contains_key(&account.account_id) {
            out.push(Blocker::new("busy", "这个账号已有一个探测正在运行"));
        } else if busy.len() >= MAX_CONCURRENT {
            out.push(Blocker::new(
                "busy_all",
                format!("同时运行的探测已达上限（{MAX_CONCURRENT} 个），稍后再试"),
            ));
        }
        out
    }

    /// 自动探测的最短间隔；手动探测不受它限制
    #[must_use]
    pub fn too_soon(&self, last_started_ms: Option<i64>) -> Option<Blocker> {
        let min = i64::from(self.config.min_interval_minutes) * 60_000;
        let last = last_started_ms?;
        let wait = last + min - now_ms();
        (min > 0 && wait > 0).then(|| {
            Blocker::new(
                "too_soon",
                format!(
                    "距离上次探测不足 {} 分钟，{} 分钟后再试",
                    self.config.min_interval_minutes,
                    wait / 60_000 + 1
                ),
            )
        })
    }

    /// 发起一次探测。测票在这一次调用里做完；ModelTrace 只建记录和题目，
    /// 之后每次 [`Self::advance`] 答一道题
    ///
    /// # Errors
    /// 被拦截时返回拦截原因；宿主回调失败时返回 `Err(None)` 以外的错误
    pub async fn start(
        &self,
        host: &HostClient,
        account: &AuthRuntimeAccount,
        method: Method,
        model: &str,
        trigger: Trigger,
    ) -> Result<Result<Run, Vec<Blocker>>, PluginFault> {
        let blockers = self.blockers(account, method, model);
        if !blockers.is_empty() {
            return Ok(Err(blockers));
        }
        let id = run_id()?;
        if let Err(blocker) = self.acquire(&account.account_id, &id) {
            return Ok(Err(vec![blocker]));
        }
        let mut run = Run {
            id: id.clone(),
            account_id: account.account_id.clone(),
            method,
            model: model.to_owned(),
            trigger: trigger.as_str().to_owned(),
            started_ms: now_ms(),
            finished_ms: 0,
            status: Status::Running,
            verdict: None,
            error: None,
            turnstate: None,
            modeltrace: None,
            pending: None,
        };
        let result = async {
            match method {
                Method::Turnstate => {
                    let version = store::save_with_notifications(
                        host,
                        &run,
                        None,
                        self.config.telegram_enabled,
                    )
                    .await?;
                    match within_budget(host, self.turnstate(host, account, model)).await {
                        Ok(result) => {
                            run.verdict = Some(turnstate::verdict(&result));
                            run.turnstate = Some(result);
                            finish(&mut run, None);
                        }
                        Err(error) => finish(&mut run, Some(error)),
                    }
                    store::save_with_notifications(
                        host,
                        &run,
                        Some(version),
                        self.config.telegram_enabled,
                    )
                    .await?;
                    crate::telegram::dispatch(host, &self.config, account).await;
                }
                Method::Modeltrace => {
                    let challenges = modeltrace::challenges()
                        .map_err(|e| PluginFault::new(ErrorCode::Fault, e))?;
                    run.pending = Some(Pending {
                        challenges,
                        completions: Vec::new(),
                    });
                    if let Err(error) = self.prepare_key(host, &account.account_id).await {
                        finish(&mut run, Some(error));
                    }
                    store::save(host, &run, None).await?;
                }
            }
            Ok::<_, PluginFault>(())
        }
        .await;
        if result.is_err() || run.status != Status::Running {
            self.release(&account.account_id, &id);
        }
        result?;
        Ok(Ok(run))
    }

    /// 让进行中的 ModelTrace 答下一道题；答完最后一道时判定并结束
    ///
    /// # Errors
    /// 宿主回调失败时返回错误
    pub async fn advance(
        &self,
        host: &HostClient,
        mut run: Run,
        version: u64,
    ) -> Result<Run, PluginFault> {
        if run.status != Status::Running || run.method != Method::Modeltrace {
            return Ok(run);
        }
        // 插件重启后忙碌登记丢了：重新登记，避免同账号并发
        {
            let mut busy = lock(&self.busy);
            match busy.get(&run.account_id) {
                Some((id, _)) if id != &run.id => {
                    return Err(PluginFault::new(
                        ErrorCode::Conflict,
                        "这个账号已有另一个探测正在运行",
                    ));
                }
                Some(_) => {}
                None => {
                    busy.insert(run.account_id.clone(), (run.id.clone(), run.started_ms));
                }
            }
        }
        let outcome = within_budget(host, self.step(host, &mut run)).await;
        if let Err(error) = outcome {
            finish(&mut run, Some(error));
        }
        if run.status != Status::Running {
            self.release(&run.account_id, &run.id);
        }
        store::save(host, &run, Some(version)).await?;
        Ok(run)
    }

    async fn step(&self, host: &HostClient, run: &mut Run) -> Result<(), UpstreamError> {
        let account = host::account(host, &run.account_id)
            .await
            .map_err(|_| UpstreamError::new("host", "账号在 CPR 里已不存在"))?;
        // 每道题前重新过一遍安全闸（除了“正在运行”，那就是它自己）
        let blockers: Vec<Blocker> = self
            .blockers(&account, run.method, &run.model)
            .into_iter()
            .filter(|b| !b.busy())
            .collect();
        if let Some(b) = blockers.first() {
            return Err(UpstreamError::new("blocked", b.message.clone()));
        }
        let Some(pending) = run.pending.as_mut() else {
            return Err(UpstreamError::new("other", "记录缺少题目"));
        };
        let index = pending.completions.len();
        let Some(challenge) = pending.challenges.get(index) else {
            return Err(UpstreamError::new("other", "题目已答完"));
        };
        let key = self.prepare_key(host, &run.account_id).await?;
        let completion = complete(host, &key, &run.account_id, &run.model, &challenge.prompt)
            .await
            .map_err(|e| UpstreamError {
                message: format!(
                    "第 {} 道题（共 {}）：{}",
                    index + 1,
                    pending.challenges.len(),
                    e.message
                ),
                ..e
            })?;
        pending.completions.push(completion);
        if pending.completions.len() == pending.challenges.len() {
            let current = self.bank.current();
            let result = modeltrace::analyze(
                &current.bank,
                &current.info.commit,
                &run.model,
                &pending.challenges,
                &pending.completions,
            );
            run.verdict = Some(modeltrace::verdict(&result));
            run.modeltrace = Some(result);
            run.pending = None;
            finish(run, None);
        }
        Ok(())
    }

    /// 把中断的进行中记录标成失败
    ///
    /// # Errors
    /// 宿主回调失败时返回错误
    pub async fn abandon(
        &self,
        host: &HostClient,
        account_id: &str,
        run_id: &str,
    ) -> Result<(), PluginFault> {
        if let Some((mut run, version)) = store::run(host, run_id).await?
            && run.status == Status::Running
        {
            run.pending = None;
            finish(
                &mut run,
                Some(UpstreamError::new(
                    "aborted",
                    "探测中断（页面关闭或插件重启），没有完成",
                )),
            );
            store::save_with_notifications(host, &run, Some(version), self.config.telegram_enabled)
                .await?;
        }
        self.release(account_id, run_id);
        Ok(())
    }

    /// ModelTrace 用的分组和 Key：插件自己建，账号进分组后 Key 才能指定它
    async fn prepare_key(
        &self,
        host: &HostClient,
        account_id: &str,
    ) -> Result<String, UpstreamError> {
        let key = self.ensure_resources(host).await.map_err(|e| {
            UpstreamError::new(
                "host",
                format!("准备探测用的 Key 失败：{}", one_line(&e.message, 200)),
            )
        })?;
        host.change_group_members(GroupMembersChange {
            resource_key: RESOURCE_KEY.into(),
            add: vec![account_id.to_owned()],
            remove: Vec::new(),
        })
        .await
        .map_err(|e| {
            UpstreamError::new(
                "host",
                format!("把账号加入探测分组失败：{}", one_line(&e.message, 200)),
            )
        })?;
        Ok(key)
    }

    /// 确保探测分组与 Key 存在，返回 Key ID
    ///
    /// # Errors
    /// 宿主回调失败时返回错误
    pub async fn ensure_resources(&self, host: &HostClient) -> Result<String, PluginFault> {
        if let Some(key) = lock(&self.key).clone() {
            return Ok(key);
        }
        host.ensure_group(GroupEnsureRequest {
            resource_key: RESOURCE_KEY.into(),
            name: "账号探测".into(),
            color: "#6366F1FF".into(),
            description: Some("账号探测插件的 ModelTrace 用，成员由插件维护".into()),
        })
        .await?;
        let key = host
            .ensure_key(KeyEnsureRequest {
                resource_key: RESOURCE_KEY.into(),
                name: "账号探测（ModelTrace）".into(),
                group_resource_keys: vec![RESOURCE_KEY.into()],
                max_concurrency: MAX_CONCURRENT as u64,
                requests_per_minute: 0,
                daily_limit_usd: "0".into(),
                weekly_limit_usd: "0".into(),
            })
            .await?;
        *lock(&self.key) = Some(key.id.clone());
        Ok(key.id)
    }

    /// 把账号补进探测分组，每次最多 200 个
    ///
    /// # Errors
    /// 宿主回调失败时返回错误
    pub async fn sync_members(
        &self,
        host: &HostClient,
        accounts: &[AuthRuntimeAccount],
    ) -> Result<(), PluginFault> {
        self.ensure_resources(host).await?;
        let ids: Vec<String> = accounts
            .iter()
            .filter(|a| supported(a))
            .map(|a| a.account_id.clone())
            .collect();
        for chunk in ids.chunks(200) {
            host.change_group_members(GroupMembersChange {
                resource_key: RESOURCE_KEY.into(),
                add: chunk.to_vec(),
                remove: Vec::new(),
            })
            .await?;
        }
        Ok(())
    }

    async fn turnstate(
        &self,
        host: &HostClient,
        account: &AuthRuntimeAccount,
        model: &str,
    ) -> Result<turnstate::TurnStateResult, UpstreamError> {
        let credential = crate::host::credential(host, &account.account_id)
            .await
            .map_err(|e| {
                UpstreamError::new(
                    "host",
                    format!("读取账号凭据失败：{}", one_line(&e.message, 200)),
                )
            })?;
        let access_token = credential
            .facts
            .material
            .get("access_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        let chatgpt_account_id = credential
            .facts
            .upstream_account_id
            .clone()
            .or_else(|| account.upstream_account_id.clone())
            .unwrap_or_default();
        if access_token.is_empty() || chatgpt_account_id.is_empty() {
            return Err(UpstreamError::new(
                "other",
                "账号缺少访问令牌或 ChatGPT 账号 ID",
            ));
        }
        let mut session = HostEcho {
            host,
            account_id: account.account_id.clone(),
            revision: credential.credential_revision,
            credentials: Credentials {
                access_token,
                chatgpt_account_id,
            },
            identity: host::codex_identity(host).await?,
            session_id: codex::new_uuid().map_err(|e| UpstreamError::new("other", e))?,
            jar: CookieJar::default(),
        };
        turnstate::run(&mut session, model).await
    }
}

use crate::host;

/// 给结果落库留出时间，避免宿主结束 RPC 后记录还停在“进行中”。
async fn within_budget<T>(
    host: &HostClient,
    future: impl std::future::Future<Output = Result<T, UpstreamError>>,
) -> Result<T, UpstreamError> {
    let budget = host
        .remaining_budget()
        .unwrap_or(Duration::from_secs(120))
        .saturating_sub(Duration::from_secs(5));
    tokio::time::timeout(budget, future)
        .await
        .unwrap_or_else(|_| Err(UpstreamError::new("timeout", "探测请求超时")))
}

fn finish(run: &mut Run, error: Option<UpstreamError>) {
    run.finished_ms = now_ms();
    match error {
        None => run.status = Status::Done,
        Some(error) => {
            run.pending = None;
            if let Some(verdict) = error.overloaded_verdict() {
                run.status = Status::Done;
                run.verdict = Some(verdict);
            } else {
                run.status = Status::Failed;
                run.verdict = None;
            }
            run.error = Some(error);
        }
    }
}

fn run_id() -> Result<String, PluginFault> {
    let mut suffix = [0_u8; 4];
    crate::fingerprint::system_random(&mut suffix)
        .map_err(|e| PluginFault::new(ErrorCode::Fault, e))?;
    Ok(format!("{}-{}", now_ms(), hex::encode(suffix)))
}

/// 测票的一次对话：同一个 session id 和 cookie，经账号绑定的代理发送
struct HostEcho<'a> {
    host: &'a HostClient,
    account_id: String,
    revision: u64,
    credentials: Credentials,
    identity: codex::Identity,
    session_id: String,
    jar: CookieJar,
}

impl EchoSession for HostEcho<'_> {
    async fn shoot(&mut self, model: &str, state: &str) -> Result<Shot, UpstreamError> {
        let before = self.jar.route();
        let (headers, body) = codex::echo_request(
            &self.credentials,
            &self.identity,
            model,
            &self.session_id,
            state,
            &self.jar.header(),
        )?;
        let start = Instant::now();
        let response = self
            .host
            .account_http(
                AccountHttpRequest {
                    account_id: self.account_id.clone(),
                    credential_revision: self.revision,
                    request: HttpRequest {
                        method: "POST".into(),
                        url: codex::ENDPOINT.into(),
                        headers,
                    },
                },
                body,
            )
            .await
            .map_err(|e| self.host_error(&e))?;
        self.jar.store(&response.headers);
        let mut body = response.body;
        if response.status != 200 {
            let text = read_limited(&mut body, MAX_ERROR_BODY).await;
            let _ = body.close().await;
            return Err(UpstreamError {
                status: Some(response.status),
                kind: codex::status_kind(response.status).into(),
                message: self.sanitize(&codex::error_text(&text)),
            });
        }
        let mut reader = StreamReader::default();
        let outcome = loop {
            match body.read().await {
                Ok(Some(chunk)) => {
                    if let Some(done) = reader.feed(&chunk) {
                        let _ = body.close().await;
                        break done;
                    }
                }
                Ok(None) => break reader.finish(),
                Err(e) => break Err(self.host_error(&e)),
            }
        };
        let outcome = outcome.map_err(|e| UpstreamError {
            message: self.sanitize(&e.message),
            ..e
        })?;
        if !outcome.status.is_empty() && outcome.status != "completed" {
            return Err(UpstreamError::new(
                "protocol",
                format!("响应以 {} 结束", one_line(&outcome.status, 40)),
            ));
        }
        let mut got = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(codex::TURN_STATE_HEADER))
            .and_then(|(_, v)| std::str::from_utf8(v).ok())
            .unwrap_or_default()
            .trim()
            .to_owned();
        if !outcome.metadata_turn_state.is_empty() {
            got = outcome.metadata_turn_state;
        }
        let changed = !before.is_empty() && self.jar.route() != before;
        Ok(Shot {
            state: got,
            route_changed: changed,
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

impl HostEcho<'_> {
    fn sanitize(&self, s: &str) -> String {
        one_line(&codex::redact(s, &self.credentials), 300)
    }

    fn host_error(&self, error: &PluginFault) -> UpstreamError {
        let message = self.sanitize(&error.message);
        match error.code {
            ErrorCode::Conflict => UpstreamError::new("host", "账号凭据刚更新过，请重试"),
            ErrorCode::PermissionDenied | ErrorCode::Rejected => {
                UpstreamError::new("host", "账号在 CPR 已停用或不可用")
            }
            ErrorCode::Timeout => UpstreamError::new("network", format!("请求超时：{message}")),
            _ => UpstreamError::new("network", message),
        }
    }
}

async fn read_limited(
    body: &mut gateway_plugin_sdk::client::HostHttpBody,
    limit: usize,
) -> Vec<u8> {
    let mut out = Vec::new();
    while out.len() < limit {
        match body.read().await {
            Ok(Some(chunk)) => out.extend_from_slice(&chunk),
            _ => break,
        }
    }
    out.truncate(limit);
    out
}

/// 经 CPR 模型链让指定账号答一道题
async fn complete(
    host: &HostClient,
    key: &str,
    account_id: &str,
    model: &str,
    prompt: &str,
) -> Result<Completion, UpstreamError> {
    let start = Instant::now();
    let batch = host::model_execute(
        host,
        &ModelExecuteRequest {
            client_key_id: Some(key.to_owned()),
            model: model.to_owned(),
            protocol: "openai".into(),
            operation: ModelOperation::Generate,
            provider: Some("openai".into()),
            account_id: Some(account_id.to_owned()),
            previous_response_id: None,
        },
        &codex::challenge_body(model, prompt),
    )
    .await
    .map_err(|e| model_error(&e))?;
    let mut out = Completion::default();
    let mut kinds: HashMap<u32, ContentKind> = HashMap::new();
    for event in batch.events {
        for fact in event.facts {
            match fact {
                CanonicalEvent::ContentAdded { index, kind } => {
                    kinds.insert(index, kind);
                }
                CanonicalEvent::TextDelta { index, text } => {
                    if kinds.get(&index).is_none_or(|k| *k == ContentKind::Text) {
                        out.text.push_str(&text);
                    }
                }
                CanonicalEvent::Usage { usage } => {
                    out.output_tokens = usage.output_tokens.unwrap_or_default();
                    out.reasoning_tokens = usage.reasoning_tokens.unwrap_or_default();
                }
                CanonicalEvent::Started { model: Some(m), .. }
                | CanonicalEvent::Completed { model: Some(m), .. } => out.response_model = m,
                _ => {}
            }
        }
    }
    out.duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    Ok(out)
}

/// 模型链的错误归类；CPR 的错误消息是给客户端看的，不带凭据
fn model_error(error: &PluginFault) -> UpstreamError {
    let details = error.details.clone().unwrap_or(Value::Null);
    let status = details
        .pointer("/response/status")
        .and_then(Value::as_u64)
        .and_then(|s| u16::try_from(s).ok());
    let detail_kind = details
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let message = one_line(&error.message, 300);
    let by_message = codex::classify(detail_kind, &message);
    let kind = if by_message == "overloaded" {
        "overloaded"
    } else if let Some(status) = status.filter(|s| codex::status_kind(*s) != "other") {
        codex::status_kind(status)
    } else if by_message != "other" {
        by_message
    } else {
        match error.code {
            ErrorCode::PermissionDenied => "unauthorized",
            ErrorCode::Capacity if detail_kind.contains("rate") => "rate_limited",
            ErrorCode::Timeout | ErrorCode::Upstream => "network",
            _ => "other",
        }
    };
    UpstreamError {
        status,
        kind: kind.into(),
        message,
    }
}
