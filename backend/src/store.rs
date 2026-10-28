//! 探测设置、历史与单次记录，存在插件私有状态里
//!
//! - `settings/schedules`：每个账号每种方法的自动探测开关、模型与间隔
//! - `history/<账号>/<方法>`：最近 30 天的探测摘要，按时间排列
//! - `runs/<记录 ID>`：单次探测的详情，每个账号每种方法只留最近 [`DETAIL_KEEP`] 条
//!
//! 私有状态只有按键读写，没有列表；摘要里记着哪些详情还在，清理时据此删除。
//! 凭据和票都不进状态。

use std::collections::BTreeMap;

use gateway_plugin_sdk::{ErrorCode, PluginFault, client::HostClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fingerprint::Challenge;
use crate::host;
use crate::modeltrace::{Completion, ModelTraceResult};
use crate::turnstate::TurnStateResult;
use crate::verdict::{Level, UpstreamError, Verdict};

const SETTINGS: &str = "settings";
const SCHEDULES: &str = "schedules";
const HISTORY: &str = "history";
const RUNS: &str = "runs";

/// 摘要保留的时间
pub const HISTORY_DAYS: i64 = 30;
/// 每个账号每种方法保留详情的条数
pub const DETAIL_KEEP: usize = 50;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 探测方法
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Turnstate,
    Modeltrace,
}

impl Method {
    /// 按发起顺序：便宜的在前
    pub const ALL: [Self; 2] = [Self::Turnstate, Self::Modeltrace];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turnstate => "turnstate",
            Self::Modeltrace => "modeltrace",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Turnstate => "测票",
            Self::Modeltrace => "ModelTrace",
        }
    }
}

/// 一种方法的自动探测设置
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MethodSchedule {
    pub enabled: bool,
    pub model: String,
    pub interval_minutes: u32,
}

impl Default for MethodSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            model: String::new(),
            interval_minutes: 60,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountSchedule {
    #[serde(default)]
    pub turnstate: MethodSchedule,
    #[serde(default)]
    pub modeltrace: MethodSchedule,
}

impl AccountSchedule {
    #[must_use]
    pub const fn get(&self, method: Method) -> &MethodSchedule {
        match method {
            Method::Turnstate => &self.turnstate,
            Method::Modeltrace => &self.modeltrace,
        }
    }

    pub const fn get_mut(&mut self, method: Method) -> &mut MethodSchedule {
        match method {
            Method::Turnstate => &mut self.turnstate,
            Method::Modeltrace => &mut self.modeltrace,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schedules {
    #[serde(default)]
    pub accounts: BTreeMap<String, AccountSchedule>,
}

/// 记录状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Running,
    Done,
    Failed,
}

/// 历史里的一条摘要
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub id: String,
    pub model: String,
    /// manual 或 auto
    pub trigger: String,
    pub started_ms: i64,
    #[serde(default)]
    pub finished_ms: i64,
    pub status: Status,
    /// 完成时的判定；失败时为空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<Level>,
    /// 判定理由或失败原因
    #[serde(default)]
    pub reason: String,
    /// 详情还在 `runs` 里
    #[serde(default)]
    pub detail: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct History {
    #[serde(default)]
    runs: Vec<Summary>,
}

/// 页面分步发起的 ModelTrace 在完成前保存的题目和已收到的回答
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub challenges: Vec<Challenge>,
    pub completions: Vec<Completion>,
}

/// 单次探测的详情
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub account_id: String,
    pub method: Method,
    pub model: String,
    pub trigger: String,
    pub started_ms: i64,
    #[serde(default)]
    pub finished_ms: i64,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<UpstreamError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turnstate: Option<TurnStateResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modeltrace: Option<ModelTraceResult>,
    /// 只在进行中的 ModelTrace 上有，完成后删掉（回答原文不长期保存）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending: Option<Pending>,
}

impl Run {
    #[must_use]
    pub fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            model: self.model.clone(),
            trigger: self.trigger.clone(),
            started_ms: self.started_ms,
            finished_ms: self.finished_ms,
            status: self.status,
            level: self.verdict.as_ref().map(|v| v.level),
            reason: match (&self.verdict, &self.error) {
                (Some(v), _) => v.reason.clone(),
                (None, Some(e)) => e.to_string(),
                (None, None) => String::new(),
            },
            detail: true,
        }
    }
}

fn history_key(account_id: &str, method: Method) -> String {
    format!("{account_id}/{}", method.as_str())
}

fn encode<T: Serialize>(value: &T) -> Result<Value, PluginFault> {
    serde_json::to_value(value).map_err(|_| PluginFault::new(ErrorCode::Fault, "记录编码失败"))
}

/// 读全部自动探测设置和它的版本
///
/// # Errors
/// 回调失败时返回错误
pub async fn schedules(host: &HostClient) -> Result<(Schedules, Option<u64>), PluginFault> {
    Ok(match host::state_get(host, SETTINGS, SCHEDULES).await? {
        Some(record) => (
            serde_json::from_value(record.value).unwrap_or_default(),
            Some(record.version),
        ),
        None => (Schedules::default(), None),
    })
}

/// 改一个账号一种方法的设置；并发修改时重读再改
///
/// # Errors
/// 回调失败时返回错误
pub async fn update_schedule(
    host: &HostClient,
    account_id: &str,
    method: Method,
    schedule: MethodSchedule,
) -> Result<AccountSchedule, PluginFault> {
    for _ in 0..5 {
        let (mut all, version) = schedules(host).await?;
        let account = all.accounts.entry(account_id.to_owned()).or_default();
        *account.get_mut(method) = schedule.clone();
        let updated = account.clone();
        if updated == AccountSchedule::default() {
            all.accounts.remove(account_id);
        }
        match host::state_put(host, SETTINGS, SCHEDULES, encode(&all)?, version).await {
            Ok(_) => return Ok(updated),
            Err(error) if error.code == ErrorCode::Conflict => {}
            Err(error) => return Err(error),
        }
    }
    Err(PluginFault::new(
        ErrorCode::Conflict,
        "设置被同时修改，请重试",
    ))
}

/// 一个账号一种方法的摘要，旧的在前
///
/// # Errors
/// 回调失败时返回错误
pub async fn history(
    host: &HostClient,
    account_id: &str,
    method: Method,
) -> Result<Vec<Summary>, PluginFault> {
    Ok(history_record(host, account_id, method).await?.0.runs)
}

async fn history_record(
    host: &HostClient,
    account_id: &str,
    method: Method,
) -> Result<(History, Option<u64>), PluginFault> {
    Ok(
        match host::state_get(host, HISTORY, &history_key(account_id, method)).await? {
            Some(record) => (
                serde_json::from_value(record.value).unwrap_or_default(),
                Some(record.version),
            ),
            None => (History::default(), None),
        },
    )
}

/// 读一条详情
///
/// # Errors
/// 回调失败时返回错误
pub async fn run(host: &HostClient, id: &str) -> Result<Option<(Run, u64)>, PluginFault> {
    Ok(match host::state_get(host, RUNS, id).await? {
        Some(record) => serde_json::from_value(record.value)
            .ok()
            .map(|run| (run, record.version)),
        None => None,
    })
}

/// 保存详情并更新摘要。`version` 为空表示新记录
///
/// # Errors
/// 回调失败时返回错误
pub async fn save(host: &HostClient, run: &Run, version: Option<u64>) -> Result<u64, PluginFault> {
    let version = host::state_put(host, RUNS, &run.id, encode(run)?, version).await?;
    let expired = upsert_summary(host, run.summary(), &run.account_id, run.method).await?;
    for id in expired {
        if let Some((_, v)) = self::run(host, &id).await? {
            let _ = host::state_delete(host, RUNS, &id, v).await;
        }
    }
    Ok(version)
}

/// 写入一条摘要，丢掉 30 天前的摘要；返回该删的详情 ID
async fn upsert_summary(
    host: &HostClient,
    summary: Summary,
    account_id: &str,
    method: Method,
) -> Result<Vec<String>, PluginFault> {
    let key = history_key(account_id, method);
    for _ in 0..5 {
        let (mut history, version) = history_record(host, account_id, method).await?;
        match history.runs.iter_mut().find(|s| s.id == summary.id) {
            Some(existing) => *existing = summary.clone(),
            None => history.runs.push(summary.clone()),
        }
        history.runs.sort_by_key(|s| s.started_ms);
        let cutoff = summary.started_ms.max(crate::clock::now_ms()) - HISTORY_DAYS * DAY_MS;
        let mut expired = Vec::new();
        history.runs.retain(|s| {
            let keep = s.started_ms >= cutoff;
            if !keep && s.detail {
                expired.push(s.id.clone());
            }
            keep
        });
        let with_detail = history.runs.iter().filter(|s| s.detail).count();
        let mut drop = with_detail.saturating_sub(DETAIL_KEEP);
        for s in &mut history.runs {
            if drop == 0 {
                break;
            }
            if s.detail && s.status != Status::Running {
                s.detail = false;
                expired.push(s.id.clone());
                drop -= 1;
            }
        }
        match host::state_put(host, HISTORY, &key, encode(&history)?, version).await {
            Ok(_) => return Ok(expired),
            Err(error) if error.code == ErrorCode::Conflict => {}
            Err(error) => return Err(error),
        }
    }
    Err(PluginFault::new(
        ErrorCode::Conflict,
        "探测历史被同时修改，请重试",
    ))
}
