//! 自动分组：经真实的 RPC 会话，宿主分组和 CPR 管理接口由 `support::FakeHost` 模拟
//!
//! 只在测票当前判定变化时调整，只动判定变了的账号，手动改动保留（用户 2026-11-02）。

#[allow(dead_code)] // 共用的假宿主，这里只用到一部分
mod support;

use std::collections::BTreeSet;

use serde_json::{Value, json};
use support::{FakeHost, Peer, ticket_reply};

fn enabled() -> Value {
    json!({"probeEnabled": true, "fingerprintSyncHours": 0})
}

/// 直接写一份测票当前判定（保留历史里的其他字段）
fn health(host: &mut FakeHost, account: &str, level: &str) {
    let key = ("history".to_owned(), format!("{account}/turnstate"));
    let entry = host.state.entry(key).or_insert((json!({"runs": []}), 1));
    entry.0["health"] =
        json!({"level":level,"model":"gpt-6-luna","last_run_id":"seed","last_started_ms":1});
}

fn grouped_level(host: &FakeHost, account: &str) -> Value {
    host.get("history", &format!("{account}/turnstate"))
        .map_or(Value::Null, |h| h["grouped_level"].clone())
}

fn members(host: &FakeHost, key: &str) -> BTreeSet<String> {
    host.groups
        .get(key)
        .map(|g| g.1.clone())
        .unwrap_or_default()
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

async fn save(peer: &mut Peer, host: &mut FakeHost, body: Value) -> (u64, Value) {
    peer.api("POST", "api/groups", "", Some(body), host).await
}

/// 连续 3 次测票把当前判定确认成 `level`（ok 或 degraded）
async fn confirm(peer: &mut Peer, host: &mut FakeHost, account: &str, level: &str) {
    for _ in 0..3 {
        let tickets: &[&str] = match level {
            "ok" => &["gAAAAAticket-a", "gAAAAAticket-a"],
            "degraded" => &[
                "gAAAAAticket-a",
                "gAAAAAticket-b",
                "gAAAAAticket-c",
                "gAAAAAticket-d",
            ],
            _ => panic!("unknown fixture"),
        };
        for t in tickets {
            host.http.push_back(ticket_reply(Some(t), None));
        }
        let (status, body) = peer
            .api(
                "POST",
                "api/probe",
                "",
                Some(json!({"account_id":account,"method":"turnstate","model":"gpt-6-luna"})),
                host,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["run"]["verdict"]["level"], level, "{body}");
    }
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
async fn owned_groups_change_only_when_the_verdict_changes() {
    let mut host = FakeHost::new(&["acct_ok", "acct_bad", "acct_new", "acct_key"]);
    host.accounts[3]["authentication_kind"] = json!("apikey");
    health(&mut host, "acct_ok", "ok");
    health(&mut host, "acct_bad", "degraded");
    let mut peer = Peer::start(enabled()).await;

    // 默认关闭：维护不建自动分组
    peer.reconcile(&mut host).await;
    assert!(host.groups.keys().all(|k| k == "probe"));

    // 开启：建好分组、记下起点，但不加任何成员
    let (status, view) = save(&mut peer, &mut host, two_rules()).await;
    assert_eq!(status, 200, "{view}");
    let ok = rule_id(&view, "测票正常");
    let bad = rule_id(&view, "测票异常");
    assert!(ok.starts_with("auto-") && ok.len() == 13, "{ok}");
    assert_eq!(view["rules"][0]["color"], "#22C55EFF");
    assert_eq!(view["status"]["rules"][&ok]["group_name"], "测票正常");
    assert!(members(&host, &ok).is_empty() && members(&host, &bad).is_empty());
    assert_eq!(grouped_level(&host, "acct_ok"), "ok");
    assert_eq!(grouped_level(&host, "acct_bad"), "degraded");
    assert_eq!(grouped_level(&host, "acct_new"), "unknown");
    assert_eq!(
        grouped_level(&host, "acct_key"),
        Value::Null,
        "unsupported accounts are ignored"
    );

    // 判定没变：手动加的成员保留，也不补加
    host.groups
        .get_mut(&ok)
        .unwrap()
        .1
        .insert("acct_new".into());
    host.groups
        .get_mut(&bad)
        .unwrap()
        .1
        .insert("acct_key".into());
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &ok), set(&["acct_new"]));
    assert_eq!(members(&host, &bad), set(&["acct_key"]));

    // 降智 → 正常：只动 acct_bad
    confirm(&mut peer, &mut host, "acct_bad", "ok").await;
    let grouping_only = host.calls.len();
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &ok), set(&["acct_new", "acct_bad"]));
    assert_eq!(members(&host, &bad), set(&["acct_key"]));
    assert_eq!(grouped_level(&host, "acct_bad"), "ok");
    let (_, view) = peer.api("GET", "api/groups", "", None, &mut host).await;
    let change = &view["status"]["changes"][0];
    assert_eq!(change["account_id"], "acct_bad");
    assert_eq!(
        (change["from"].as_str(), change["to"].as_str()),
        (Some("degraded"), Some("ok"))
    );
    assert_eq!(change["added"], json!(["测票正常"]));
    assert!(
        !host.calls[grouping_only..]
            .iter()
            .any(|c| c == "host.auth.get" || c == "host.model.execute"),
        "grouping never reads credentials"
    );

    // 再跑一轮、保存设置：都不改成员
    peer.reconcile(&mut host).await;
    save(&mut peer, &mut host, json!({"enabled": true, "rules": [
        {"id": ok, "name": "测票正常", "color": "#22C55EFF", "levels": ["ok", "unknown"]},
        {"id": bad, "name": "测票异常", "color": "#EF4444FF", "levels": ["suspect", "degraded"]},
    ]}))
    .await;
    assert_eq!(members(&host, &ok), set(&["acct_new", "acct_bad"]));

    // 正常 → 降智：从正常组移出、加进异常组，acct_new 和 acct_key 不动
    confirm(&mut peer, &mut host, "acct_bad", "degraded").await;
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &ok), set(&["acct_new"]));
    assert_eq!(members(&host, &bad), set(&["acct_key", "acct_bad"]));

    // 删除规则也不清空成员
    let (_, view) = save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"id": ok, "name": "测票正常", "color": "#22C55EFF", "levels": ["ok"]},
        ]}),
    )
    .await;
    assert_eq!(members(&host, &bad), set(&["acct_key", "acct_bad"]));
    assert!(view["status"]["rules"].get(&bad).is_none());

    // 关闭期间的变化不补：重新开启只记新起点
    save(
        &mut peer,
        &mut host,
        json!({"enabled": false, "rules": [
            {"id": ok, "name": "测票正常", "color": "#22C55EFF", "levels": ["ok"]},
        ]}),
    )
    .await;
    confirm(&mut peer, &mut host, "acct_bad", "ok").await;
    peer.reconcile(&mut host).await;
    save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"id": ok, "name": "测票正常", "color": "#22C55EFF", "levels": ["ok"]},
        ]}),
    )
    .await;
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &ok), set(&["acct_new"]));
    assert_eq!(grouped_level(&host, "acct_bad"), "ok");
}

#[tokio::test]
async fn a_change_confirmed_before_a_restart_is_applied_once() {
    let mut host = FakeHost::new(&["acct_a", "acct_b"]);
    health(&mut host, "acct_a", "ok");
    health(&mut host, "acct_b", "ok");
    let mut peer = Peer::start(enabled()).await;
    let (_, view) = save(&mut peer, &mut host, two_rules()).await;
    let ok = rule_id(&view, "测票正常");
    let bad = rule_id(&view, "测票异常");
    host.groups.get_mut(&ok).unwrap().1.insert("acct_b".into());
    drop(peer);

    // 插件不在时 acct_a 被确认降智（来不及调整分组）
    health(&mut host, "acct_a", "degraded");
    let mut peer = Peer::start(enabled()).await;
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &bad), set(&["acct_a"]));
    assert_eq!(
        members(&host, &ok),
        set(&["acct_b"]),
        "acct_b did not change"
    );
    assert_eq!(grouped_level(&host, "acct_a"), "degraded");
    // 调整过一次就不再动：手动移出后不会被加回去
    host.groups.get_mut(&bad).unwrap().1.remove("acct_a");
    peer.reconcile(&mut host).await;
    assert!(members(&host, &bad).is_empty(), "applied once");
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
        json!({"enabled": true, "rules": [{"name": "x", "color": "#22C55E", "levels": ["ok"], "plans": ["bad plan"]}]}),
        json!({"enabled": true, "rules": [{"name": "x", "color": "#22C55E", "levels": ["ok"], "plans": ["x".repeat(33)]}]}),
        json!({"enabled": true, "rules": [{"name": "x", "color": "#22C55E", "levels": ["ok"], "plans": (0..17).map(|i| format!("p{i}")).collect::<Vec<_>>()}]}),
    ] {
        let (status, body) = save(&mut peer, &mut host, body.clone()).await;
        assert_eq!(status, 400, "{body}");
    }
    assert_eq!(host.get("settings", "groups").unwrap(), &saved);
}

#[tokio::test]
async fn a_name_taken_in_cpr_is_reported_per_rule() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.taken_group_names.push("测票正常".into());
    health(&mut host, "acct_a", "ok");
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

    // 判定变化时，重名的规则跳过，其他规则照常，也不会一直重试
    confirm(&mut peer, &mut host, "acct_a", "degraded").await;
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &bad), set(&["acct_a"]));
    assert_eq!(grouped_level(&host, "acct_a"), "degraded");
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
        .insert("acct_bad".into(), set(&[BAD, OTHER]));
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
async fn existing_cpr_groups_change_only_for_accounts_whose_verdict_changed() {
    let mut host = cpr_host();
    let mut peer = Peer::start(admin_config()).await;

    let (status, body) = peer.api("GET", "api/groups/cpr", "", None, &mut host).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["groups"].as_array().unwrap().len(), 3);

    // 保存：只读分组名，不写任何账号
    let (status, view) = save(&mut peer, &mut host, existing_rules()).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["rules"][0]["name"], "主力", "names come from CPR");
    let main = rule_id(&view, "主力");
    assert_eq!(view["status"]["rules"][&main]["group_name"], "主力");
    peer.reconcile(&mut host).await;
    assert!(host.cpr_writes.is_empty(), "{:?}", host.cpr_writes);
    assert_eq!(
        cpr(&host, "acct_ok"),
        set(&[OTHER]),
        "not added without a change"
    );
    assert_eq!(
        cpr(&host, "acct_key"),
        set(&[MAIN]),
        "manual membership kept"
    );

    // acct_bad 降智 → 正常：换掉这两个分组，保留它的其他分组；只写它自己
    confirm(&mut peer, &mut host, "acct_bad", "ok").await;
    peer.reconcile(&mut host).await;
    assert_eq!(cpr(&host, "acct_bad"), set(&[MAIN, OTHER]));
    assert_eq!(
        host.cpr_writes,
        vec![json!({"accountIds":["acct_bad"],"groupIds":[MAIN, OTHER]})]
    );
    assert_eq!(cpr(&host, "acct_ok"), set(&[OTHER]));
    assert_eq!(cpr(&host, "acct_key"), set(&[MAIN]));
    let (_, view) = peer.api("GET", "api/groups", "", None, &mut host).await;
    assert_eq!(view["status"]["changes"][0]["added"], json!(["主力"]));
    assert_eq!(view["status"]["changes"][0]["removed"], json!(["隔离"]));

    // Key 不进私有状态和响应；只调用这三个管理路由
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
async fn a_failed_admin_write_is_retried_next_round() {
    let mut host = cpr_host();
    let mut peer = Peer::start(admin_config()).await;
    save(&mut peer, &mut host, existing_rules()).await;
    confirm(&mut peer, &mut host, "acct_bad", "ok").await;
    let key = std::mem::replace(&mut host.cpr_admin_key, "rotated".into());
    peer.reconcile(&mut host).await;
    assert_eq!(cpr(&host, "acct_bad"), set(&[BAD, OTHER]));
    assert_eq!(
        grouped_level(&host, "acct_bad"),
        "degraded",
        "not marked as applied"
    );
    let (_, view) = peer.api("GET", "api/groups", "", None, &mut host).await;
    assert!(
        view["status"]["rules"].to_string().contains("无效"),
        "{view}"
    );
    host.cpr_admin_key = key;
    peer.reconcile(&mut host).await;
    assert_eq!(cpr(&host, "acct_bad"), set(&[MAIN, OTHER]));
    assert_eq!(grouped_level(&host, "acct_bad"), "ok");
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
    let bad = rule_id(&view, "隔离");
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

    // 现有分组在 CPR 里被删掉：仍能保存，那条规则报错；判定变化时其他规则照常
    host.cpr_groups.retain(|g| g["id"] != BAD);
    host.cpr_account_groups
        .insert("acct_bad".into(), set(&[OTHER]));
    let (status, view) = save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"id": main, "group_id": MAIN, "levels": ["ok"]},
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
    confirm(&mut peer, &mut host, "acct_bad", "ok").await;
    peer.reconcile(&mut host).await;
    assert_eq!(cpr(&host, "acct_bad"), set(&[MAIN, OTHER]));
    assert!(host.cpr_account_groups.values().all(|g| !g.contains(BAD)));
}

#[tokio::test]
async fn rules_can_filter_by_account_type() {
    let mut host = cpr_host();
    host.accounts[0]["plan_type"] = json!("Team");
    host.accounts[1]["plan_type"] = json!("pro");
    host.accounts[2]["plan_type"] = json!(null);
    for a in ["acct_ok", "acct_bad", "acct_new"] {
        health(&mut host, a, "suspect");
    }
    let mut peer = Peer::start(admin_config()).await;
    let (_, view) = peer.api("GET", "api/groups", "", None, &mut host).await;
    assert_eq!(
        view["plans"],
        json!(["pro", "team", "unknown"]),
        "only accounts that can be probed"
    );

    let (status, view) = save(
        &mut peer,
        &mut host,
        json!({"enabled": true, "rules": [
            {"name": "Team 正常", "color": "#22C55E", "levels": ["ok"], "plans": ["team"]},
            {"group_id": MAIN, "levels": ["ok"], "plans": ["PRO", "unknown", "pro"]},
            {"name": "全部正常", "color": "#3B82F6", "levels": ["ok"]},
        ]}),
    )
    .await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        view["rules"][1]["plans"],
        json!(["pro", "unknown"]),
        "normalized"
    );
    assert!(
        view["rules"][2].get("plans").is_none(),
        "no filter means every type"
    );
    let team = rule_id(&view, "Team 正常");
    let all = rule_id(&view, "全部正常");

    // 三个账号都变成正常（插件不在时确认的），重启后调整
    for a in ["acct_ok", "acct_bad", "acct_new"] {
        health(&mut host, a, "ok");
    }
    drop(peer);
    let mut peer = Peer::start(admin_config()).await;
    peer.reconcile(&mut host).await;
    assert_eq!(members(&host, &team), set(&["acct_ok"]));
    assert_eq!(
        members(&host, &all),
        set(&["acct_ok", "acct_bad", "acct_new"])
    );
    assert_eq!(
        cpr(&host, "acct_ok"),
        set(&[OTHER]),
        "team is not pro/unknown"
    );
    assert_eq!(
        cpr(&host, "acct_bad"),
        set(&[BAD, MAIN, OTHER]),
        "BAD has no rule here"
    );
    assert_eq!(cpr(&host, "acct_new"), set(&[MAIN]));
}
