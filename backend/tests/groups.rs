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

const MAIN: &str = "grp_00000000000000000000000000000001";
const BAD: &str = "grp_00000000000000000000000000000002";
const OTHER: &str = "grp_00000000000000000000000000000003";

fn admin_config() -> Value {
    json!({"probeEnabled": true, "fingerprintSyncHours": 0,
        "cprAdminUrl": "http://cpr.test/", "cprAdminApiKey": "admin-secret"})
}

fn cpr_host() -> FakeHost {
    let mut host = FakeHost::new(&["acct_ok", "acct_bad", "acct_new", "acct_key"]);
    host.accounts[3]["authentication_kind"] = json!("apikey");
    host.cpr_admin_key = "admin-secret".into();
    host.cpr_groups = vec![
        json!({"id": MAIN, "name": "主力", "color": "#22C55EFF", "enabled": true}),
        json!({"id": BAD, "name": "隔离", "color": "#EF4444FF", "enabled": true}),
        json!({"id": OTHER, "name": "手动", "color": "#6B7280FF", "enabled": true}),
    ];
    host.cpr_account_groups
        .insert("acct_ok".into(), set(&[OTHER]));
    host.cpr_account_groups
        .insert("acct_bad".into(), set(&[MAIN, OTHER]));
    host.cpr_account_groups
        .insert("acct_key".into(), set(&[MAIN]));
    health(&mut host, "acct_ok", "ok");
    health(&mut host, "acct_bad", "degraded");
    host
}

fn cpr(host: &FakeHost, account: &str) -> BTreeSet<String> {
    host.cpr_account_groups
        .get(account)
        .cloned()
        .unwrap_or_default()
}

fn existing_rules() -> Value {
    json!({"enabled": true, "rules": [
        {"group_id": MAIN, "levels": ["ok"]},
        {"group_id": BAD, "levels": ["suspect", "degraded"]},
    ]})
}

#[tokio::test]
async fn existing_cpr_groups_are_adjusted_through_the_admin_api() {
    let mut host = cpr_host();
    let mut peer = Peer::start(admin_config()).await;

    let (status, body) = peer.api("GET", "api/groups/cpr", "", None, &mut host).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["groups"].as_array().unwrap().len(), 3);

    let (status, view) = save(&mut peer, &mut host, existing_rules()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["rules"][0]["name"], "主力", "names come from CPR");
    assert!(view.get("admin_error").is_none_or(Value::is_null), "{view}");
    // 只换掉规则管的分组，其他分组保留；不支持探测的账号移出
    assert_eq!(cpr(&host, "acct_ok"), set(&[MAIN, OTHER]));
    assert_eq!(cpr(&host, "acct_bad"), set(&[BAD, OTHER]));
    assert_eq!(cpr(&host, "acct_key"), set(&[]));
    assert_eq!(cpr(&host, "acct_new"), set(&[]));
    let main = rule_id(&view, "主力");
    assert_eq!(view["status"]["rules"][&main]["group_name"], "主力");
    assert_eq!(view["status"]["rules"][&main]["members"], 1);
    assert_eq!(host.cpr_writes.len(), 3, "acct_new needed no write");

    // 刚测完、判定变了的账号下一轮只写它自己
    for _ in 0..3 {
        host.http
            .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
        host.http
            .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
        peer.api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_bad","method":"turnstate","model":"gpt-6-luna"})),
            &mut host,
        )
        .await;
    }
    let writes = host.cpr_writes.len();
    peer.reconcile(&mut host).await;
    assert_eq!(cpr(&host, "acct_bad"), set(&[MAIN, OTHER]));
    assert_eq!(
        &host.cpr_writes[writes..],
        &[json!({"accountIds":["acct_bad"],"groupIds":[MAIN, OTHER]})]
    );

    // 删掉现有分组的规则：不再调整，成员保持
    let (status, view) = save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"id": main, "group_id": MAIN, "levels": ["ok"]},
        ]}),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["retired"], 0);
    health(&mut host, "acct_bad", "degraded");
    save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"id": main, "group_id": MAIN, "levels": ["ok"]},
        ]}),
    )
    .await;
    assert_eq!(
        cpr(&host, "acct_bad"),
        set(&[OTHER]),
        "left MAIN, BAD untouched"
    );

    // Key 不进私有状态和响应
    let stored = serde_json::to_string(&host.state.values().collect::<Vec<_>>()).unwrap();
    assert!(!stored.contains("admin-secret") && !view.to_string().contains("admin-secret"));
    assert!(
        host.cpr_admin_calls
            .iter()
            .all(|c| c == "GET /api/admin/account-groups/options"
                || c == "GET /api/admin/accounts"
                || c == "POST /api/admin/accounts/batch-update"),
        "{:?}",
        host.cpr_admin_calls
    );
}

#[tokio::test]
async fn existing_group_rules_need_a_working_admin_api() {
    // 没配地址和 Key
    let mut host = cpr_host();
    let mut peer = Peer::start(enabled()).await;
    let (status, body) = save(&mut peer, &mut host, existing_rules()).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("管理 API Key")
    );
    assert_eq!(
        peer.api("GET", "api/groups/cpr", "", None, &mut host)
            .await
            .0,
        409
    );
    assert!(host.cpr_admin_calls.is_empty());

    // Key 不对：错误里不带 Key
    let mut host = cpr_host();
    host.cpr_admin_key = "another".into();
    let mut peer = Peer::start(admin_config()).await;
    let (status, body) = peer.api("GET", "api/groups/cpr", "", None, &mut host).await;
    assert_eq!(status, 502, "{body}");
    assert!(body.to_string().contains("无效") && !body.to_string().contains("admin-secret"));
}

#[tokio::test]
async fn existing_group_rules_are_validated_and_a_deleted_group_is_reported() {
    let mut host = cpr_host();
    let mut peer = Peer::start(admin_config()).await;
    // 插件自己新建的分组不能再选作现有分组
    let (_, view) = save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"name": "测票正常", "color": "#22C55E", "levels": ["ok"]},
        ]}),
    )
    .await;
    let owned = rule_id(&view, "测票正常");
    let owned_group = support::plugin_group_id(&owned);
    let (_, view) = save(&mut peer, &mut host, existing_rules()).await;
    let main = rule_id(&view, "主力");
    for body in [
        json!({"enabled": true, "rules": [{"group_id": owned_group, "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [{"group_id": MAIN, "levels": ["ok"]}, {"group_id": MAIN, "levels": ["suspect"]}]}),
        json!({"enabled": true, "rules": [{"group_id": "grp_ffffffffffffffffffffffffffffffff", "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [{"group_id": "not-a-group", "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [{"id": main, "group_id": BAD, "levels": ["ok"]}]}),
        json!({"enabled": true, "rules": [{"id": main, "name": "主力", "color": "#22C55E", "levels": ["ok"]}]}),
    ] {
        let (status, body) = save(&mut peer, &mut host, body.clone()).await;
        assert_eq!(status, 400, "{body}");
    }

    // 现有分组在 CPR 里被删掉：仍能保存，那条规则报错，其他规则照常
    let bad = rule_id(&view, "隔离");
    host.cpr_groups.retain(|g| g["id"] != BAD);
    host.cpr_account_groups
        .insert("acct_bad".into(), set(&[OTHER]));
    let (status, view) = save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"id": main, "group_id": MAIN, "levels": ["ok", "degraded"]},
            {"id": bad, "group_id": BAD, "levels": ["suspect", "degraded"]},
        ]}),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert!(
        view["status"]["rules"][&bad]["error"]
            .as_str()
            .unwrap()
            .contains("找不到")
    );
    assert!(view["status"]["rules"][&main].get("error").is_none());
    assert_eq!(cpr(&host, "acct_bad"), set(&[MAIN, OTHER]));
    assert!(host.cpr_account_groups.values().all(|g| !g.contains(BAD)));
}
