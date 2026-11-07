//! Persistent, debounced turn-state health; raw probe verdicts remain unchanged.

use serde::{Deserialize, Serialize};

use crate::store::{Status, Summary};
use crate::verdict::Level;

pub const REQUIRED: u8 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    pub level: Level,
    pub model: String,
    pub candidate: Option<Level>,
    pub streak: u8,
    pub changed_ms: i64,
    pub last_run_id: String,
    pub last_started_ms: i64,
}

impl Default for Health {
    fn default() -> Self {
        Self {
            level: Level::Unknown,
            model: String::new(),
            candidate: None,
            streak: 0,
            changed_ms: 0,
            last_run_id: String::new(),
            last_started_ms: 0,
        }
    }
}

impl Health {
    /// Preserve the latest legacy verdict as a baseline, without retroactive alerts.
    pub fn from_history(runs: &[Summary]) -> Self {
        let latest = runs.iter().rev().find(|r| r.status != Status::Running);
        let judged = runs
            .iter()
            .rev()
            .find(|r| r.status == Status::Done && r.level.is_some_and(|l| l != Level::Unknown));
        Self {
            level: judged.and_then(|r| r.level).unwrap_or(Level::Unknown),
            model: latest.map(|r| r.model.clone()).unwrap_or_default(),
            changed_ms: judged.map_or(0, |r| r.finished_ms),
            last_run_id: latest.map(|r| r.id.clone()).unwrap_or_default(),
            last_started_ms: latest.map_or(0, |r| r.started_ms),
            ..Self::default()
        }
    }

    /// Returns a transition only once. Failures/unknowns break a streak, not health.
    pub fn observe(&mut self, run: &Summary) -> Option<(Level, Level)> {
        if run.status == Status::Running
            || run.id == self.last_run_id
            || run.started_ms < self.last_started_ms
        {
            return None;
        }
        self.last_run_id.clone_from(&run.id);
        self.last_started_ms = run.started_ms;
        if self.model != run.model {
            self.candidate = None;
            self.streak = 0;
            self.model.clone_from(&run.model);
        }
        let level = run.level.filter(|l| *l != Level::Unknown);
        if run.status != Status::Done || level.is_none() {
            self.candidate = None;
            self.streak = 0;
            return None;
        }
        let level = level.unwrap();
        if self
            .candidate
            .is_some_and(|c| c == level || (c.abnormal() && level.abnormal()))
        {
            self.candidate = Some(self.candidate.unwrap().max(level));
            self.streak += 1;
        } else {
            self.candidate = Some(level);
            self.streak = 1;
        }
        if self.streak < REQUIRED {
            return None;
        }
        let next = self.candidate.take().unwrap();
        self.streak = 0;
        if next == self.level {
            return None;
        }
        let previous = self.level;
        self.level = next;
        self.changed_ms = run.finished_ms;
        Some((previous, next))
    }
}

/// 账号本身的问题（凭据失效、令牌过期等）让测票发不出去。不是判定，不改当前判定
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Blocked {
    /// 拦截原因的代码，如 `credential_state`、`token_expired`
    pub code: String,
    pub message: String,
    /// 从什么时候开始一直受阻
    pub since_ms: i64,
    /// 持续 [`BLOCK_CONFIRM_MS`] 后确认（并通知）的时间；0 表示还没确认
    #[serde(default)]
    pub confirmed_ms: i64,
}

/// 受阻持续这么久才确认：CPR 刷新令牌通常几分钟内完成，不为一闪而过的过期打扰人
pub const BLOCK_CONFIRM_MS: i64 = 30 * 60 * 1000;

/// 通知的种类；旧版队列里的都是判定变化
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    #[default]
    Verdict,
    /// 自动测票受阻已确认
    Blocked,
    /// 受阻解除
    Unblocked,
}

impl NoticeKind {
    #[allow(clippy::trivially_copy_pass_by_ref)]
    const fn is_verdict(&self) -> bool {
        matches!(self, Self::Verdict)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    #[serde(default, skip_serializing_if = "NoticeKind::is_verdict")]
    pub kind: NoticeKind,
    pub from: Level,
    pub to: Level,
    pub model: String,
    pub at_ms: i64,
    /// 受阻通知：原因和开始时间
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<Blocked>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub retry_at_ms: i64,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthView {
    #[serde(flatten)]
    pub health: Health,
    pub notification_pending: usize,
    pub notification_error: Option<String>,
    pub notifications_dropped: u32,
}
