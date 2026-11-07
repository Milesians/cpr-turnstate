//! 探测历史的展示结构
//!
//! 每个账号每种方法的历史由两部分组成，同一条探测只在其中一处：
//! - 最近的逐条摘要（[`RAW_HOURS`] 小时内，或最近 [`RAW_KEEP`] 条），带判定理由，可点开详情
//! - 更早的按小时汇总的次数（[`Bucket`]），保留 [`crate::store::HISTORY_DAYS`] 天，只有各判定的次数
//!
//! 时间条、正常率和走势都按两部分合起来算，所以压缩旧记录不改变展示的次数。
//! 时间按北京时间对齐（格子边界落在整点、整天）。

use serde::{Deserialize, Serialize};

use crate::store::{Status, Summary};
use crate::verdict::Level;

pub const HOUR_MS: i64 = 3_600_000;
pub const DAY_MS: i64 = 24 * HOUR_MS;
/// 北京时间相对 UTC 的偏移
const OFFSET_MS: i64 = 8 * HOUR_MS;
/// 逐条摘要保留的小时数；更早的并进按小时汇总
pub const RAW_HOURS: i64 = 48;
/// 不论多早，每种方法至少保留最近这么多条逐条摘要（手动 ModelTrace 很少，记录不该被压掉）
pub const RAW_KEEP: usize = 50;

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// 一段时间里各结果的次数
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ok: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub suspect: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub degraded: u32,
    /// 探测完成但没有判定
    #[serde(default, skip_serializing_if = "is_zero")]
    pub unknown: u32,
    /// 探测失败
    #[serde(default, skip_serializing_if = "is_zero")]
    pub failed: u32,
    /// 还在进行；汇总里没有
    #[serde(default, skip_serializing_if = "is_zero")]
    pub running: u32,
}

impl Counts {
    pub const fn add(&mut self, s: &Summary) {
        match s.status {
            Status::Running => self.running += 1,
            Status::Failed => self.failed += 1,
            Status::Done => match s.level {
                Some(Level::Ok) => self.ok += 1,
                Some(Level::Suspect) => self.suspect += 1,
                Some(Level::Degraded) => self.degraded += 1,
                Some(Level::Unknown) | None => self.unknown += 1,
            },
        }
    }

    pub const fn merge(&mut self, o: &Self) {
        self.ok += o.ok;
        self.suspect += o.suspect;
        self.degraded += o.degraded;
        self.unknown += o.unknown;
        self.failed += o.failed;
        self.running += o.running;
    }

    #[must_use]
    pub const fn total(&self) -> u32 {
        self.ok + self.suspect + self.degraded + self.unknown + self.failed + self.running
    }

    /// 有效判定（正常、疑似、降智）的次数，正常率的分母
    #[must_use]
    pub const fn judged(&self) -> u32 {
        self.ok + self.suspect + self.degraded
    }

    /// 最严重的结果；失败和未判定算 unknown；只有进行中的没有结果
    #[must_use]
    pub const fn worst(&self) -> Option<Level> {
        if self.degraded > 0 {
            Some(Level::Degraded)
        } else if self.suspect > 0 {
            Some(Level::Suspect)
        } else if self.ok > 0 {
            Some(Level::Ok)
        } else if self.unknown > 0 || self.failed > 0 {
            Some(Level::Unknown)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.total() == 0
    }
}

/// 一小时里压缩掉的探测次数
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    /// 这一小时的开始（毫秒）
    pub at: i64,
    #[serde(flatten)]
    pub counts: Counts,
}

/// 把 `t` 按北京时间向下对齐到 `step` 的整数倍
#[must_use]
pub const fn floor(t: i64, step: i64) -> i64 {
    (t + OFFSET_MS).div_euclid(step) * step - OFFSET_MS
}

/// 并入一条摘要；`buckets` 按时间排列
pub fn fold(buckets: &mut Vec<Bucket>, s: &Summary) {
    let at = floor(s.started_ms, HOUR_MS);
    match buckets.binary_search_by_key(&at, |b| b.at) {
        Ok(i) => buckets[i].counts.add(s),
        Err(i) => {
            let mut counts = Counts::default();
            counts.add(s);
            buckets.insert(i, Bucket { at, counts });
        }
    }
}

/// 时间条的范围：从 `start_ms` 起 `slots` 格，每格 `slot_ms`，最后一格包含现在
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Range {
    pub hours: i64,
    pub start_ms: i64,
    pub slot_ms: i64,
    pub slots: usize,
}

impl Range {
    /// 只支持 24 小时（15 分钟一格）、7 天（2 小时一格）、30 天（6 小时一格）
    #[must_use]
    pub const fn new(hours: i64, now: i64) -> Option<Self> {
        let slot_ms = match hours {
            24 => 15 * 60_000,
            168 => 2 * HOUR_MS,
            720 => 6 * HOUR_MS,
            _ => return None,
        };
        let end = floor(now, slot_ms) + slot_ms;
        let start_ms = end - hours * HOUR_MS;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let slots = (hours * HOUR_MS / slot_ms) as usize;
        Some(Self {
            hours,
            start_ms,
            slot_ms,
            slots,
        })
    }

    #[must_use]
    pub const fn end_ms(&self) -> i64 {
        self.start_ms + self.slot_ms * self.slots as i64
    }

    fn index(&self, t: i64) -> Option<usize> {
        if t < self.start_ms || t >= self.end_ms() {
            return None;
        }
        usize::try_from((t - self.start_ms) / self.slot_ms).ok()
    }
}

/// 时间条里有探测的一格
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Cell {
    /// 第几格（从 0 开始）
    pub i: usize,
    #[serde(flatten)]
    pub counts: Counts,
}

/// 时间条：只列出有探测的格子，按格子排列
#[must_use]
pub fn strip(range: &Range, runs: &[Summary], buckets: &[Bucket]) -> Vec<Cell> {
    let mut cells = vec![Counts::default(); range.slots];
    for s in runs {
        if let Some(i) = range.index(s.started_ms) {
            cells[i].add(s);
        }
    }
    for b in buckets {
        if let Some(i) = range.index(b.at) {
            cells[i].merge(&b.counts);
        }
    }
    cells
        .into_iter()
        .enumerate()
        .filter(|(_, c)| !c.is_empty())
        .map(|(i, counts)| Cell { i, counts })
        .collect()
}

/// `[from, to)` 里的次数；按小时汇总的部分按它的开始时间算
#[must_use]
pub fn between(from: i64, to: i64, runs: &[Summary], buckets: &[Bucket]) -> Counts {
    let mut counts = Counts::default();
    for s in runs
        .iter()
        .filter(|s| s.started_ms >= from && s.started_ms < to)
    {
        counts.add(s);
    }
    for b in buckets.iter().filter(|b| b.at >= from && b.at < to) {
        counts.merge(&b.counts);
    }
    counts
}

/// `[from, to)` 里有没有只剩次数、没有逐条记录的部分
#[must_use]
pub fn has_buckets(from: i64, to: i64, buckets: &[Bucket]) -> bool {
    buckets
        .iter()
        .any(|b| b.at + HOUR_MS > from && b.at < to && !b.counts.is_empty())
}

/// 走势里的一天（北京时间）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Day {
    pub start_ms: i64,
    #[serde(flatten)]
    pub counts: Counts,
}

/// 最近 `days` 天每天的次数，旧的在前；最后一天是今天
#[must_use]
pub fn daily(days: i64, now: i64, runs: &[Summary], buckets: &[Bucket]) -> Vec<Day> {
    let today = floor(now, DAY_MS);
    (0..days)
        .map(|i| {
            let start_ms = today - (days - 1 - i) * DAY_MS;
            Day {
                start_ms,
                counts: between(start_ms, start_ms + DAY_MS, runs, buckets),
            }
        })
        .collect()
}
