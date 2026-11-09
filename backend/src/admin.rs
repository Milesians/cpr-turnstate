//! CPR 管理员接口：只用来读分组、读账号所在的分组，以及改账号的分组
//!
//! 插件 SDK 只能管理插件自己建的分组；要调整管理员建的现有分组，只能经 CPR 的管理员接口
//! （用户 2026-11-02 选择）。经 `host.http.dispatch` 发给宿主自己的路由：只传相对路径，
//! 宿主给子请求注入插件身份，不需要管理 API Key，也不出本机网络。只调用下面这几个路由；
//! `accounts/batch-update` 只带 `accountIds` 和 `groupIds`。

use gateway_plugin_sdk::{
    call::middleware::{MiddlewareHeader, http::Version},
    client::{HostClient, HttpBody, HttpFrame, HttpRequest},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::verdict::one_line;

const PAGE_SIZE: u32 = 200;
const MAX_PAGES: u32 = 100;
const MAX_BODY: usize = 8 * 1024 * 1024;
const TIMEOUT_MS: u64 = 20_000;
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

pub struct Admin<'a> {
    host: &'a HostClient,
}

impl<'a> Admin<'a> {
    #[must_use]
    pub const fn new(host: &'a HostClient) -> Self {
        Self { host }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        query: &str,
        body: Option<Value>,
    ) -> Result<T, String> {
        let mut headers = vec![MiddlewareHeader {
            name: "accept".to_owned(),
            value: b"application/json".to_vec(),
        }];
        let body = match body {
            Some(body) => {
                headers.push(MiddlewareHeader {
                    name: "content-type".to_owned(),
                    value: b"application/json".to_vec(),
                });
                HttpBody::from_bytes(
                    serde_json::to_vec(&body).map_err(|_| "请求编码失败".to_owned())?,
                )
            }
            None => HttpBody::empty(),
        };
        let uri = if query.is_empty() {
            path.to_owned()
        } else {
            format!("{path}?{query}")
        };
        let mut response = self
            .host
            .dispatch_http(HttpRequest {
                settings: Value::Null,
                method: method.to_owned(),
                uri,
                version: Version::Http11,
                headers,
                timeout_ms: Some(TIMEOUT_MS),
                body,
            })
            .await
            .map_err(|_| "调不通 CPR 管理接口，请确认 CPR 版本支持插件调用管理接口".to_owned())?;
        let status = response.status;
        let mut bytes = Vec::new();
        loop {
            match response.body.read().await {
                Ok(Some(HttpFrame::Data(chunk))) => {
                    if bytes.len() + chunk.len() > MAX_BODY {
                        let _ = response.body.close().await;
                        return Err("CPR 管理接口的响应太大".into());
                    }
                    bytes.extend(chunk);
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => return Err("读取 CPR 管理接口的响应失败".into()),
            }
        }
        if matches!(status, 401 | 403) {
            return Err("CPR 拒绝了插件调用管理接口（没有权限）".into());
        }
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|_| format!("CPR 管理接口返回了无效响应（HTTP {status}）"))?;
        // 成功时业务码是 200（创建时 HTTP 201）
        if !(200..300).contains(&status) || envelope.code.as_u64() != Some(200) {
            return Err(format!(
                "CPR 管理接口拒绝了请求（HTTP {status}）：{}",
                one_line(&envelope.message, 200)
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
