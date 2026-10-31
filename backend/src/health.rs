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

fn abnormal(level: Level) -> bool {
    matches!(level, Level::Suspect | Level::Degraded)
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
            .is_some_and(|c| c == level || (abnormal(c) && abnormal(level)))
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub from: Level,
    pub to: Level,
    pub model: String,
    pub at_ms: i64,
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
