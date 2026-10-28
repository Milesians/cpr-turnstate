//! 宿主回调的编码：账号、凭据、私有状态、模型调用和日志

use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    call::host::{
        AuthCredential, AuthGetRequest, AuthListRequest, AuthListResult, AuthRuntimeAccount,
        LogLevel, LogRequest, ModelEventBatch, ModelExecuteRequest, ModelExecuteResult,
        StateDeleteRequest, StateGetRequest, StateGetResult, StatePutRequest, StatePutResult,
        StateRecord,
    },
    client::{HostClient, SessionError},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn invalid_reply() -> PluginFault {
    PluginFault::new(ErrorCode::Fault, "宿主回调返回了无效响应")
}

async fn call(
    host: &HostClient,
    method: &str,
    params: Value,
    payload: Vec<u8>,
) -> Result<(Value, Vec<u8>), PluginFault> {
    let reply = host
        .call(method, params, payload)
        .await
        .map_err(SessionError::into_plugin_fault)?;
    Ok((reply.result, reply.payload))
}

/// 请求和结果都在二进制载荷里的回调（账号接口）
async fn payload_call<I: Serialize, O: DeserializeOwned>(
    host: &HostClient,
    method: &str,
    input: &I,
) -> Result<O, PluginFault> {
    let body = serde_json::to_vec(input).map_err(|_| invalid_reply())?;
    let (_, payload) = call(host, method, json!({}), body).await?;
    serde_json::from_slice(&payload).map_err(|_| invalid_reply())
}

/// 控制参数和结果都是 JSON 元数据的回调（私有状态）
async fn metadata_call<I: Serialize, O: DeserializeOwned>(
    host: &HostClient,
    method: &str,
    input: &I,
) -> Result<O, PluginFault> {
    let params = serde_json::to_value(input).map_err(|_| invalid_reply())?;
    let (result, _) = call(host, method, params, Vec::new()).await?;
    serde_json::from_value(result).map_err(|_| invalid_reply())
}

/// CPR 的全部 OpenAI 账号，不含凭据
///
/// # Errors
/// 宿主回调失败时返回错误
pub async fn openai_accounts(host: &HostClient) -> Result<Vec<AuthRuntimeAccount>, PluginFault> {
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        let page: AuthListResult = payload_call(
            host,
            "host.auth.list",
            &AuthListRequest {
                provider_id: Some("openai".into()),
                cursor,
                limit: 200,
            },
        )
        .await?;
        out.extend(page.accounts);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(out),
        }
    }
}

/// 一个账号的运行投影，不含凭据
///
/// # Errors
/// 账号不存在或回调失败时返回错误
pub async fn account(
    host: &HostClient,
    account_id: &str,
) -> Result<AuthRuntimeAccount, PluginFault> {
    payload_call(
        host,
        "host.auth.get_runtime",
        &AuthGetRequest {
            account_id: account_id.to_owned(),
        },
    )
    .await
}

/// 一个账号当前的凭据。只在发探测前读，不保存
///
/// # Errors
/// 账号不存在或回调失败时返回错误
pub async fn credential(
    host: &HostClient,
    account_id: &str,
) -> Result<AuthCredential, PluginFault> {
    payload_call(
        host,
        "host.auth.get",
        &AuthGetRequest {
            account_id: account_id.to_owned(),
        },
    )
    .await
}

/// 读一条私有状态
///
/// # Errors
/// 回调失败时返回错误
pub async fn state_get(
    host: &HostClient,
    namespace: &str,
    key: &str,
) -> Result<Option<StateRecord>, PluginFault> {
    let result: StateGetResult = metadata_call(
        host,
        "host.state.get",
        &StateGetRequest {
            namespace: namespace.to_owned(),
            key: key.to_owned(),
        },
    )
    .await?;
    Ok(result.record)
}

/// 写一条私有状态；`expected_version` 为空表示只在不存在时创建
///
/// # Errors
/// 版本冲突（`Conflict`）、超出配额或回调失败时返回错误
pub async fn state_put(
    host: &HostClient,
    namespace: &str,
    key: &str,
    value: Value,
    expected_version: Option<u64>,
) -> Result<u64, PluginFault> {
    let result: StatePutResult = metadata_call(
        host,
        "host.state.put",
        &StatePutRequest {
            namespace: namespace.to_owned(),
            key: key.to_owned(),
            value,
            expected_version,
        },
    )
    .await?;
    Ok(result.version)
}

/// 删除一条私有状态
///
/// # Errors
/// 版本冲突或回调失败时返回错误
pub async fn state_delete(
    host: &HostClient,
    namespace: &str,
    key: &str,
    expected_version: u64,
) -> Result<(), PluginFault> {
    let _: Value = metadata_call(
        host,
        "host.state.delete",
        &StateDeleteRequest {
            namespace: namespace.to_owned(),
            key: key.to_owned(),
            expected_version,
        },
    )
    .await?;
    Ok(())
}

/// 经 CPR 的模型链对指定账号执行一次非流式请求，返回事件批次
///
/// # Errors
/// 准入、上游或回调失败时返回错误
pub async fn model_execute(
    host: &HostClient,
    request: &ModelExecuteRequest,
    body: &Value,
) -> Result<ModelEventBatch, PluginFault> {
    let params = serde_json::to_value(request).map_err(|_| invalid_reply())?;
    let body = serde_json::to_vec(body).map_err(|_| invalid_reply())?;
    let (result, payload) = call(host, "host.model.execute", params, body).await?;
    let _: ModelExecuteResult = serde_json::from_value(result).map_err(|_| invalid_reply())?;
    ModelEventBatch::decode(&payload).map_err(|_| invalid_reply())
}

/// 写一条宿主日志；字段由宿主脱敏，不能放凭据。日志失败不影响业务
pub async fn log(host: &HostClient, event: &str, level: LogLevel, fields: Value) {
    let fields = match fields {
        Value::Object(map) => map.into_iter().collect(),
        _ => std::collections::BTreeMap::new(),
    };
    let request = LogRequest {
        event: event.to_owned(),
        level,
        fields,
    };
    let _: Result<Value, _> = metadata_call(host, "host.log", &request).await;
}
