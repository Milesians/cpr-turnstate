//! 测票：KlN-4096/sub2api（v0.2.17-klno.1，“独立降智防护”）的回声判据
//!
//! Codex 后端在响应头 `X-Codex-Turn-State` 里发一张不透明的票，希望同一轮的下一个请求
//! 原样带回。正常的线路接受带回的票；降智的线路不停地换新票。
//!
//! 这是关于路由的启发式判断，不是哪个模型作答的证明，KlN 自己也这么说。票的长度只记录、
//! 不参与判定：2026-09-23 起上游也会发 780 字符的票，含义没人知道。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::verdict::{Level, UpstreamError, Verdict};

/// 带回票的次数。KlN 给新票三次确认
pub const ECHO_ATTEMPTS: usize = 3;
/// 每次都发的消息，只需要被回答
pub const ECHO_PROMPT: &str = "hi";

/// 一次请求带回来的东西
pub struct Shot {
    /// 响应带的票，没有就是空
    pub state: String,
    /// 响应改动了请求带的负载均衡 cookie：这次到了另一条线路，什么也证明不了
    pub route_changed: bool,
    pub duration_ms: u64,
}

/// 与上游的一次对话：同一个 session id 和 cookie，像真实 Codex 的一轮
#[allow(async_fn_in_trait)]
pub trait EchoSession {
    /// 发送测票消息；`state` 非空时带回它。非 200 或没完成的流是错误，不是判定
    async fn shoot(&mut self, model: &str, state: &str) -> Result<Shot, UpstreamError>;
}

pub const SHOT_MINTED: &str = "minted";
pub const SHOT_NO_TICKET: &str = "no_ticket";
pub const SHOT_ACCEPTED: &str = "accepted";
pub const SHOT_REFRESHED: &str = "refreshed";
pub const SHOT_ROUTE_CHANGED: &str = "route_changed";

/// 一次请求的记录。票本身是会话凭据，只存长度和短指纹
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    /// 0 是第一次、不带票的请求
    pub shot: usize,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sent_length: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sent_fingerprint: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub got_length: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub got_fingerprint: String,
    pub duration_ms: u64,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnStateResult {
    pub model: String,
    pub steps: Vec<Step>,
    /// 第一张票的长度，没发票为 0
    pub minted_length: usize,
    pub echoes: usize,
    pub refreshed: usize,
    /// 第几次带回（从 1 起）被接受，0 表示没有
    pub accepted_at: usize,
}

fn fingerprint(state: &str) -> String {
    if state.is_empty() {
        return String::new();
    }
    hex::encode(&Sha256::digest(state.as_bytes())[..4])
}

/// 先拿票，再带回最新的票，直到被接受或用完次数。遇到第一个上游错误就停
///
/// # Errors
/// 任一次请求失败时返回该错误
pub async fn run(
    session: &mut impl EchoSession,
    model: &str,
) -> Result<TurnStateResult, UpstreamError> {
    let mut result = TurnStateResult {
        model: model.to_owned(),
        steps: Vec::new(),
        minted_length: 0,
        echoes: 0,
        refreshed: 0,
        accepted_at: 0,
    };
    let first = session.shoot(model, "").await?;
    let mut step = Step {
        shot: 0,
        outcome: SHOT_NO_TICKET.into(),
        sent_length: 0,
        sent_fingerprint: String::new(),
        got_length: 0,
        got_fingerprint: String::new(),
        duration_ms: first.duration_ms,
    };
    if !first.state.is_empty() {
        step.outcome = SHOT_MINTED.into();
        step.got_length = first.state.len();
        step.got_fingerprint = fingerprint(&first.state);
        result.minted_length = first.state.len();
    }
    result.steps.push(step);
    if first.state.is_empty() {
        return Ok(result);
    }

    let mut state = first.state;
    for i in 1..=ECHO_ATTEMPTS {
        let shot = session.shoot(model, &state).await?;
        result.echoes = i;
        let outcome = if shot.route_changed {
            SHOT_ROUTE_CHANGED
        } else if shot.state.is_empty() || shot.state == state {
            SHOT_ACCEPTED
        } else {
            result.refreshed += 1;
            SHOT_REFRESHED
        };
        result.steps.push(Step {
            shot: i,
            outcome: outcome.into(),
            sent_length: state.len(),
            sent_fingerprint: fingerprint(&state),
            got_length: shot.state.len(),
            got_fingerprint: fingerprint(&shot.state),
            duration_ms: shot.duration_ms,
        });
        if outcome == SHOT_ACCEPTED {
            result.accepted_at = i;
            return Ok(result);
        }
        if !shot.state.is_empty() {
            state = shot.state; // 总是带回最新的票
        }
    }
    Ok(result)
}

/// 判定一次测票。只有每次带回都被换票才算降智；换过票后才被接受是疑似
#[must_use]
pub fn verdict(r: &TurnStateResult) -> Verdict {
    if r.minted_length == 0 {
        return Verdict::new(
            Level::Unknown,
            "上游没有发票（响应里没有 X-Codex-Turn-State），测票无从判断",
        );
    }
    if r.accepted_at == 1 {
        return Verdict::new(
            Level::Ok,
            format!(
                "拿到长度 {} 的票，原样带回后上游接受了它，没有换新票",
                r.minted_length
            ),
        );
    }
    if r.accepted_at > 1 {
        return Verdict::new(
            Level::Suspect,
            format!(
                "带回的票先被换了 {} 次新票，第 {} 次才被接受",
                r.refreshed, r.accepted_at
            ),
        );
    }
    if r.refreshed == ECHO_ATTEMPTS {
        return Verdict::new(
            Level::Degraded,
            format!(
                "连续 {} 次带回最新的票，每次都被换成新票（KlN 的判据：全部刷新即降智）",
                r.refreshed
            ),
        );
    }
    if r.refreshed > 0 {
        return Verdict::new(
            Level::Suspect,
            format!(
                "{} 次回带里有 {} 次被换成新票，其余落到了别的路由，没有一次被接受",
                r.echoes, r.refreshed
            ),
        );
    }
    Verdict::new(
        Level::Unknown,
        "每次回带都落到了别的路由（负载均衡 cookie 变了），无法下结论",
    )
}
