//! 展示结构：北京时间对齐、格子归属、逐条与按小时汇总合并计数

use cpr_account_probe::series::{
    self, Bucket, Counts, DAY_MS, HOUR_MS, Range, between, daily, floor, fold, has_buckets, strip,
};
use cpr_account_probe::store::{Status, Summary};
use cpr_account_probe::verdict::Level;

/// 2026-11-07 12:34:56 北京时间
const NOW: i64 = 1_794_026_096_000;

fn summary(started_ms: i64, status: Status, level: Option<Level>) -> Summary {
    Summary {
        id: format!("r{started_ms}"),
        model: "m".into(),
        trigger: "auto".into(),
        started_ms,
        finished_ms: started_ms + 1,
        status,
        level,
        reason: String::new(),
        detail: false,
    }
}

#[test]
fn ranges_align_to_beijing_time_and_end_after_now() {
    // 北京时间零点
    let midnight = floor(NOW, DAY_MS);
    assert_eq!((midnight + 8 * HOUR_MS) % DAY_MS, 0);
    for (hours, slot_ms, slots) in [
        (24, 900_000, 96),
        (168, 2 * HOUR_MS, 84),
        (720, 6 * HOUR_MS, 120),
    ] {
        let r = Range::new(hours, NOW).unwrap();
        assert_eq!(r.slot_ms, slot_ms);
        assert_eq!(r.slots, slots);
        assert_eq!(r.end_ms() - r.start_ms, hours * HOUR_MS);
        assert!(r.end_ms() > NOW && r.end_ms() - NOW <= slot_ms);
        assert_eq!((r.start_ms + 8 * HOUR_MS) % slot_ms, 0, "{hours}h");
    }
    assert!(Range::new(1, NOW).is_none());
}

#[test]
fn strip_merges_raw_runs_and_hourly_buckets() {
    let range = Range::new(168, NOW).unwrap();
    let runs = vec![
        summary(NOW - 60_000, Status::Done, Some(Level::Ok)),
        summary(NOW - 30_000, Status::Running, None),
    ];
    let mut buckets = Vec::new();
    let old = NOW - 3 * DAY_MS;
    fold(
        &mut buckets,
        &summary(old, Status::Done, Some(Level::Degraded)),
    );
    fold(&mut buckets, &summary(old + 60_000, Status::Failed, None));
    // 范围外的不计
    fold(
        &mut buckets,
        &summary(NOW - 20 * DAY_MS, Status::Done, Some(Level::Ok)),
    );
    assert_eq!(buckets.len(), 2);

    let cells = strip(&range, &runs, &buckets);
    assert_eq!(cells.len(), 2);
    let last = cells.last().unwrap();
    assert_eq!(last.i, range.slots - 1);
    assert_eq!((last.counts.ok, last.counts.running), (1, 1));
    assert_eq!(last.counts.worst(), Some(Level::Ok));
    let older = cells[0];
    assert_eq!((older.counts.degraded, older.counts.failed), (1, 1));
    assert_eq!(older.counts.worst(), Some(Level::Degraded));

    let total = between(range.start_ms, range.end_ms(), &runs, &buckets);
    assert_eq!(total.total(), 4);
    assert_eq!(total.judged(), 2);
    assert!(has_buckets(old - HOUR_MS, old + 1, &buckets));
    assert!(!has_buckets(NOW - DAY_MS, NOW, &buckets));
}

#[test]
fn worst_ranks_degraded_over_suspect_over_ok_over_unknown() {
    let mut c = Counts::default();
    assert_eq!(c.worst(), None);
    c.running = 1;
    assert_eq!(c.worst(), None, "only running has no verdict yet");
    c.failed = 1;
    assert_eq!(c.worst(), Some(Level::Unknown));
    c.ok = 1;
    assert_eq!(c.worst(), Some(Level::Ok));
    c.suspect = 1;
    assert_eq!(c.worst(), Some(Level::Suspect));
    c.degraded = 1;
    assert_eq!(c.worst(), Some(Level::Degraded));
}

#[test]
fn daily_covers_thirty_beijing_days_ending_today() {
    let runs = vec![summary(NOW - 60_000, Status::Done, Some(Level::Ok))];
    let mut buckets: Vec<Bucket> = Vec::new();
    fold(
        &mut buckets,
        &summary(NOW - 29 * DAY_MS, Status::Done, Some(Level::Suspect)),
    );
    let days = daily(30, NOW, &runs, &buckets);
    assert_eq!(days.len(), 30);
    assert_eq!(days.last().unwrap().start_ms, floor(NOW, DAY_MS));
    assert_eq!(days.last().unwrap().counts.ok, 1);
    assert_eq!(days[0].counts.suspect, 1);
    assert_eq!(days.iter().map(|d| d.counts.total()).sum::<u32>(), 2);
    assert_eq!(series::RAW_HOURS, 48);
}
