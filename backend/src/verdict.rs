//! 判定级别与上游错误

use serde::{Deserialize, Serialize};

/// 判定级别，按严重程度排序
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Unknown,
    Ok,
    Suspect,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub level: Level,
    pub reason: String,
}

impl Verdict {
    pub fn new(level: Level, reason: impl Into<String>) -> Self {
        Self {
            level,
            reason: reason.into(),
        }
    }
}

/// 失败的上游请求；`kind` 让运行器提前停止，不重复注定失败的请求
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamError {
    /// HTTP 状态，非 200 应答时才有
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// unauthorized、forbidden、rate_limited、overloaded、network、protocol、host、other
    pub kind: String,
    /// 已去掉凭据
    pub message: String,
}

impl UpstreamError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            status: None,
            kind: kind.to_owned(),
            message: message.into(),
        }
    }

    /// 上游明确说自己过载（消息里有 overload）时按疑似处理，计入状态条；
    /// 没有这个字样的 503 仍是探测失败
    #[must_use]
    pub fn overloaded_verdict(&self) -> Option<Verdict> {
        (self.kind == "overloaded" && self.message.to_lowercase().contains("overload")).then(|| {
            Verdict::new(
                Level::Suspect,
                format!("上游返回过载（{self}），按疑似降智处理"),
            )
        })
    }
}

impl std::fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            Some(status) => write!(f, "上游 {}（HTTP {status}）：{}", self.kind, self.message),
            None => write!(f, "上游 {}：{}", self.kind, self.message),
        }
    }
}

/// 把空白压成一个空格，截到 `n` 个字符
#[must_use]
pub fn one_line(s: &str, n: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    excerpt(&joined, n)
}

/// 最多 `n` 个字符，超出时加省略号
#[must_use]
pub fn excerpt(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(n).collect();
        out.push('…');
        out
    }
}
