//! 测票发给 ChatGPT Codex 后端的请求，以及流式响应的解析
//!
//! 请求复刻 sub2api 给 OpenAI OAuth 账号“测试连接”时发的那个：同样的请求头、同样的最小
//! 请求体、同样的 Codex 身份；测票多带 `Session_id`、`X-Codex-Turn-State` 和
//! `reasoning.effort=low`。这里不刷新令牌、不写任何东西。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::verdict::{UpstreamError, one_line};

/// Codex 后端发票、要求带回的响应头
pub const TURN_STATE_HEADER: &str = "X-Codex-Turn-State";
/// OAuth 账号发 Codex 请求的地址
pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
/// sub2api 自称的客户端：Codex 终端界面
pub const ORIGINATOR: &str = "codex-tui";
const USER_AGENT_SUFFIX: &str = " (Ubuntu 22.4.0; x86_64) xterm-256color";
const ECHO_REASONING_EFFORT: &str = "low";

/// 每个请求都要带的 Codex 基础提示词：openai/codex 的
/// `codex-rs/core/gpt_5_codex_prompt.md`，原样未改，来源见 `data/README.md`
pub const INSTRUCTIONS: &str = include_str!("../data/gpt_5_codex_prompt.md");

/// 负载均衡 cookie（Cloudflare 与 OpenAI 的），把客户端钉在一条后端线路上
const ROUTE_COOKIES: &[&str] = &["__cflb", "__oailb"];

const MAX_OUTPUT: usize = 256 * 1024;
const MAX_ERROR_BODY: usize = 4096;

/// 一个账号的凭据。`access_token` 是密钥：不能进日志、错误或记录
pub struct Credentials {
    pub access_token: String,
    pub chatgpt_account_id: String,
}

/// 校验模型名，避免把任意字符写进请求
#[must_use]
pub fn valid_model(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(b))
}

fn header_safe(s: &str) -> bool {
    !s.is_empty() && !s.contains(['\r', '\n', '\0'])
}

/// 请求头：名字与原始字节，保留顺序
pub type Headers = Vec<(String, Vec<u8>)>;

/// 测票一次请求的头和体
///
/// # Errors
/// 凭据、模型或票不能放进请求头时返回错误
pub fn echo_request(
    credentials: &Credentials,
    codex_version: &str,
    model: &str,
    session_id: &str,
    turn_state: &str,
    cookie: &str,
) -> Result<(Headers, Vec<u8>), UpstreamError> {
    if !valid_model(model) {
        return Err(UpstreamError::new("other", "模型名无效"));
    }
    if !header_safe(&credentials.access_token) {
        return Err(UpstreamError::new("other", "访问令牌缺失或格式不对"));
    }
    if !header_safe(&credentials.chatgpt_account_id) {
        return Err(UpstreamError::new(
            "other",
            "ChatGPT 账号 ID 缺失或格式不对",
        ));
    }
    if !turn_state.is_empty() && !header_safe(turn_state) {
        return Err(UpstreamError::new("protocol", "上游发的票不能放进请求头"));
    }
    let mut headers = vec![
        ("Content-Type".to_owned(), b"application/json".to_vec()),
        (
            "Authorization".to_owned(),
            format!("Bearer {}", credentials.access_token).into_bytes(),
        ),
        ("Accept".to_owned(), b"text/event-stream".to_vec()),
        ("Openai-Beta".to_owned(), b"responses=experimental".to_vec()),
        ("Originator".to_owned(), ORIGINATOR.as_bytes().to_vec()),
        (
            "User-Agent".to_owned(),
            format!("{ORIGINATOR}/{codex_version}{USER_AGENT_SUFFIX}").into_bytes(),
        ),
        ("Version".to_owned(), codex_version.as_bytes().to_vec()),
        (
            "Chatgpt-Account-Id".to_owned(),
            credentials.chatgpt_account_id.as_bytes().to_vec(),
        ),
        ("Session_id".to_owned(), session_id.as_bytes().to_vec()),
    ];
    if !turn_state.is_empty() {
        headers.push((TURN_STATE_HEADER.to_owned(), turn_state.as_bytes().to_vec()));
    }
    if !cookie.is_empty() {
        headers.push(("Cookie".to_owned(), cookie.as_bytes().to_vec()));
    }
    let body = json!({
        "input": [{
            "content": [{"text": crate::turnstate::ECHO_PROMPT, "type": "input_text"}],
            "role": "user",
        }],
        "instructions": INSTRUCTIONS,
        "model": model,
        "reasoning": {"effort": ECHO_REASONING_EFFORT},
        "store": false,
        "stream": true,
    });
    let body =
        serde_json::to_vec(&body).map_err(|_| UpstreamError::new("other", "请求编码失败"))?;
    Ok((headers, body))
}

/// ModelTrace 一道题经 CPR 模型链发送的 Responses 请求体
#[must_use]
pub fn challenge_body(model: &str, prompt: &str) -> Value {
    json!({
        "model": model,
        "input": [{
            "role": "user",
            "content": [{"type": "input_text", "text": prompt}],
        }],
        "instructions": INSTRUCTIONS,
        "store": false,
        "stream": false,
    })
}

/// 一次测票对话私有的 cookie：只记名字和值，全部发给 chatgpt.com
#[derive(Default)]
pub struct CookieJar {
    cookies: Vec<(String, String)>,
}

impl CookieJar {
    /// 请求要带的 Cookie 头
    #[must_use]
    pub fn header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// 收下响应的 Set-Cookie
    pub fn store(&mut self, headers: &[(String, Vec<u8>)]) {
        for (name, value) in headers {
            if !name.eq_ignore_ascii_case("set-cookie") {
                continue;
            }
            let Ok(value) = std::str::from_utf8(value) else {
                continue;
            };
            let mut parts = value.split(';');
            let Some((key, val)) = parts.next().and_then(|p| p.split_once('=')) else {
                continue;
            };
            let (key, val) = (key.trim(), val.trim());
            if key.is_empty() || !header_safe(key) || val.contains(['\r', '\n', '\0']) {
                continue;
            }
            let expired = parts.any(|attr| {
                let attr = attr.trim().to_ascii_lowercase();
                attr == "max-age=0" || attr.starts_with("max-age=-")
            });
            self.cookies.retain(|(k, _)| k != key);
            if !expired && !val.is_empty() {
                self.cookies.push((key.to_owned(), val.to_owned()));
            }
        }
    }

    /// 会话被钉到的线路；没有负载均衡 cookie 时为空
    #[must_use]
    pub fn route(&self) -> String {
        let mut out = String::new();
        for name in ROUTE_COOKIES {
            if let Some((_, v)) = self.cookies.iter().find(|(k, _)| k == name) {
                out.push_str(&format!("{name}={v};"));
            }
        }
        out
    }
}

/// 读完的流
#[derive(Debug, Default)]
pub struct StreamOutcome {
    pub text: String,
    pub response_model: String,
    /// `response.metadata` 事件里重复的票
    pub metadata_turn_state: String,
    /// 最后事件的 `response.status`（completed、incomplete）
    pub status: String,
}

#[derive(Deserialize, Default)]
struct Event {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    delta: String,
    #[serde(default)]
    headers: serde_json::Map<String, Value>,
    #[serde(default)]
    response: EventResponse,
    #[serde(default)]
    error: Option<EventError>,
}

#[derive(Deserialize, Default)]
struct EventResponse {
    #[serde(default)]
    model: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    output: Vec<OutputItem>,
    #[serde(default)]
    error: Option<EventError>,
}

#[derive(Deserialize, Default)]
struct OutputItem {
    #[serde(default)]
    content: Vec<OutputContent>,
}

#[derive(Deserialize, Default)]
struct OutputContent {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize, Default)]
struct EventError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    code: String,
    #[serde(default, rename = "type")]
    kind: String,
}

/// 逐块喂入 SSE 字节，读到完成事件为止
#[derive(Default)]
pub struct StreamReader {
    buffer: Vec<u8>,
    text: String,
    outcome: StreamOutcome,
}

impl StreamReader {
    /// 喂入一块；读到结束时返回结果，失败事件返回错误
    pub fn feed(&mut self, chunk: &[u8]) -> Option<Result<StreamOutcome, UpstreamError>> {
        self.buffer.extend_from_slice(chunk);
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            if let Some(done) = self.line(&line) {
                return Some(done);
            }
        }
        None
    }

    /// 流结束；没见到完成事件就是协议错误
    pub fn finish(&mut self) -> Result<StreamOutcome, UpstreamError> {
        let rest = std::mem::take(&mut self.buffer);
        if let Some(done) = self.line(&rest) {
            return done;
        }
        Err(UpstreamError::new(
            "protocol",
            "流在 response.completed 之前结束",
        ))
    }

    fn line(&mut self, raw: &[u8]) -> Option<Result<StreamOutcome, UpstreamError>> {
        let line = String::from_utf8_lossy(raw);
        let data = line.trim().strip_prefix("data:")?.trim().to_owned();
        if data == "[DONE]" {
            return Some(Err(UpstreamError::new(
                "protocol",
                "流在 response.completed 之前结束",
            )));
        }
        let event: Event = serde_json::from_str(&data).ok()?;
        if !event.response.model.is_empty() {
            self.outcome
                .response_model
                .clone_from(&event.response.model);
        }
        match event.kind.as_str() {
            "response.metadata" => {
                for (k, v) in &event.headers {
                    if let Some(s) = v.as_str()
                        && k.eq_ignore_ascii_case(TURN_STATE_HEADER)
                        && !s.trim().is_empty()
                    {
                        self.outcome.metadata_turn_state = s.trim().to_owned();
                    }
                }
                None
            }
            "response.output_text.delta" => {
                if self.text.len() < MAX_OUTPUT {
                    self.text.push_str(&event.delta);
                }
                None
            }
            "response.completed" | "response.done" | "response.incomplete" => {
                let mut outcome = std::mem::take(&mut self.outcome);
                outcome.status = event.response.status.clone();
                outcome.text = std::mem::take(&mut self.text);
                if outcome.text.is_empty() {
                    for item in &event.response.output {
                        for c in &item.content {
                            if c.kind == "output_text" {
                                outcome.text.push_str(&c.text);
                            }
                        }
                    }
                }
                Some(Ok(outcome))
            }
            "response.failed" => Some(Err(failure(event.response.error, "响应失败"))),
            "error" => Some(Err(failure(event.error, "上游发来错误"))),
            _ => None,
        }
    }
}

fn failure(error: Option<EventError>, fallback: &str) -> UpstreamError {
    let error = error.unwrap_or_default();
    let message = if error.message.is_empty() {
        fallback.to_owned()
    } else {
        error.message
    };
    let code = if error.code.is_empty() {
        error.kind
    } else {
        error.code
    };
    UpstreamError::new(classify(&code, &message), one_line(&message, 300))
}

/// 按错误代码或消息归类
#[must_use]
pub fn classify(code: &str, message: &str) -> &'static str {
    let s = format!("{code} {message}").to_lowercase();
    if s.contains("overload") {
        "overloaded"
    } else if ["rate_limit", "rate limit", "usage_limit", "usage limit"]
        .iter()
        .any(|k| s.contains(k))
    {
        "rate_limited"
    } else if ["unauthorized", "invalid_api_key", "token_expired"]
        .iter()
        .any(|k| s.contains(k))
    {
        "unauthorized"
    } else {
        "other"
    }
}

/// 按 HTTP 状态归类
#[must_use]
pub fn status_kind(status: u16) -> &'static str {
    match status {
        401 => "unauthorized",
        403 => "forbidden",
        429 => "rate_limited",
        503 | 529 => "overloaded",
        _ => "other",
    }
}

/// 从错误正文里挑出可读的消息：常见的 JSON 形状，否则前几百字节
#[must_use]
pub fn error_text(body: &[u8]) -> String {
    let body = &body[..body.len().min(MAX_ERROR_BODY)];
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        for key in ["error", "detail"] {
            match value.get(key) {
                Some(Value::String(s)) if !s.is_empty() => return s.clone(),
                Some(Value::Object(o)) => {
                    let code = o.get("code").and_then(Value::as_str).unwrap_or_default();
                    let message = o.get("message").and_then(Value::as_str).unwrap_or_default();
                    if !code.is_empty() || !message.is_empty() {
                        return format!("{code} {message}").trim().to_owned();
                    }
                }
                _ => {}
            }
        }
    }
    String::from_utf8_lossy(body).into_owned()
}

/// 去掉上游可能回显的凭据
#[must_use]
pub fn redact(s: &str, credentials: &Credentials) -> String {
    let mut out = s.to_owned();
    for secret in [&credentials.access_token, &credentials.chatgpt_account_id] {
        if !secret.is_empty() {
            out = out.replace(secret.as_str(), "‹hidden›");
        }
    }
    redact_bearer(&out)
}

fn redact_bearer(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find("bearer") {
        let start = i + pos;
        out.push_str(&s[i..start]);
        let mut j = start + "bearer".len();
        let spaces = s[j..].len() - s[j..].trim_start().len();
        j += spaces;
        let token_len = s[j..]
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || b"._-~+/=".contains(b))
            .count();
        if spaces > 0 && token_len > 0 {
            out.push_str("Bearer ‹hidden›");
            i = j + token_len;
        } else {
            out.push_str(&s[start..start + "bearer".len()]);
            i = start + "bearer".len();
        }
    }
    out.push_str(&s[i..]);
    out
}

/// 新的 v4 UUID，作为一次测票的 session id
///
/// # Errors
/// 随机数不可用时返回错误
pub fn new_uuid() -> Result<String, String> {
    let mut b = [0_u8; 16];
    crate::fingerprint::system_random(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Ok(format!(
        "{}-{}-{}-{}-{}",
        hex::encode(&b[0..4]),
        hex::encode(&b[4..6]),
        hex::encode(&b[6..8]),
        hex::encode(&b[8..10]),
        hex::encode(&b[10..16])
    ))
}
