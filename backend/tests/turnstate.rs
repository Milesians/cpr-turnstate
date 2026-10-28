use cpr_account_probe::turnstate::{
    self, ECHO_ATTEMPTS, EchoSession, SHOT_ACCEPTED, SHOT_NO_TICKET, Shot,
};
use cpr_account_probe::verdict::{Level, UpstreamError};

/// 按脚本应答每一次请求，并记下被要求带回的票
struct Script {
    replies: Vec<(String, bool)>,
    /// 第几次（从 1 起）失败，0 表示不失败
    error_at: usize,
    sent: Vec<String>,
}

impl Script {
    fn new(replies: &[(&str, bool)]) -> Self {
        Self {
            replies: replies.iter().map(|(s, r)| ((*s).to_owned(), *r)).collect(),
            error_at: 0,
            sent: Vec::new(),
        }
    }
}

impl EchoSession for Script {
    async fn shoot(&mut self, _model: &str, state: &str) -> Result<Shot, UpstreamError> {
        self.sent.push(state.to_owned());
        if self.sent.len() == self.error_at {
            return Err(UpstreamError {
                status: Some(429),
                kind: "rate_limited".into(),
                message: "slow down".into(),
            });
        }
        let (state, route_changed) = self.replies[self.sent.len() - 1].clone();
        Ok(Shot {
            state,
            route_changed,
            duration_ms: 1,
        })
    }
}

fn ticket(c: char, n: usize) -> String {
    format!("gAAAAA{}", c.to_string().repeat(n - 6))
}

async fn run(script: &mut Script) -> turnstate::TurnStateResult {
    turnstate::run(script, "m").await.unwrap()
}

#[tokio::test]
async fn accepted_on_the_first_echo_is_ok() {
    let a = ticket('a', 780);
    let mut s = Script::new(&[(&a, false), (&a, false)]);
    let r = run(&mut s).await;
    assert_eq!(s.sent, vec![String::new(), a]);
    assert_eq!(
        (r.minted_length, r.accepted_at, r.echoes, r.refreshed),
        (780, 1, 1, 0)
    );
    let v = turnstate::verdict(&r);
    assert_eq!(v.level, Level::Ok);
    assert!(v.reason.contains("780"));
}

#[tokio::test]
async fn an_empty_answer_also_keeps_the_ticket() {
    let a = ticket('a', 292);
    let mut s = Script::new(&[(&a, false), ("", false)]);
    let r = run(&mut s).await;
    assert_eq!(r.accepted_at, 1);
    assert_eq!(turnstate::verdict(&r).level, Level::Ok);
}

#[tokio::test]
async fn refreshed_on_every_echo_is_degraded() {
    let (a, b, c, d) = (
        ticket('a', 780),
        ticket('b', 780),
        ticket('c', 780),
        ticket('d', 780),
    );
    let mut s = Script::new(&[(&a, false), (&b, false), (&c, false), (&d, false)]);
    let r = run(&mut s).await;
    // 每次带回最新的票
    assert_eq!(s.sent, vec![String::new(), a, b, c]);
    assert_eq!(
        (r.echoes, r.refreshed, r.accepted_at),
        (ECHO_ATTEMPTS, ECHO_ATTEMPTS, 0)
    );
    assert_eq!(turnstate::verdict(&r).level, Level::Degraded);
}

#[tokio::test]
async fn accepted_after_a_refresh_is_only_suspect() {
    let (a, b) = (ticket('a', 292), ticket('b', 292));
    let mut s = Script::new(&[(&a, false), (&b, false), (&b, false)]);
    let r = run(&mut s).await;
    assert_eq!((r.accepted_at, r.refreshed, s.sent.len()), (2, 1, 3));
    assert_eq!(turnstate::verdict(&r).level, Level::Suspect);
}

#[tokio::test]
async fn without_a_ticket_is_unknown_and_stops_at_once() {
    let mut s = Script::new(&[("", false)]);
    let r = run(&mut s).await;
    assert_eq!(s.sent.len(), 1);
    assert_eq!(r.steps[0].outcome, SHOT_NO_TICKET);
    assert_eq!(turnstate::verdict(&r).level, Level::Unknown);
}

#[tokio::test]
async fn another_route_proves_nothing() {
    let (a, b) = (ticket('a', 292), ticket('b', 292));
    let mut s = Script::new(&[(&a, false), (&b, true), (&b, true), ("", true)]);
    let r = run(&mut s).await;
    assert_eq!(
        (r.refreshed, r.accepted_at, r.echoes),
        (0, 0, ECHO_ATTEMPTS)
    );
    assert_eq!(turnstate::verdict(&r).level, Level::Unknown);
    // 仍然跟着最新的票走
    assert_eq!(s.sent[2], b);
}

#[tokio::test]
async fn mixed_refreshes_and_route_changes_are_suspect() {
    let (a, b, c, d) = (
        ticket('a', 292),
        ticket('b', 292),
        ticket('c', 292),
        ticket('d', 292),
    );
    let mut s = Script::new(&[(&a, false), (&b, false), (&c, true), (&d, false)]);
    let r = run(&mut s).await;
    assert_eq!(r.refreshed, 2);
    assert_eq!(turnstate::verdict(&r).level, Level::Suspect);
}

#[tokio::test]
async fn stops_at_the_first_upstream_error() {
    let a = ticket('a', 292);
    let mut s = Script::new(&[(&a, false)]);
    s.error_at = 2;
    let err = turnstate::run(&mut s, "m").await.unwrap_err();
    assert_eq!(err.kind, "rate_limited");
    assert_eq!(s.sent.len(), 2);
}

#[tokio::test]
async fn never_stores_a_ticket() {
    let (a, b) = (ticket('a', 780), ticket('b', 780));
    let mut s = Script::new(&[(&a, false), (&b, false), (&b, false)]);
    let r = run(&mut s).await;
    let raw = serde_json::to_string(&r).unwrap();
    assert!(!raw.contains("gAAAAA"), "ticket leaked: {raw}");
    assert!(!r.steps[1].sent_fingerprint.is_empty());
    assert_ne!(r.steps[1].sent_fingerprint, r.steps[1].got_fingerprint);
    assert_eq!(r.steps[2].sent_fingerprint, r.steps[2].got_fingerprint);
    assert_eq!(r.steps[2].outcome, SHOT_ACCEPTED);
}

#[test]
fn overload_is_suspect_only_when_the_upstream_says_so() {
    let overloaded = UpstreamError {
        status: Some(503),
        kind: "overloaded".into(),
        message: "Our servers are currently overloaded".into(),
    };
    assert_eq!(
        overloaded.overloaded_verdict().unwrap().level,
        Level::Suspect
    );
    let bare = UpstreamError {
        status: Some(503),
        kind: "overloaded".into(),
        message: "Service Unavailable".into(),
    };
    assert!(bare.overloaded_verdict().is_none());
}
