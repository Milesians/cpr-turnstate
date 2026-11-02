//! 自动分组：经真实的 RPC 会话，宿主分组由 `support::FakeHost` 模拟

#[allow(dead_code)] // 共用的假宿主，这里只用到一部分
mod support;

use std::collections::BTreeSet;

use serde_json::{Value, json};
use support::{FakeHost, Peer, ticket_reply};

fn enabled() -> Value {
    json!({"probeEnabled": true, "fingerprintSyncHours": 0})
}

/// 直接写一份测票当前判定
fn health(host: &mut FakeHost, account: &str, level: &str) {
    host.state.insert(
        ("history".into(), format!("{account}/turnstate")),
        (
            json!({"runs":[],"health":{"level":level,"model":"gpt-6-luna","last_run_id":"seed","last_started_ms":1}}),
            1,
        ),
    );
}

fn members(host: &FakeHost, key: &str) -> BTreeSet<String> {
    host.groups[key].1.clone()
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

async fn save(peer: &mut Peer, host: &mut FakeHost, body: Value) -> (u64, Value) {
    peer.api("POST", "api/groups", "", Some(body), host).await
}

fn two_rules() -> Value {
    json!({"enabled": true, "rules": [
        {"name": "测票正常", "color": "#22c55e", "levels": ["ok"]},
        {"name": "测票异常", "color": "#EF4444FF", "levels": ["suspect", "degraded"]},
    ]})
}

fn rule_id(view: &Value, name: &str) -> String {
    view["rules"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn groups_follow_the_confirmed_turnstate_verdict() {
    let mut host = FakeHost::new(&["acct_ok", "acct_bad", "acct_new", "acct_key"]);
    host.accounts[3]["authentication_kind"] = json!("apikey");
    health(&mut host, "acct_ok", "ok");
    health(&mut host, "acct_bad", "degraded");
    let mut peer = Peer::start(enabled()).await;

    // 默认关闭：维护不建自动分组
    peer.reconcile(&mut host).await;
    assert!(host.groups.keys().all(|k| k == "probe"));

    let (status, view) = save(&mut peer, &mut host, two_rules()).await;
    assert_eq!(status, 200, "{view}");
    let ok = rule_id(&view, "测票正常");
    let bad = rule_id(&view, "测票异常");
    assert!(ok.starts_with("auto-") && ok.len() == 13, "{ok}");
    assert_eq!(view["rules"][0]["color"], "#22C55EFF");
    assert_eq!(host.groups[&ok].0, "测票正常");
    assert_eq!(members(&host, &ok), set(&["acct_ok"]));
    assert_eq!(members(&host, &bad), set(&["acct_bad"]));
    assert_eq!(view["status"]["rules"][&ok]["members"], 1);
    assert_eq!(view["status"]["rules"][&ok]["group_name"], "测票正常");

    // 3 次正常确认后，下一轮维护把它移到正常组
    for _ in 0..3 {
        host.http
            .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
        host.http
            .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
        let (status, body) = peer
            .api(
                "POST",
                "api/probe",
                "",
                Some(json!({"account_id":"acct_bad","method":"turnstate","model":"gpt-6-luna"})),
                &mut host,
            )
            .await;
        assert_eq!(status, 200, "{body}");
    }
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &ok), set(&["acct_ok", "acct_bad"]));
    assert!(members(&host, &bad).is_empty());
    let grouping_only = host.calls.len();

    // 管理员在 CPR 里手动加进来的，全量同步时纠正
    host.groups
        .get_mut(&bad)
        .unwrap()
        .1
        .insert("acct_new".into());
    save(&mut peer, &mut host, json!({"enabled": true, "rules": [
        {"id": ok, "name": "测票正常", "color": "#22C55EFF", "levels": ["ok"]},
        {"id": bad, "name": "测票异常", "color": "#EF4444FF", "levels": ["suspect", "degraded", "unknown"]},
    ]}))
    .await;
    assert_eq!(
        members(&host, &bad),
        set(&["acct_new"]),
        "unknown verdicts join when selected; apikey accounts never do"
    );

    // 删掉一条规则：清空它的成员后不再管它
    let (status, view) = save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"id": ok, "name": "测票正常", "color": "#22C55EFF", "levels": ["ok"]},
        ]}),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert!(members(&host, &bad).is_empty());
    assert_eq!(view["retired"], 0);
    assert!(view["status"]["rules"].get(&bad).is_none());

    // 关闭后成员保持原样
    save(
        &mut peer,
        &mut host,
        json!({"enabled": false, "rules": [
            {"id": ok, "name": "测票正常", "color": "#22C55EFF", "levels": ["ok"]},
        ]}),
    )
    .await;
    health(&mut host, "acct_ok", "degraded");
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &ok), set(&["acct_ok", "acct_bad"]));
    assert!(
        !host.calls[grouping_only..]
            .iter()
            .any(|c| c == "host.auth.get" || c == "host.model.execute"),
        "grouping never reads credentials"
    );
}

#[tokio::test]
async fn invalid_group_settings_change_nothing() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut peer = Peer::start(enabled()).await;
    let (_, view) = save(&mut peer, &mut host, two_rules()).await;
    let ok = rule_id(&view, "测票正常");
    let saved = host.get("settings", "groups").unwrap().clone();
    let rule = |name: &str| json!({"name": name, "color": "#22C55E", "levels": ["ok"]});
    for body in [
        json!({"enabled": true, "rules": [rule("a"), rule("A")]}),
        json!({"enabled": true, "rules": [{"name": " ", "color": "#22C55E", "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [{"name": "x", "color": "green", "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [{"name": "x", "color": "#22C55E", "levels": []}]}),
        json!({"enabled": true, "rules": [{"name": "x", "color": "#22C55E", "levels": ["great"]}]}),
        json!({"enabled": true, "rules": (0..9).map(|i| rule(&format!("g{i}"))).collect::<Vec<_>>()}),
        json!({"enabled": true, "rules": [{"id": ok, "name": "改名", "color": "#22C55EFF", "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [{"id": "auto-00000000", "name": "x", "color": "#22C55E", "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [], "extra": 1}),
    ] {
        let (status, body) = save(&mut peer, &mut host, body.clone()).await;
        assert!(status == 400, "{status} {body}");
    }
    assert_eq!(host.get("settings", "groups").unwrap(), &saved);
}

#[tokio::test]
async fn a_name_taken_in_cpr_is_reported_per_rule() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.taken_group_names.push("测票正常".into());
    health(&mut host, "acct_a", "suspect");
    let mut peer = Peer::start(enabled()).await;
    let (status, view) = save(&mut peer, &mut host, two_rules()).await;
    assert_eq!(status, 200, "{view}");
    let ok = rule_id(&view, "测票正常");
    let bad = rule_id(&view, "测票异常");
    assert!(
        view["status"]["rules"][&ok]["error"]
            .as_str()
            .unwrap()
            .contains("同名分组")
    );
    assert!(!host.groups.contains_key(&ok));
    assert_eq!(members(&host, &bad), set(&["acct_a"]));

    // 删掉冲突的规则：从没建过，也能顺利丢掉
    let (_, view) = save(&mut peer, &mut host, json!({"enabled": true, "rules": [
        {"id": bad, "name": "测票异常", "color": "#EF4444FF", "levels": ["suspect", "degraded"]},
    ]}))
    .await;
    assert_eq!(view["retired"], 0, "{view}");
}
