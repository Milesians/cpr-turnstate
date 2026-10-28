//! 指纹库：内置一份，按设置的间隔从 GitHub 上的 xqy2006/ModelTrace 同步
//!
//! 只同步数据文件 `static/data/unified_bank.json`；评分算法是移植的，上游改了库格式时
//! 校验会失败，继续用手上的那份并显示原因。指纹库只在内存里（单条私有状态最大
//! 256 KiB，放不下 900 KiB 的库），插件重启后先用内置的，下一轮维护再同步。

use std::sync::{Arc, Mutex};

use gateway_plugin_sdk::{call::host::HttpRequest, client::HostClient};
use serde::Serialize;

use crate::clock::now_ms;
use crate::fingerprint::{Bank, EMBEDDED_COMMIT};
use crate::verdict::one_line;

const REPOSITORY: &str = "xqy2006/ModelTrace";
const PATH: &str = "static/data/unified_bank.json";
const MAX_BANK_BYTES: usize = 8 * 1024 * 1024;
const USER_AGENT: &str = "cpr-account-probe";

/// 当前在用的指纹库
pub struct Current {
    pub bank: Arc<Bank>,
    pub info: Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Info {
    /// ModelTrace 提交
    pub commit: String,
    /// embedded 或 github
    pub source: String,
    /// 提交时间（GitHub 给的 ISO 时间），内置的为空
    #[serde(skip_serializing_if = "String::is_empty")]
    pub committed_at: String,
    pub models: Vec<String>,
    /// 上次检查的时间，0 表示还没检查过
    pub checked_ms: i64,
    /// 上次检查失败的原因
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

pub struct BankStore {
    current: Mutex<Arc<Current>>,
}

impl Default for BankStore {
    fn default() -> Self {
        let bank = Bank::embedded();
        let info = Info {
            commit: EMBEDDED_COMMIT.to_owned(),
            source: "embedded".into(),
            committed_at: String::new(),
            models: bank.models.iter().map(|m| m.id.clone()).collect(),
            checked_ms: 0,
            error: String::new(),
        };
        Self {
            current: Mutex::new(Arc::new(Current {
                bank: Arc::new(bank),
                info,
            })),
        }
    }
}

impl BankStore {
    #[must_use]
    pub fn current(&self) -> Arc<Current> {
        Arc::clone(
            &self
                .current
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    fn set(&self, next: Current) {
        *self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(next);
    }

    fn note(&self, error: String) {
        let current = self.current();
        let mut info = current.info.clone();
        info.checked_ms = now_ms();
        info.error = error;
        self.set(Current {
            bank: Arc::clone(&current.bank),
            info,
        });
    }

    /// 到时间了就同步：`hours` 为 0 时不同步
    pub async fn sync_if_due(&self, host: &HostClient, hours: u32) {
        let checked = self.current().info.checked_ms;
        if hours == 0 || (checked > 0 && now_ms() - checked < i64::from(hours) * 3_600_000) {
            return;
        }
        self.sync(host).await;
    }

    /// 查 GitHub 上数据文件最新的提交，变了就下载、校验、替换
    pub async fn sync(&self, host: &HostClient) {
        match self.fetch(host).await {
            Ok(Some(next)) => self.set(next),
            Ok(None) => self.note(String::new()),
            Err(error) => self.note(error),
        }
    }

    async fn fetch(&self, host: &HostClient) -> Result<Option<Current>, String> {
        let url =
            format!("https://api.github.com/repos/{REPOSITORY}/commits?path={PATH}&per_page=1");
        let body = get(host, &url, "application/vnd.github+json", 256 * 1024).await?;
        let commits: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|_| "GitHub 返回的提交列表不是 JSON".to_owned())?;
        let latest = commits.get(0).ok_or("GitHub 没有返回这个文件的提交")?;
        let sha = latest
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("GitHub 返回的提交号无效")?
            .to_owned();
        let committed_at = latest
            .pointer("/commit/committer/date")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if sha == self.current().info.commit {
            return Ok(None);
        }
        let raw = format!("https://raw.githubusercontent.com/{REPOSITORY}/{sha}/{PATH}");
        let data = get(host, &raw, "application/json", MAX_BANK_BYTES).await?;
        let bank = Bank::parse(&data)?;
        Ok(Some(Current {
            info: Info {
                commit: sha,
                source: "github".into(),
                committed_at,
                models: bank.models.iter().map(|m| m.id.clone()).collect(),
                checked_ms: now_ms(),
                error: String::new(),
            },
            bank: Arc::new(bank),
        }))
    }
}

async fn get(host: &HostClient, url: &str, accept: &str, limit: usize) -> Result<Vec<u8>, String> {
    let response = host
        .http(
            HttpRequest {
                method: "GET".into(),
                url: url.to_owned(),
                headers: vec![
                    ("Accept".into(), accept.as_bytes().to_vec()),
                    ("User-Agent".into(), USER_AGENT.as_bytes().to_vec()),
                ],
            },
            Vec::new(),
        )
        .await
        .map_err(|error| format!("访问 GitHub 失败：{}", one_line(&error.message, 200)))?;
    if response.status != 200 {
        let mut body = response.body;
        let _ = body.close().await;
        return Err(format!("GitHub 返回 HTTP {}", response.status));
    }
    response
        .body
        .collect(limit)
        .await
        .map_err(|error| format!("读取 GitHub 响应失败：{}", one_line(&error.message, 200)))
}
