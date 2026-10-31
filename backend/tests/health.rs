use cpr_account_probe::{
    health::Health,
    store::{Status, Summary},
    verdict::Level,
};

fn result(n: i64, level: Option<Level>) -> Summary {
    Summary {
        id: format!("r{n}"),
        model: "model-a".into(),
        trigger: "manual".into(),
        started_ms: n,
        finished_ms: n + 1,
        status: if level.is_some() {
            Status::Done
        } else {
            Status::Failed
        },
        level,
        reason: String::new(),
        detail: true,
    }
}

#[test]
fn recovery_and_regression_both_require_three_results() {
    for previous in [Level::Suspect, Level::Degraded] {
        let mut health = Health {
            level: previous,
            ..Health::default()
        };
        assert_eq!(health.observe(&result(1, Some(Level::Ok))), None);
        assert_eq!(health.observe(&result(2, Some(Level::Ok))), None);
        assert_eq!(health.level, previous);
        assert_eq!(
            health.observe(&result(3, Some(Level::Ok))),
            Some((previous, Level::Ok))
        );
        assert_eq!(health.observe(&result(4, Some(Level::Suspect))), None);
        assert_eq!(health.observe(&result(5, Some(Level::Degraded))), None);
        assert_eq!(
            health.observe(&result(6, Some(Level::Suspect))),
            Some((Level::Ok, Level::Degraded))
        );
        assert_eq!(health.observe(&result(6, Some(Level::Suspect))), None);
        assert_eq!(health.observe(&result(2, Some(Level::Ok))), None);
        assert_eq!(health.level, Level::Degraded);
    }
}

#[test]
fn interruptions_failures_and_unknowns_do_not_change_health() {
    for interruption in [None, Some(Level::Unknown), Some(Level::Degraded)] {
        let mut health = Health {
            level: Level::Suspect,
            ..Health::default()
        };
        health.observe(&result(1, Some(Level::Ok)));
        health.observe(&result(2, Some(Level::Ok)));
        health.observe(&result(3, interruption));
        assert_eq!(health.observe(&result(4, Some(Level::Ok))), None);
        assert_eq!(health.level, Level::Suspect);
        assert_eq!(health.streak, 1);
        let mut running = result(5, None);
        running.status = Status::Running;
        health.observe(&running);
        assert_eq!(health.streak, 1);
    }
}

#[test]
fn model_switches_reset_streak_and_restart_preserves_it() {
    let mut health = Health {
        level: Level::Degraded,
        ..Health::default()
    };
    health.observe(&result(1, Some(Level::Ok)));
    health.observe(&result(2, Some(Level::Ok)));
    let saved = serde_json::to_value(&health).unwrap();
    let mut restored: Health = serde_json::from_value(saved).unwrap();
    assert_eq!(
        restored.observe(&result(3, Some(Level::Ok))),
        Some((Level::Degraded, Level::Ok))
    );
    let mut changed = result(3, Some(Level::Ok));
    changed.model = "model-b".into();
    assert_eq!(health.observe(&changed), None);
    assert_eq!(health.streak, 1);
    assert_eq!(health.level, Level::Degraded);
}

#[test]
fn initial_health_requires_three_and_stable_states_do_not_repeat() {
    let mut health = Health::default();
    for n in 1..=2 {
        assert_eq!(health.observe(&result(n, Some(Level::Ok))), None);
        assert_eq!(health.level, Level::Unknown);
    }
    assert_eq!(
        health.observe(&result(3, Some(Level::Ok))),
        Some((Level::Unknown, Level::Ok))
    );
    for n in 4..=12 {
        assert_eq!(health.observe(&result(n, Some(Level::Ok))), None);
    }
    for n in 13..=14 {
        health.observe(&result(n, Some(Level::Suspect)));
    }
    assert_eq!(
        health.observe(&result(15, Some(Level::Suspect))),
        Some((Level::Ok, Level::Suspect))
    );
}

#[test]
fn legacy_baseline_uses_latest_judged_result_not_worst_in_window() {
    let history = [
        result(1, Some(Level::Degraded)),
        result(2, Some(Level::Ok)),
        result(3, None),
    ];
    let baseline = Health::from_history(&history);
    assert_eq!(baseline.level, Level::Ok);
    assert_eq!(baseline.streak, 0);
    assert_eq!(baseline.last_run_id, "r3");
}
