//! CPR 管理员接口：只用来读分组、读账号所在的分组，以及改账号的分组
//!
//! 插件 SDK 只能管理插件自己建的分组；要调整管理员建的现有分组，只能经 CPR 的管理员接口
//! （用户 2026-11-02 选择）。用部署级管理 API Key（`x-api-key`），只发给配置的 CPR 地址，
//! 宿主客户端不跟随重定向，3xx 也当失败。只调用下面这几个路由；`accounts/batch-update`
//! 只带 `accountIds` 和 `groupIds`。错误信息不带 Key 和地址。

use gateway_plugin_sdk::{call::host::HttpRequest, client::HostClient};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::config::Config;
use crate::verdict::one_line;

const PAGE_SIZE: u32 = 200;
const MAX_PAGES: u32 = 100;
const MAX_BODY: usize = 8 * 1024 * 1024;
/// 宿主上限是 1000，留余量
pub const BATCH: usize = 500;

const GROUPS: &str = "/api/admin/account-groups/options";
const ACCOUNTS: &str = "/api/admin/accounts";
const BATCH_UPDATE: &str = "/api/admin/accounts/batch-update";

/// CPR 里的一个分组
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRef {
    pub id: String,
    pub name: String,
    pub color: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

const fn enabled() -> bool {
    true
}

/// 账号和它现在所在的全部分组
#[derive(Debug, Clone, Deserialize)]
pub struct AccountGroups {
    pub id: String,
    #[serde(default)]
    pub groups: Vec<GroupRef>,
}

#[derive(Deserialize)]
struct Page<T> {
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    #[serde(default)]
    page: PageMeta,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PageMeta {
    #[serde(default)]
    total_pages: u32,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    code: Value,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Value,
}

/// 配好了地址和 Key 时返回规范化的地址（去掉末尾的 `/`）
///
/// # Errors
/// 没配或配得不对时返回给页面看的原因
pub fn base_url(config: &Config) -> Result<String, &'static str> {
    let url = config.cpr_admin_url.trim().trim_end_matches('/');
    let key = config.cpr_admin_api_key.trim();
    if url.is_empty() || key.is_empty() {
        return Err(
            "在插件设置里填写 CPR 地址，并在敏感配置里填写 CPR 管理 API Key 后才能调整现有分组",
        );
    }
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    if rest.is_none_or(|r| {
        r.is_empty() || r.contains(['?', '#', '@']) || r.chars().any(char::is_whitespace)
    }) || url.len() > 512
    {
        return Err("CPR 地址格式不对，应为 http(s)://主机[:端口]");
    }
    if key.len() > 512 || key.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("CPR 管理 API Key 格式不对");
    }
    Ok(url.to_owned())
}

pub struct Admin<'a> {
    host: &'a HostClient,
    base: String,
    key: String,
}

impl<'a> Admin<'a> {
    /// # Errors
    /// 没配好时返回原因
    pub fn new(host: &'a HostClient, config: &Config) -> Result<Self, String> {
        Ok(Self {
            host,
            base: base_url(config)?,
            key: config.cpr_admin_api_key.trim().to_owned(),
        })
    }

    fn redact(&self, s: &str) -> String {
        one_line(&s.replace(&self.key, "***").replace(&self.base, "CPR"), 200)
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: Option<Value>,
    ) -> Result<T, String> {
        let mut headers = vec![
            ("x-api-key".to_owned(), self.key.as_bytes().to_vec()),
            ("Accept".to_owned(), b"application/json".to_vec()),
        ];
        let payload = match body {
            Some(body) => {
                headers.push(("Content-Type".to_owned(), b"application/json".to_vec()));
                serde_json::to_vec(&body).map_err(|_| "请求编码失败".to_owned())?
            }
            None => Vec::new(),
        };
        let url = if query.is_empty() {
            format!("{}{path}", self.base)
        } else {
            format!("{}{path}?{query}", self.base)
        };
        let response = self
            .host
            .http(
                HttpRequest {
                    method: method.to_owned(),
                    url,
                    headers,
                },
                payload,
            )
            .await
            .map_err(|_| "连不上 CPR 管理接口，请检查插件设置里的 CPR 地址".to_owned())?;
        let status = response.status;
        let bytes = response
            .body
            .collect(MAX_BODY)
            .await
            .map_err(|_| "读取 CPR 管理接口的响应失败".to_owned())?;
        match status {
            401 | 403 => return Err("CPR 管理 API Key 无效或没有权限".into()),
            300..=399 => {
                return Err(format!(
                    "CPR 管理接口返回重定向（HTTP {status}），请填写 CPR 的实际地址"
                ));
            }
            _ => {}
        }
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| format!("CPR 管理接口返回了无效响应（HTTP {status}）"))?;
        // 成功时业务码是 200（创建时 HTTP 201）
        if !(200..300).contains(&status) || envelope.code.as_u64() != Some(200) {
            return Err(format!(
                "CPR 管理接口拒绝了请求（HTTP {status}）：{}",
                self.redact(&envelope.message)
            ));
        }
        serde_json::from_value(envelope.data)
            .map_err(|_| "CPR 管理接口的响应格式和预期不同，可能是 CPR 版本不兼容".to_owned())
    }

    async fn pages<T: DeserializeOwned>(&self, path: &str, filter: &str) -> Result<Vec<T>, String> {
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let query = format!("{filter}page={page}&pageSize={PAGE_SIZE}");
            let p: Page<T> = self.call("GET", path, &query, None).await?;
            let n = p.items.len();
            out.extend(p.items);
            if n < PAGE_SIZE as usize || page >= p.page.total_pages {
                return Ok(out);
            }
        }
        Err("CPR 里的记录太多，读不完".into())
    }

    /// CPR 里的全部分组
    ///
    /// # Errors
    /// 管理接口失败时返回原因
    pub async fn groups(&self) -> Result<Vec<GroupRef>, String> {
        self.pages(GROUPS, "").await
    }

    /// 全部 OpenAI 账号现在所在的分组
    ///
    /// # Errors
    /// 管理接口失败时返回原因
    pub async fn accounts(&self) -> Result<Vec<AccountGroups>, String> {
        self.pages(ACCOUNTS, "provider=openai&").await
    }

    /// 把这些账号的分组整个设成 `group_ids`（CPR 是整体替换）
    ///
    /// # Errors
    /// 管理接口失败时返回原因
    pub async fn set_groups(
        &self,
        account_ids: &[String],
        group_ids: &[String],
    ) -> Result<(), String> {
        for chunk in account_ids.chunks(BATCH) {
            let _: Value = self
                .call(
                    "POST",
                    BATCH_UPDATE,
                    "",
                    Some(json!({"accountIds": chunk, "groupIds": group_ids})),
                )
                .await?;
        }
        Ok(())
    }
}
