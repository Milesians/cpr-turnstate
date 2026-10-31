//! Telegram delivery uses the host HTTP client and never returns transport errors/URLs.

use std::time::Duration;

use gateway_plugin_sdk::{
    call::host::{AuthRuntimeAccount, HttpRequest, LogLevel},
    client::HostClient,
};
use serde_json::json;

use crate::{
    config::Config,
    health::Notification,
    host, store,
    verdict::{Level, one_line},
};

pub fn configuration_error(config: &Config) -> Option<&'static str> {
    let token = config.telegram_bot_token.trim();
    let valid_token = token.split_once(':').is_some_and(|(id, secret)| {
        !id.is_empty()
            && id.bytes().all(|b| b.is_ascii_digit())
            && !secret.is_empty()
            && secret
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    });
    if !valid_token || token.len() > 256 {
        return Some("请在插件敏感配置中填写有效的 TG Bot Token");
    }
    let chat = config.telegram_chat_id.trim();
    if chat.is_empty() || chat.len() > 128 || chat.chars().any(char::is_control) {
        return Some("请在插件设置中填写有效的 TG Chat ID");
    }
    None
}

fn label(level: Level) -> &'static str {
    match level {
        Level::Unknown => "未判定",
        Level::Ok => "正常",
        Level::Suspect => "疑似",
        Level::Degraded => "降智",
    }
}

fn message(account: &AuthRuntimeAccount, event: &Notification) -> String {
    let title = if event.to == Level::Ok {
        "测票恢复正常"
    } else {
        "测票异常确认"
    };
    format!(
        "CPR 账号探测 | {title}\n账号：{}\n账号 ID：{}\n判定：{} → {}\n模型：{}\n连续 3 次{}，已更新当前判定。\n事件 ID：{}\n确认时间（Unix 毫秒）：{}",
        one_line(&account.name, 120),
        one_line(&account.account_id, 100),
        label(event.from),
        label(event.to),
        one_line(&event.model, 100),
        if event.to == Level::Ok {
            "测票正常"
        } else {
            "测票降智或疑似"
        },
        one_line(&event.id, 100),
        event.at_ms,
    )
}

pub struct SendError {
    pub message: String,
    pub retry_after_ms: Option<i64>,
}

impl From<&str> for SendError {
    fn from(message: &str) -> Self {
        Self {
            message: message.into(),
            retry_after_ms: None,
        }
    }
}

async fn send_inner(host: &HostClient, config: &Config, text: &str) -> Result<(), SendError> {
    if let Some(error) = configuration_error(config) {
        return Err(error.into());
    }
    let mut body = json!({
        "chat_id": config.telegram_chat_id.trim(), "text": text,
        "link_preview_options": {"is_disabled": true},
    });
    if config.telegram_thread_id != 0 {
        body["message_thread_id"] = json!(config.telegram_thread_id);
    }
    let response = host
        .http(
            HttpRequest {
                method: "POST".into(),
                url: format!(
                    "https://api.telegram.org/bot{}/sendMessage",
                    config.telegram_bot_token.trim()
                ),
                headers: vec![("Content-Type".into(), b"application/json".to_vec())],
            },
            serde_json::to_vec(&body).map_err(|_| SendError::from("TG 请求编码失败"))?,
        )
        .await
        .map_err(|_| SendError::from("无法连接 Telegram，请检查服务器网络"))?;
    let status = response.status;
    let body = response
        .body
        .collect(16 * 1024)
        .await
        .map_err(|_| SendError::from("读取 TG 响应失败"))?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| SendError::from("TG 返回无效响应"))?;
    if status == 200 && value["ok"] == true {
        return Ok(());
    }
    // Telegram descriptions and host errors may contain request data, including the token.
    let message = match value["error_code"].as_u64().unwrap_or(u64::from(status)) {
        401 => "TG 认证失败，请检查 Bot Token",
        403 => "TG 拒绝发送，请检查机器人是否被屏蔽或缺少群组权限",
        400 => "TG 参数无效，请检查 Chat ID、话题 ID，并先向机器人发送 /start",
        429 => "TG 发送限流，稍后自动重试",
        _ => "TG 发送失败，稍后自动重试",
    };
    Err(SendError {
        message: message.into(),
        retry_after_ms: value["parameters"]["retry_after"]
            .as_i64()
            .filter(|s| *s > 0)
            .map(|s| s.min(86_400) * 1000),
    })
}

pub async fn send(host: &HostClient, config: &Config, text: &str) -> Result<(), SendError> {
    let budget = host
        .remaining_budget()
        .unwrap_or(Duration::from_secs(120))
        .saturating_sub(Duration::from_secs(3))
        .min(Duration::from_secs(8));
    tokio::time::timeout(budget, send_inner(host, config, text))
        .await
        .unwrap_or_else(|_| Err("TG 请求超时，稍后自动重试".into()))
}

/// At-least-once delivery: a crash after Telegram accepts but before ACK may duplicate.
pub async fn dispatch(host: &HostClient, config: &Config, account: &AuthRuntimeAccount) -> bool {
    if host
        .remaining_budget()
        .is_some_and(|b| b < Duration::from_secs(12))
    {
        return false;
    }
    let event =
        match store::claim_notification(host, &account.account_id, config.telegram_enabled).await {
            Ok(Some(event)) => event,
            Ok(None) => return false,
            Err(_) => {
                host::log(host, "telegram_queue_failed", LogLevel::Warn, json!({})).await;
                return false;
            }
        };
    let result = send(host, config, &message(account, &event)).await;
    let (error, retry_after) = match result {
        Ok(()) => (None, None),
        Err(e) => (Some(e.message), e.retry_after_ms),
    };
    if error.is_some() {
        host::log(
            host,
            "telegram_send_failed",
            LogLevel::Warn,
            json!({"account_id": account.account_id}),
        )
        .await;
    }
    if store::finish_notification(host, &account.account_id, &event, error, retry_after)
        .await
        .is_err()
    {
        host::log(host, "telegram_ack_failed", LogLevel::Warn, json!({})).await;
    }
    true
}
