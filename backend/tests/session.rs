//! 经真实的 RPC 会话跑完整流程，宿主回调由 `support::FakeHost` 模拟

mod support;

use serde_json::{Value, json};
use support::{FakeHost, HttpReply, Peer, ticket_reply};

const ENABLED: fn() -> Value = || json!({"probeEnabled": true, "fingerprintSyncHours": 0});
/// ModelTrace 需要用户配置的 Client Key ID
const WITH_KEY: fn() -> Value = || json!({"probeEnabled": true, "fingerprintSyncHours": 0, "modeltraceClientKeyId": "key_mine"});

fn telegram_config() -> Value {
    json!({"probeEnabled":true,"fingerprintSyncHours":0,"telegramEnabled":true,
        "telegramBotToken":"12345:unit_test_token","telegramChatId":"-10012345","telegramThreadId":42})
}

fn telegram_reply(status: u16, body: Value) -> HttpReply {
    HttpReply {
        status,
        headers: vec![],
        body: serde_json::to_vec(&body).unwrap(),
    }
}

fn legacy_health(host: &mut FakeHost, level: &str) {
    host.state.insert(
        ("history".into(), "acct_a/turnstate".into()),
        (
            json!({"runs":[{
                "id":"legacy","model":"gpt-6-astra","trigger":"manual",
                "started_ms":cpr_account_probe::clock::now_ms()-10000,
                "finished_ms":cpr_account_probe::clock::now_ms()-9000,
                "status":"done","level":level,"reason":"legacy","detail":false
            }]}),
            1,
        ),
    );
}

async fn ticket_probe(peer: &mut Peer, host: &mut FakeHost, level: &str) -> Value {
    match level {
        "ok" => {
            host.http
                .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
            host.http
                .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
        }
        "suspect" => {
            host.http
                .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
            host.http
                .push_back(ticket_reply(Some("gAAAAAticket-b"), None));
            host.http
                .push_back(ticket_reply(Some("gAAAAAticket-b"), None));
        }
        "degraded" => {
            for ticket in [
                "gAAAAAticket-a",
                "gAAAAAticket-b",
                "gAAAAAticket-c",
                "gAAAAAticket-d",
            ] {
                host.http.push_back(ticket_reply(Some(ticket), None));
            }
        }
        _ => panic!("unknown fixture"),
    }
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            host,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["run"]["verdict"]["level"], level, "{body}");
    body
}

#[tokio::test]
async fn telegram_recovery_regression_and_restart_preserve_raw_history() {
    let mut host = FakeHost::new(&["acct_a"]);
    legacy_health(&mut host, "degraded");
    let mut peer = Peer::start(telegram_config()).await;
    for _ in 0..2 {
        ticket_probe(&mut peer, &mut host, "ok").await;
    }
    assert!(host.seen_public_http.is_empty());
    assert_eq!(
        host.get("history", "acct_a/turnstate").unwrap()["health"]["level"],
        "degraded"
    );
    // Restart the process after two successes; confirmation must still happen on the third.
    drop(peer);
    let mut peer = Peer::start(telegram_config()).await;
    host.public_http
        .push_back(telegram_reply(200, json!({"ok":true,"result":{}})));
    ticket_probe(&mut peer, &mut host, "ok").await;
    assert_eq!(host.seen_public_http.len(), 1);
    let body = &host.seen_public_bodies[0];
    assert_eq!(body["chat_id"], "-10012345");
    assert_eq!(body["message_thread_id"], 42);
    let text = body["text"].as_str().unwrap();
    assert!(text.contains("降智 → 正常"));
    assert!(
        text.contains("账号：name-acct_a\n邮箱：acct_a@example.com\n"),
        "{text}"
    );
    assert!(
        text.contains("（北京时间）") && !text.contains("Unix"),
        "{text}"
    );
    assert!(body.get("parse_mode").is_none());
    assert_eq!(
        host.get("history", "acct_a/turnstate").unwrap()["health"]["level"],
        "ok"
    );
    for _ in 0..3 {
        ticket_probe(&mut peer, &mut host, "ok").await;
    }
    assert_eq!(
        host.seen_public_http.len(),
        1,
        "unchanged health must not notify"
    );
    ticket_probe(&mut peer, &mut host, "suspect").await;
    ticket_probe(&mut peer, &mut host, "degraded").await;
    assert_eq!(
        host.get("history", "acct_a/turnstate").unwrap()["health"]["level"],
        "ok"
    );
    host.public_http
        .push_back(telegram_reply(200, json!({"ok":true})));
    ticket_probe(&mut peer, &mut host, "suspect").await;
    assert_eq!(host.seen_public_http.len(), 2);
    assert!(
        host.seen_public_bodies[1]["text"]
            .as_str()
            .unwrap()
            .contains("正常 → 降智")
    );
    let (_, overview) = peer
        .api("GET", "api/overview", "hours=24", None, &mut host)
        .await;
    assert_eq!(overview["accounts"][0]["health"]["level"], "degraded");
    let (_, detail) = peer
        .api("GET", "api/account", "id=acct_a", None, &mut host)
        .await;
    assert_eq!(
        detail["account"]["methods"]["turnstate"]["runs"][0]["level"],
        "suspect"
    );
    let stored = serde_json::to_string(&host.state.values().collect::<Vec<_>>()).unwrap();
    for text in [stored, overview.to_string(), detail.to_string()] {
        assert!(!text.contains("unit_test_token"));
        assert!(!text.contains("secret-access-token"));
    }
}

#[tokio::test]
async fn telegram_only_alerts_when_crossing_normal_and_abnormal() {
    let mut host = FakeHost::new(&["acct_a"]);
    legacy_health(&mut host, "ok");
    let mut peer = Peer::start(telegram_config()).await;
    host.public_http
        .push_back(telegram_reply(200, json!({"ok":true})));
    for _ in 0..3 {
        ticket_probe(&mut peer, &mut host, "degraded").await;
    }
    assert_eq!(host.seen_public_http.len(), 1);
    assert!(
        host.seen_public_bodies[0]["text"]
            .as_str()
            .unwrap()
            .contains("正常 → 降智")
    );
    for _ in 0..3 {
        ticket_probe(&mut peer, &mut host, "suspect").await;
    }
    let history = host.get("history", "acct_a/turnstate").unwrap();
    assert_eq!(history["health"]["level"], "suspect");
    assert!(history.get("notifications").is_none());
    assert_eq!(
        host.seen_public_http.len(),
        1,
        "降智 → 疑似 is still abnormal and must not notify"
    );
    host.public_http
        .push_back(telegram_reply(200, json!({"ok":true})));
    for _ in 0..3 {
        ticket_probe(&mut peer, &mut host, "ok").await;
    }
    assert_eq!(host.seen_public_http.len(), 2);
    assert!(
        host.seen_public_bodies[1]["text"]
            .as_str()
            .unwrap()
            .contains("疑似 → 正常")
    );
}

#[tokio::test]
async fn telegram_failure_is_durable_sanitized_and_retried_by_maintenance() {
    let mut host = FakeHost::new(&["acct_a"]);
    legacy_health(&mut host, "suspect");
    let mut peer = Peer::start(telegram_config()).await;
    for _ in 0..2 {
        ticket_probe(&mut peer, &mut host, "ok").await;
    }
    host.public_http.push_back(telegram_reply(
        429,
        json!({"ok":false,"error_code":429,
        "description":"do not expose 12345:unit_test_token", "parameters":{"retry_after":180}}),
    ));
    let result = ticket_probe(&mut peer, &mut host, "ok").await;
    assert_eq!(result["run"]["status"], "done");
    let history = host.get("history", "acct_a/turnstate").unwrap();
    assert_eq!(history["health"]["level"], "ok");
    assert_eq!(history["notifications"][0]["attempts"], 1);
    assert!(
        history["notifications"][0]["retry_at_ms"].as_i64().unwrap()
            > cpr_account_probe::clock::now_ms() + 170000
    );
    assert!(!history.to_string().contains("unit_test_token"));
    peer.reconcile(&mut host).await;
    assert_eq!(
        host.seen_public_http.len(),
        1,
        "retry must respect retry_after"
    );
    host.state
        .get_mut(&("history".into(), "acct_a/turnstate".into()))
        .unwrap()
        .0["notifications"][0]["retry_at_ms"] = json!(0);
    drop(peer);
    let mut peer = Peer::start(telegram_config()).await;
    host.public_http
        .push_back(telegram_reply(200, json!({"ok":true})));
    peer.reconcile(&mut host).await;
    assert_eq!(host.seen_public_http.len(), 2);
    assert!(
        host.get("history", "acct_a/turnstate")
            .unwrap()
            .get("notifications")
            .is_none()
    );
    peer.reconcile(&mut host).await;
    assert_eq!(host.seen_public_http.len(), 2);
}

#[tokio::test]
async fn disabled_telegram_still_updates_health_without_backfill() {
    let mut host = FakeHost::new(&["acct_a"]);
    legacy_health(&mut host, "degraded");
    let mut peer = Peer::start(ENABLED()).await;
    for _ in 0..3 {
        ticket_probe(&mut peer, &mut host, "ok").await;
    }
    assert_eq!(
        host.get("history", "acct_a/turnstate").unwrap()["health"]["level"],
        "ok"
    );
    assert!(host.seen_public_http.is_empty());
    drop(peer);
    let mut peer = Peer::start(telegram_config()).await;
    peer.reconcile(&mut host).await;
    assert!(host.seen_public_http.is_empty());
}

#[tokio::test]
async fn telegram_test_validates_config_and_redacts_api_errors() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut config = telegram_config();
    config["telegramBotToken"] = json!("123:token/../../evil");
    let mut peer = Peer::start(config).await;
    let (status, _) = peer
        .api("POST", "api/telegram/test", "", Some(json!({})), &mut host)
        .await;
    assert_eq!(status, 502);
    assert!(host.seen_public_http.is_empty());
    drop(peer);
    let mut peer = Peer::start(telegram_config()).await;
    host.public_http.push_back(telegram_reply(
        200,
        json!({"ok":false,"error_code":401,"description":"12345:unit_test_token"}),
    ));
    let (status, error) = peer
        .api("POST", "api/telegram/test", "", Some(json!({})), &mut host)
        .await;
    assert_eq!(status, 502);
    assert!(!error.to_string().contains("unit_test_token"));
    host.public_http
        .push_back(telegram_reply(200, json!({"ok":true})));
    let (status, body) = peer
        .api("POST", "api/telegram/test", "", Some(json!({})), &mut host)
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["sent"], true);
}

fn numbers(n: usize, seed: usize) -> String {
    (0..n)
        .map(|i| ((i * 37 + seed * 11) % 355 + 1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[tokio::test]
async fn overview_lists_accounts_without_touching_credentials() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut peer = Peer::start(json!({})).await;
    let (status, body) = peer.api("GET", "api/overview", "", None, &mut host).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["config"]["probe_enabled"], false);
    assert_eq!(body["bank"]["source"], "embedded");
    let a = &body["accounts"][0];
    assert_eq!(a["id"], "acct_a");
    assert_eq!(a["supported"], true);
    // 24 小时 15 分钟一格；没有探测时一格都不列出
    assert_eq!(body["range"]["slots"], 96);
    assert_eq!(body["range"]["slot_ms"], 900_000);
    assert!(a["strip"].as_array().unwrap().is_empty());
    assert!(
        !host.calls.iter().any(|c| c == "host.auth.get"),
        "overview read credentials: {:?}",
        host.calls
    );
}

#[tokio::test]
async fn nothing_is_sent_while_probes_are_disabled() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut peer = Peer::start(json!({})).await;
    for method in ["turnstate", "modeltrace"] {
        let (status, body) = peer
            .api(
                "POST",
                "api/probe",
                "",
                Some(json!({"account_id":"acct_a","method":method,"model":"gpt-6-astra"})),
                &mut host,
            )
            .await;
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"]["blockers"][0]["code"], "probe_disabled");
    }
    // 自动探测也发不出去
    host.state.insert(
        ("settings".into(), "schedules".into()),
        (json!({"accounts":{"acct_a":{"turnstate":{"enabled":true,"model":"gpt-6-astra","interval_minutes":60}}}}), 1),
    );
    peer.reconcile(&mut host).await;
    assert!(host.seen_http.is_empty() && host.seen_models.is_empty());
    assert!(!host.calls.iter().any(|c| c == "host.auth.get"));
}

#[tokio::test]
async fn plan_sends_nothing() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut peer = Peer::start(ENABLED()).await;
    let (status, body) = peer
        .api(
            "POST",
            "api/plan",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["blockers"], json!([]));
    assert!(!body.to_string().contains("secret-access-token"));
    assert!(host.seen_http.is_empty() && !host.calls.iter().any(|c| c == "host.auth.get"));
}

#[tokio::test]
async fn turnstate_echoes_the_ticket_through_the_account_proxy() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.http.push_back(ticket_reply(
        Some("gAAAAAticket-1"),
        Some("__oailb=route-7; Path=/"),
    ));
    host.http
        .push_back(ticket_reply(Some("gAAAAAticket-1"), None));
    let mut peer = Peer::start(ENABLED()).await;
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let run = &body["run"];
    assert_eq!(run["status"], "done");
    assert_eq!(run["verdict"]["level"], "ok", "{run}");
    assert_eq!(host.seen_http.len(), 2);
    let (a, b) = (&host.seen_http[0], &host.seen_http[1]);
    assert_eq!((a.account_id.as_str(), a.revision), ("acct_a", 7));
    assert_eq!(
        a.header("Authorization").unwrap(),
        "Bearer secret-access-token"
    );
    assert_eq!(a.header("Chatgpt-Account-Id").unwrap(), "chatgpt-acct_a");
    assert_eq!(a.header("Session_id"), b.header("Session_id"));
    assert!(a.header("X-Codex-Turn-State").is_none());
    assert_eq!(b.header("X-Codex-Turn-State").unwrap(), "gAAAAAticket-1");
    assert_eq!(b.header("Cookie").unwrap(), "__oailb=route-7");
    assert_eq!(a.body["reasoning"]["effort"], "low");

    // 凭据和票都不进私有状态
    let stored =
        serde_json::to_string(&host.state.values().map(|(v, _)| v).collect::<Vec<_>>()).unwrap();
    assert!(
        !stored.contains("secret-access-token") && !stored.contains("gAAAAA"),
        "{stored}"
    );
    let history = host.get("history", "acct_a/turnstate").unwrap();
    assert_eq!(history["runs"].as_array().unwrap().len(), 1);
    assert_eq!(history["runs"][0]["level"], "ok");

    // 详情可以按账号读回，别的账号读不到
    let id = run["id"].as_str().unwrap();
    let (status, _) = peer
        .api(
            "GET",
            "api/run",
            &format!("id={id}&account=acct_a"),
            None,
            &mut host,
        )
        .await;
    assert_eq!(status, 200);
    let (status, _) = peer
        .api(
            "GET",
            "api/run",
            &format!("id={id}&account=acct_b"),
            None,
            &mut host,
        )
        .await;
    assert_eq!(status, 404);
}

fn overloaded_reply() -> HttpReply {
    HttpReply {
        status: 503,
        headers: vec![],
        body: br#"{"error":{"message":"Our servers are currently overloaded"}}"#.to_vec(),
    }
}

async fn turnstate_probe(peer: &mut Peer, host: &mut FakeHost) -> Value {
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            host,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    body
}

#[tokio::test]
async fn persistent_overload_is_degraded_after_three_retries() {
    let mut host = FakeHost::new(&["acct_a"]);
    for _ in 0..4 {
        host.http.push_back(overloaded_reply());
    }
    let mut peer = Peer::start(ENABLED()).await;
    let body = turnstate_probe(&mut peer, &mut host).await;
    assert_eq!(body["run"]["status"], "done", "{body}");
    assert_eq!(body["run"]["verdict"]["level"], "degraded", "{body}");
    assert_eq!(body["run"]["error"]["kind"], "overloaded");
    assert_eq!(host.seen_http.len(), 4);
}

#[tokio::test]
async fn overload_is_retried_and_a_later_answer_decides() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.http.push_back(overloaded_reply());
    host.http.push_back(overloaded_reply());
    host.http
        .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
    host.http
        .push_back(ticket_reply(Some("gAAAAAticket-a"), None));
    let mut peer = Peer::start(ENABLED()).await;
    let body = turnstate_probe(&mut peer, &mut host).await;
    assert_eq!(body["run"]["verdict"]["level"], "ok", "{body}");
    assert!(body["run"]["error"].is_null(), "{body}");
    assert_eq!(host.seen_http.len(), 4);
}

#[tokio::test]
async fn an_upstream_error_stops_the_check_and_is_not_a_verdict() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.http.push_back(HttpReply {
        status: 429,
        headers: vec![],
        body: br#"{"error":{"code":"rate_limit_exceeded","message":"slow down, token secret-access-token"}}"#.to_vec(),
    });
    let mut peer = Peer::start(ENABLED()).await;
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(body["run"]["status"], "failed");
    assert_eq!(body["run"]["error"]["kind"], "rate_limited");
    assert!(!body.to_string().contains("secret-access-token"), "{body}");
    assert_eq!(host.seen_http.len(), 1);
}

#[tokio::test]
async fn an_expiring_token_is_never_refreshed_or_used() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.accounts[0]["access_token_expires_at_ms"] = json!(1000);
    let mut peer = Peer::start(ENABLED()).await;
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 409);
    assert!(
        body["error"]["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["code"] == "token_expired")
    );
    assert!(
        !host
            .calls
            .iter()
            .any(|c| c == "host.auth.get" || c == "host.http.account")
    );
}

#[tokio::test]
async fn modeltrace_goes_through_the_cpr_model_chain_one_challenge_at_a_time() {
    let mut host = FakeHost::new(&["acct_a"]);
    for seed in 0..3 {
        host.models.push_back(Ok(numbers(320, seed)));
    }
    let mut peer = Peer::start(WITH_KEY()).await;
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"modeltrace","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    let mut run = body["run"].clone();
    assert_eq!(run["status"], "running");
    assert_eq!(run["progress"], json!({"answered":0,"total":3}));
    assert!(
        run.get("pending").is_none(),
        "prompts must not reach the page"
    );
    assert!(
        !host
            .calls
            .iter()
            .any(|c| c == "host.keys.ensure" || c.starts_with("host.groups.")),
        "the plugin creates no key or group of its own"
    );

    // 同账号串行：进行中时再发起会被拦
    let (status, busy) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(busy["error"]["blockers"][0]["code"], "busy");

    let id = run["id"].as_str().unwrap().to_owned();
    for _ in 0..3 {
        let (status, body) = peer
            .api(
                "POST",
                "api/probe/step",
                "",
                Some(json!({"run_id": id, "account_id": "acct_a"})),
                &mut host,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        run = body["run"].clone();
    }
    assert_eq!(run["status"], "done", "{run}");
    let mt = &run["modeltrace"];
    assert_eq!(mt["used_outputs"], 3);
    assert_eq!(mt["outputs"][0]["output_tokens"], 900);
    assert!(
        mt["outputs"][0]["excerpt"]
            .as_str()
            .unwrap()
            .starts_with(&numbers(3, 0))
    );
    assert!(
        !mt["outputs"][0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("thinking")
    );
    assert!(run["verdict"]["level"].is_string());

    assert_eq!(host.seen_models.len(), 3);
    let (params, body) = &host.seen_models[0];
    assert_eq!(params["client_key_id"], "key_mine");
    assert_eq!(params["account_id"], "acct_a");
    assert_eq!(params["provider"], "openai");
    assert_eq!(params["protocol"], "openai");
    assert_eq!(params["operation"], "generate");
    assert_eq!(body["model"], "gpt-6-astra");
    assert!(
        body["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("1 到 355")
    );
    assert!(
        host.seen_http.is_empty(),
        "ModelTrace must not go around CPR"
    );

    // 完成后不再保存题目和回答原文
    let stored = host.get("runs", &id).unwrap();
    assert!(stored.get("pending").is_none());
}

#[tokio::test]
async fn schedules_save_and_drive_automatic_probes() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut peer = Peer::start(ENABLED()).await;
    let (status, body) = peer
        .api("POST", "api/schedule", "", Some(json!({"account_id":"acct_a","method":"turnstate","enabled":true,"model":"","interval_minutes":60})), &mut host)
        .await;
    assert_eq!(status, 400, "enabling without a model is refused: {body}");
    let (status, _) = peer
        .api("POST", "api/schedule", "", Some(json!({"account_id":"acct_a","method":"turnstate","enabled":true,"model":"gpt-6-astra","interval_minutes":60})), &mut host)
        .await;
    assert_eq!(status, 200);

    host.http.push_back(ticket_reply(Some("gAAAAAt1"), None));
    host.http.push_back(ticket_reply(Some("gAAAAAt2"), None));
    host.http.push_back(ticket_reply(Some("gAAAAAt3"), None));
    host.http.push_back(ticket_reply(Some("gAAAAAt4"), None));
    peer.reconcile(&mut host).await;
    assert_eq!(host.seen_http.len(), 4);
    let history = host.get("history", "acct_a/turnstate").unwrap();
    assert_eq!(history["runs"][0]["trigger"], "auto");
    assert_eq!(history["runs"][0]["level"], "degraded");

    // 间隔没到，下一轮不再发
    peer.reconcile(&mut host).await;
    assert_eq!(host.seen_http.len(), 4);
    let (_, body) = peer.api("GET", "api/overview", "", None, &mut host).await;
    let strip = body["accounts"][0]["strip"].as_array().unwrap();
    // 最近一次自动测票落在最后一格
    let last = strip.last().unwrap();
    assert_eq!(last["i"], 95);
    assert_eq!(last["degraded"], 1);
}

#[tokio::test]
async fn fingerprint_sync_downloads_then_reuses_the_validated_commit() {
    let mut host = FakeHost::default();
    let sha = "a".repeat(40);
    let commits = serde_json::to_vec(
        &json!([{"sha":sha,"commit":{"committer":{"date":"2026-10-28T00:00:00Z"}}}]),
    )
    .unwrap();
    host.public_http.push_back(HttpReply {
        status: 200,
        headers: vec![],
        body: commits.clone(),
    });
    host.public_http.push_back(HttpReply {
        status: 200,
        headers: vec![],
        body: include_bytes!("../data/unified_bank.json").to_vec(),
    });
    let mut peer = Peer::start(json!({})).await;
    let (status, body) = peer
        .api(
            "POST",
            "api/fingerprint/sync",
            "",
            Some(json!({})),
            &mut host,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["bank"]["commit"], sha);
    assert_eq!(body["bank"]["source"], "github");
    assert!(body["bank"].get("error").is_none(), "{body}");
    assert_eq!(host.seen_public_http.len(), 2);
    assert_eq!(host.seen_public_http[0]["method"], "GET");
    assert_eq!(
        host.seen_public_http[1]["url"],
        format!(
            "https://raw.githubusercontent.com/xqy2006/ModelTrace/{sha}/static/data/unified_bank.json"
        )
    );
    host.public_http.push_back(HttpReply {
        status: 200,
        headers: vec![],
        body: commits,
    });
    let (_, body) = peer
        .api(
            "POST",
            "api/fingerprint/sync",
            "",
            Some(json!({})),
            &mut host,
        )
        .await;
    assert_eq!(body["bank"]["source"], "github");
    assert_eq!(
        host.seen_public_http.len(),
        3,
        "unchanged commit must not download again"
    );
    assert!(
        !host
            .calls
            .iter()
            .any(|c| c == "host.auth.get" || c == "host.http.account" || c == "host.model.execute")
    );
}

#[tokio::test]
async fn invalid_fingerprint_update_keeps_the_embedded_bank() {
    let mut host = FakeHost::default();
    host.public_http.push_back(HttpReply {
        status: 200,
        headers: vec![],
        body: serde_json::to_vec(&json!([{"sha":"b".repeat(40)}])).unwrap(),
    });
    host.public_http.push_back(HttpReply {
        status: 200,
        headers: vec![],
        body: b"{}".to_vec(),
    });
    let mut peer = Peer::start(json!({})).await;
    let (_, body) = peer
        .api(
            "POST",
            "api/fingerprint/sync",
            "",
            Some(json!({})),
            &mut host,
        )
        .await;
    assert_eq!(body["bank"]["source"], "embedded");
    assert!(
        body["bank"]["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty())
    );
    assert!(body["bank"]["models"].as_array().unwrap().len() > 1);
}

#[tokio::test]
async fn turnstate_uses_the_current_cpr_profile_without_a_local_ua_override() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut peer = Peer::start(json!({"probeEnabled":true,"codexVersion":"0.1.0"})).await;
    for (ua, originator, version) in [
        (
            "codex_exec/0.156.1 custom terminal",
            "codex_exec",
            "0.156.1",
        ),
        (
            "Codex Desktop/26.10.1 (Linux; aarch64)",
            "Codex Desktop",
            "0.160.0",
        ),
    ] {
        host.profile = Some(json!({"userAgent":ua,"originator":originator,"codexVersion":version}));
        let (_, plan) = peer
            .api(
                "POST",
                "api/plan",
                "",
                Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
                &mut host,
            )
            .await;
        assert_eq!(plan["request"]["headers"]["User-Agent"], ua);
        assert_eq!(plan["request"]["headers"]["Originator"], originator);
        assert_eq!(plan["request"]["headers"]["Version"], version);
        host.http.push_back(ticket_reply(None, None));
        let (_, body) = peer
            .api(
                "POST",
                "api/probe",
                "",
                Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
                &mut host,
            )
            .await;
        assert_eq!(body["run"]["status"], "done", "{body}");
        let request = host.seen_http.last().unwrap();
        assert_eq!(request.header("User-Agent").as_deref(), Some(ua));
        assert_eq!(request.header("Originator").as_deref(), Some(originator));
        assert_eq!(request.header("Version").as_deref(), Some(version));
    }
}

#[tokio::test]
async fn missing_cpr_identity_never_falls_back_to_an_invented_ua() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.profile =
        Some(json!({"userAgent":"bad\r\nInjected: secret","originator":"x","codexVersion":"1.0"}));
    let mut peer = Peer::start(ENABLED()).await;
    let (_, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"turnstate","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(body["run"]["status"], "failed", "{body}");
    assert!(host.seen_http.is_empty());
    assert!(!body.to_string().contains("Injected"));
}

#[tokio::test]
async fn modeltrace_is_manual_only_and_old_schedules_are_ignored() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.state.insert(("settings".into(),"schedules".into()), (json!({"accounts":{"acct_a":{"modeltrace":{"enabled":true,"model":"gpt-6-astra","interval_minutes":60}}}}),1));
    host.models.push_back(Ok(numbers(320, 0)));
    let mut peer = Peer::start(WITH_KEY()).await;
    for _ in 0..3 {
        peer.reconcile(&mut host).await;
    }
    assert!(
        host.seen_models.is_empty(),
        "a saved ModelTrace schedule must not run"
    );
    assert!(host.get("history", "acct_a/modeltrace").is_none());
    let (_, overview) = peer.api("GET", "api/overview", "", None, &mut host).await;
    let schedule = &overview["accounts"][0]["schedule"]["modeltrace"];
    assert_eq!(schedule["enabled"], false);
    assert_eq!(schedule["model"], "gpt-6-astra");

    let (status, body) = peer
        .api("POST", "api/schedule", "", Some(json!({"account_id":"acct_a","method":"modeltrace","enabled":true,"model":"gpt-6-astra","interval_minutes":60})), &mut host)
        .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = peer
        .api(
            "POST",
            "api/schedules/batch",
            "",
            Some(json!({"account_ids":["acct_a"],"method":"modeltrace","enabled":true})),
            &mut host,
        )
        .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = peer
        .api("POST", "api/schedule", "", Some(json!({"account_id":"acct_a","method":"modeltrace","enabled":false,"model":"gpt-6.1-sol","interval_minutes":60})), &mut host)
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["schedule"]["modeltrace"]["enabled"], false);

    // Manual ModelTrace still runs and is recorded.
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"modeltrace","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["run"]["status"], "running", "{body}");
    assert_eq!(body["run"]["trigger"], "manual");
    assert_eq!(
        host.get("history", "acct_a/modeltrace").unwrap()["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn overview_ranges_include_only_their_records_and_distinguish_empty_slots() {
    let mut host = FakeHost::new(&["acct_a"]);
    let now = cpr_account_probe::clock::now_ms();
    let summary = |id: &str, age_hours: i64, status: &str, level: Option<&str>| {
        json!({
            "id":id,"model":"gpt-6-astra","trigger":"manual","started_ms":now-age_hours*3_600_000,
            "finished_ms":now-age_hours*3_600_000+1,"status":status,"level":level,"reason":"test","detail":true
        })
    };
    host.state.insert(
        ("history".into(), "acct_a/turnstate".into()),
        (
            json!({"runs":[
                summary("older",24*20,"done",Some("degraded")),
                summary("week",24*3,"done",Some("suspect")),
                summary("day",3,"done",Some("ok")),
                summary("failed",2,"failed",None),
                summary("running",0,"running",None),
            ]}),
            1,
        ),
    );
    let mut peer = Peer::start(json!({})).await;
    for (hours, expected_count, judged, ok, expected_runs) in
        [(24, 96, 1, 1, 3), (168, 84, 2, 1, 4), (720, 120, 3, 1, 5)]
    {
        let (status, body) = peer
            .api(
                "GET",
                "api/overview",
                &format!("hours={hours}"),
                None,
                &mut host,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let account = &body["accounts"][0];
        let range = &body["range"];
        assert_eq!(range["slots"], expected_count);
        let start = range["start_ms"].as_i64().unwrap();
        let slot_ms = range["slot_ms"].as_i64().unwrap();
        let end = start + slot_ms * expected_count;
        assert_eq!(end - start, hours * 3_600_000);
        assert!(start <= now - hours * 3_600_000 + slot_ms && end > now);
        assert_eq!(
            account["rate"],
            json!({"judged":judged,"ok":ok}),
            "failures and empty time must not count as healthy"
        );
        // 只列出有探测的格子，每格只带各结果的次数
        let strip = account["strip"].as_array().unwrap();
        assert!(strip.len() < usize::try_from(expected_count).unwrap());
        let sum = |field: &str| -> u64 { strip.iter().filter_map(|c| c[field].as_u64()).sum() };
        let total: u64 = ["ok", "suspect", "degraded", "unknown", "failed", "running"]
            .iter()
            .map(|f| sum(f))
            .sum();
        assert_eq!(total, expected_runs);
        assert_eq!(sum("running"), 1);
        assert_eq!(sum("failed"), 1);
        // 弹层按时间范围取记录，新的在前
        let (status, page) = peer
            .api(
                "GET",
                "api/runs",
                &format!("account=acct_a&method=turnstate&from={start}&to={end}"),
                None,
                &mut host,
            )
            .await;
        assert_eq!(status, 200, "{page}");
        assert_eq!(page["complete"], true);
        let runs = page["runs"].as_array().unwrap();
        assert_eq!(runs.len() as u64, expected_runs);
        assert_eq!(runs[0]["id"], "running");
        assert!(
            runs[0]["level"].is_null(),
            "a running probe has no verdict yet"
        );
        assert_eq!(runs[1]["id"], "failed");
        assert_eq!(page["counts"]["failed"], 1);
    }
    for query in ["hours=0", "hours=1000000", "hours=bad"] {
        assert_eq!(
            peer.api("GET", "api/overview", query, None, &mut host)
                .await
                .0,
            400
        );
    }
}

#[tokio::test]
async fn defaults_are_method_specific_and_existing_models_are_preserved() {
    let mut host = FakeHost::new(&["acct_a", "acct_b"]);
    host.state.insert(("settings".into(),"schedules".into()),(json!({"accounts":{
        "acct_b":{"turnstate":{"enabled":false,"model":"custom-ticket","interval_minutes":90},"modeltrace":{"enabled":false,"model":"","interval_minutes":120}}
    }}),1));
    let mut peer = Peer::start(json!({})).await;
    let (_, body) = peer.api("GET", "api/overview", "", None, &mut host).await;
    let a = &body["accounts"][0]["schedule"];
    assert_eq!(a["turnstate"]["model"], "gpt-6-luna");
    assert_eq!(a["modeltrace"]["model"], "gpt-6.1-sol");
    assert_eq!(a["turnstate"]["enabled"], false);
    assert_eq!(a["modeltrace"]["enabled"], false);
    let b = &body["accounts"][1]["schedule"];
    assert_eq!(b["turnstate"]["model"], "custom-ticket");
    assert_eq!(b["turnstate"]["interval_minutes"], 90);
    assert_eq!(b["modeltrace"]["model"], "gpt-6.1-sol");
    assert_eq!(b["modeltrace"]["interval_minutes"], 120);
    assert!(!host.calls.iter().any(|c| c == "host.state.put"
        || c == "host.auth.get"
        || c.starts_with("host.http.")
        || c == "host.model.execute"));
}

#[tokio::test]
async fn batch_configuration_changes_only_selected_fields_accounts_and_method() {
    let mut host = FakeHost::new(&["acct_a", "acct_b", "acct_c"]);
    host.state.insert(("settings".into(),"schedules".into()),(json!({"accounts":{
        "acct_a":{"turnstate":{"enabled":false,"model":"ticket-a","interval_minutes":90},"modeltrace":{"enabled":true,"model":"trace-a","interval_minutes":120}},
        "acct_b":{"turnstate":{"enabled":true,"model":"ticket-b","interval_minutes":180}},
        "acct_c":{"turnstate":{"enabled":true,"model":"ticket-c","interval_minutes":240}}
    }}),1));
    let mut peer = Peer::start(json!({})).await;
    let (status, body) = peer.api("POST","api/schedules/batch","",Some(json!({"account_ids":["acct_a","acct_b"],"method":"turnstate","interval_minutes":60})),&mut host).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["updated"], 2);
    let state = host.get("settings", "schedules").unwrap();
    assert_eq!(
        state["accounts"]["acct_a"]["turnstate"],
        json!({"enabled":false,"model":"ticket-a","interval_minutes":60})
    );
    assert_eq!(
        state["accounts"]["acct_b"]["turnstate"],
        json!({"enabled":true,"model":"ticket-b","interval_minutes":60})
    );
    assert_eq!(
        state["accounts"]["acct_a"]["modeltrace"],
        json!({"enabled":false,"model":"trace-a","interval_minutes":120}),
        "the old ModelTrace switch is dropped, its model and interval kept"
    );
    assert_eq!(
        state["accounts"]["acct_c"]["turnstate"]["interval_minutes"],
        240
    );
    let (status, body) = peer.api("POST","api/schedules/batch","",Some(json!({"account_ids":["acct_a","acct_b"],"method":"turnstate","model":"gpt-6-luna","enabled":true})),&mut host).await;
    assert_eq!(status, 200, "{body}");
    let state = host.get("settings", "schedules").unwrap();
    for id in ["acct_a", "acct_b"] {
        assert_eq!(
            state["accounts"][id]["turnstate"],
            json!({"enabled":true,"model":"gpt-6-luna","interval_minutes":60})
        );
    }
    assert!(
        !host.calls.iter().any(|c| c == "host.auth.get"
            || c.starts_with("host.http.")
            || c == "host.model.execute")
    );
    assert_eq!(
        host.calls
            .iter()
            .filter(|c| c.as_str() == "host.state.put")
            .count(),
        2,
        "one state write for each batch"
    );
}

#[tokio::test]
async fn invalid_batch_configuration_changes_nothing() {
    let mut host = FakeHost::new(&["acct_a", "acct_b"]);
    host.accounts[1]["authentication_kind"] = json!("apikey");
    let mut peer = Peer::start(json!({})).await;
    for request in [
        json!({"account_ids":[],"method":"turnstate","enabled":true}),
        json!({"account_ids":vec!["acct_a";201],"method":"turnstate","enabled":true}),
        json!({"account_ids":["acct_a","acct_a"],"method":"turnstate","enabled":true}),
        json!({"account_ids":["acct_a"],"method":"turnstate"}),
        json!({"account_ids":["acct_a"],"method":"turnstate","model":""}),
        json!({"account_ids":["acct_a"],"method":"turnstate","model":"bad model"}),
        json!({"account_ids":["acct_a"],"method":"turnstate","interval_minutes":0}),
        json!({"account_ids":["acct_a"],"method":"turnstate","enabled":true,"unrecognized":true}),
        json!({"account_ids":["acct_a","missing"],"method":"turnstate","enabled":true}),
        json!({"account_ids":["acct_a","acct_b"],"method":"turnstate","enabled":true}),
    ] {
        let (status, body) = peer
            .api(
                "POST",
                "api/schedules/batch",
                "",
                Some(request.clone()),
                &mut host,
            )
            .await;
        assert_eq!(status, 400, "{request}: {body}");
        assert!(host.get("settings", "schedules").is_none());
    }
    assert!(!host.calls.iter().any(|c| c == "host.state.put"));
}

#[tokio::test]
async fn modeltrace_needs_a_configured_client_key_and_creates_nothing() {
    let mut host = FakeHost::new(&["acct_a"]);
    let mut peer = Peer::start(ENABLED()).await;
    peer.reconcile(&mut host).await;
    let (status, plan) = peer
        .api(
            "POST",
            "api/plan",
            "",
            Some(json!({"account_id":"acct_a","method":"modeltrace","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 200, "{plan}");
    assert!(plan.to_string().contains("no_client_key"), "{plan}");
    let (status, body) = peer
        .api(
            "POST",
            "api/probe",
            "",
            Some(json!({"account_id":"acct_a","method":"modeltrace","model":"gpt-6-astra"})),
            &mut host,
        )
        .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["blockers"][0]["code"], "no_client_key");
    let (_, models) = peer.api("GET", "api/models", "", None, &mut host).await;
    assert!(
        models["cpr"].is_null(),
        "no key, no CPR model list: {models}"
    );
    assert!(host.seen_models.is_empty());
    assert!(
        !host
            .calls
            .iter()
            .any(|c| c == "host.keys.ensure" || c.starts_with("host.groups.")),
        "{:?}",
        host.calls
    );
}

/// 每 10 分钟自动测票时，30 天的摘要会超过宿主单条 256 KiB 的上限（生产里的 502）
fn crowded_history(host: &mut FakeHost, count: usize, reason: &str) {
    let now = cpr_account_probe::clock::now_ms();
    let runs: Vec<Value> = (0..count)
        .map(|i| {
            let started = now - 600_000 * (count - i) as i64;
            json!({
                "id": format!("old-{i}"), "model": "gpt-6-astra", "trigger": "auto",
                "started_ms": started, "finished_ms": started + 60_000,
                "status": "done", "level": "degraded", "reason": reason,
                // 最旧的一条还留着详情，被丢掉时要一并删掉
                "detail": i == 0,
            })
        })
        .collect();
    host.state.insert(
        ("runs".into(), "old-0".into()),
        (
            json!({
                "id": "old-0", "account_id": "acct_a", "method": "turnstate",
                "model": "gpt-6-astra", "trigger": "auto",
                "started_ms": now - 600_000 * count as i64, "status": "failed",
            }),
            1,
        ),
    );
    host.state.insert(
        ("history".into(), "acct_a/turnstate".into()),
        (json!({"runs": runs}), 1),
    );
}

fn history_bytes(host: &FakeHost) -> usize {
    serde_json::to_vec(host.get("history", "acct_a/turnstate").unwrap())
        .unwrap()
        .len()
}

fn strip_total(body: &Value) -> u64 {
    body["accounts"][0]["strip"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| {
            ["ok", "suspect", "degraded", "unknown", "failed", "running"]
                .into_iter()
                .filter_map(|f| c[f].as_u64())
        })
        .sum()
}

/// 旧版的历史只有 30 天的逐条摘要：读的时候照常展示，下次写入时把 48 小时前的并进按小时汇总，次数不丢
#[tokio::test]
async fn legacy_history_is_compacted_into_hourly_counts_without_losing_probes() {
    let mut host = FakeHost::new(&["acct_a"]);
    let reason = "连续 3 次带回最新的票，每次都被换成新票（KlN 的判据：全部刷新即降智）";
    // 每 10 分钟一条，约 6.6 天
    crowded_history(&mut host, 950, reason);
    assert!(history_bytes(&host) > cpr_account_probe::store::HISTORY_MAX_BYTES);
    let mut peer = Peer::start(ENABLED()).await;
    // 升级后还没写入：读出来就是完整的
    let (_, before) = peer
        .api("GET", "api/overview", "hours=168", None, &mut host)
        .await;
    assert_eq!(strip_total(&before), 950);
    assert_eq!(before["accounts"][0]["rate"], json!({"judged":950,"ok":0}));

    ticket_probe(&mut peer, &mut host, "ok").await;
    let stored = host.get("history", "acct_a/turnstate").unwrap().clone();
    let bytes = serde_json::to_vec(&stored).unwrap().len();
    assert!(bytes < 120 * 1024, "compacted history is {bytes} bytes");
    // 48 小时内的约 288 条和这次的一条逐条保存，其余并进按小时汇总
    let runs = stored["runs"].as_array().unwrap();
    assert!((287..=289).contains(&runs.len()), "{}", runs.len());
    assert_eq!(runs[0]["reason"], reason);
    let folded: u64 = stored["hours"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["degraded"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(folded + runs.len() as u64, 951);
    let raw = runs.len();
    // 被并掉的那条的详情一起删了
    assert!(host.get("runs", "old-0").is_none());
    // 展示的次数不变
    let (_, after) = peer
        .api("GET", "api/overview", "hours=168", None, &mut host)
        .await;
    assert_eq!(strip_total(&after), 951);
    assert_eq!(after["accounts"][0]["rate"], json!({"judged":951,"ok":1}));
    // 当前判定照旧
    assert_eq!(after["accounts"][0]["health"]["level"], "degraded");
    // 详情：30 天每天的次数加起来也是 951，逐条记录从 48 小时前开始
    let (_, detail) = peer
        .api("GET", "api/account", "id=acct_a", None, &mut host)
        .await;
    let turnstate = &detail["account"]["methods"]["turnstate"];
    let daily = turnstate["daily"].as_array().unwrap();
    assert_eq!(daily.len(), 30);
    let days: u64 = daily
        .iter()
        .map(|d| d["degraded"].as_u64().unwrap_or(0) + d["ok"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(days, 951);
    assert_eq!(turnstate["runs"].as_array().unwrap().len(), raw);
    assert!(turnstate["raw_since"].is_i64());
    // 只剩次数的时间段，弹层能拿到次数，并说明没有逐条记录
    let now = cpr_account_probe::clock::now_ms();
    let (_, page) = peer
        .api(
            "GET",
            "api/runs",
            &format!(
                "account=acct_a&method=turnstate&from={}&to={}",
                now - 5 * 86_400_000,
                now - 4 * 86_400_000
            ),
            None,
            &mut host,
        )
        .await;
    assert_eq!(page["complete"], false);
    assert!(page["runs"].as_array().unwrap().is_empty());
    assert!(page["counts"]["degraded"].as_u64().unwrap() >= 140);
}

/// 探测非常频繁时 48 小时内的逐条摘要也会超过上限：去掉旧理由、再把最旧的并进汇总，次数不丢
#[tokio::test]
async fn very_frequent_probes_stay_under_the_record_limit_without_losing_counts() {
    let mut host = FakeHost::new(&["acct_a"]);
    let now = cpr_account_probe::clock::now_ms();
    let reason = "连续 3 次带回最新的票，每次都被换成新票（KlN 的判据：全部刷新即降智）";
    // 每分钟一条，47 小时
    let count = 47 * 60;
    let runs: Vec<Value> = (0..count)
        .map(|i| {
            let started = now - 60_000 * (count - i) as i64;
            json!({
                "id": format!("f-{i}"), "model": "gpt-6-astra", "trigger": "auto",
                "started_ms": started, "finished_ms": started + 1000,
                "status": "done", "level": "degraded", "reason": reason, "detail": false,
            })
        })
        .collect();
    host.state.insert(
        ("history".into(), "acct_a/turnstate".into()),
        (json!({"runs": runs}), 1),
    );
    let mut peer = Peer::start(ENABLED()).await;
    ticket_probe(&mut peer, &mut host, "ok").await;
    assert!(history_bytes(&host) <= cpr_account_probe::store::HISTORY_MAX_BYTES);
    let (_, body) = peer
        .api("GET", "api/overview", "hours=168", None, &mut host)
        .await;
    assert_eq!(strip_total(&body), count as u64 + 1);
    // 之后的测票照常写入
    ticket_probe(&mut peer, &mut host, "ok").await;
    let (_, body) = peer
        .api("GET", "api/overview", "hours=24", None, &mut host)
        .await;
    assert_eq!(body["accounts"][0]["latest"]["turnstate"]["level"], "ok");
}

/// 宿主的管理响应正文最多 1 MiB；超了页面报“插件管理响应状态、内容类型或正文无效”
#[tokio::test]
async fn overview_stays_under_the_management_body_limit_for_busy_accounts() {
    let ids: Vec<String> = (0..16).map(|i| format!("acct_{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut host = FakeHost::new(&refs);
    let now = cpr_account_probe::clock::now_ms();
    let reason = "连续 3 次带回最新的票，每次都被换成新票（KlN 的判据：全部刷新即降智）";
    for id in &ids {
        // 每 10 分钟一次，7 天
        let runs: Vec<Value> = (0..1008)
            .map(|i| {
                let started = now - 600_000 * (1008 - i);
                json!({
                    "id": format!("{id}-{i}"), "model": "gpt-6-luna", "trigger": "auto",
                    "started_ms": started, "finished_ms": started + 60_000,
                    "status": "done", "level": "degraded", "reason": reason, "detail": false,
                })
            })
            .collect();
        host.state.insert(
            ("history".into(), format!("{id}/turnstate")),
            (json!({"runs": runs}), 1),
        );
    }
    let mut peer = Peer::start(json!({})).await;
    for hours in [24, 168, 720] {
        let (status, body) = peer
            .api(
                "GET",
                "api/overview",
                &format!("hours={hours}"),
                None,
                &mut host,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let bytes = serde_json::to_vec(&body).unwrap().len();
        assert!(bytes < 1024 * 1024, "{hours}h overview is {bytes} bytes");
    }
    // 弹层只取一个账号一段时间的记录
    let (status, page) = peer
        .api(
            "GET",
            "api/runs",
            &format!(
                "account=acct_0&method=turnstate&from={}&to={}",
                now - 7_200_000,
                now + 1
            ),
            None,
            &mut host,
        )
        .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["runs"].as_array().unwrap().len(), 12);
    for bad in [
        "account=acct_0&method=nope&from=0&to=1",
        "account=acct_0&method=turnstate&from=x&to=1",
        "method=turnstate&from=0&to=1",
    ] {
        assert_eq!(
            peer.api("GET", "api/runs", bad, None, &mut host).await.0,
            400,
            "{bad}"
        );
    }
}
