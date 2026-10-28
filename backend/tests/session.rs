//! 经真实的 RPC 会话跑完整流程，宿主回调由 `support::FakeHost` 模拟

mod support;

use serde_json::{Value, json};
use support::{FakeHost, HttpReply, Peer, ticket_reply};

const ENABLED: fn() -> Value = || json!({"probeEnabled": true, "fingerprintSyncHours": 0});

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
    assert_eq!(
        a["methods"]["turnstate"]["slots"].as_array().unwrap().len(),
        96
    );
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
    let mut peer = Peer::start(ENABLED()).await;
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
    assert!(host.calls.iter().any(|c| c == "host.keys.ensure"));

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
    assert_eq!(params["client_key_id"], "key-1");
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
    let slots = body["accounts"][0]["methods"]["turnstate"]["slots"]
        .as_array()
        .unwrap();
    assert_eq!(slots.last().unwrap()["level"], "degraded");
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
async fn scheduled_modeltrace_advances_across_maintenance_calls() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.state.insert(("settings".into(),"schedules".into()), (json!({"accounts":{"acct_a":{"modeltrace":{"enabled":true,"model":"gpt-6-astra","interval_minutes":60}}}}),1));
    for seed in 0..3 {
        host.models.push_back(Ok(numbers(320, seed)));
    }
    let mut peer = Peer::start(ENABLED()).await;
    peer.reconcile(&mut host).await;
    assert!(
        host.seen_models.is_empty(),
        "starting a run must not answer all three challenges"
    );
    for answered in 1..=3 {
        peer.reconcile(&mut host).await;
        assert_eq!(host.seen_models.len(), answered);
        let history = host.get("history", "acct_a/modeltrace").unwrap();
        assert_eq!(
            history["runs"].as_array().unwrap().len(),
            1,
            "must keep the same run"
        );
        assert_eq!(
            history["runs"][0]["status"],
            if answered == 3 { "done" } else { "running" }
        );
    }
    peer.reconcile(&mut host).await;
    assert_eq!(
        host.seen_models.len(),
        3,
        "completed run must respect the schedule interval"
    );
}

#[tokio::test]
async fn scheduled_modeltrace_rechecks_account_before_the_next_challenge() {
    let mut host = FakeHost::new(&["acct_a"]);
    host.state.insert(("settings".into(),"schedules".into()), (json!({"accounts":{"acct_a":{"modeltrace":{"enabled":true,"model":"gpt-6-astra","interval_minutes":60}}}}),1));
    let mut peer = Peer::start(ENABLED()).await;
    peer.reconcile(&mut host).await;
    host.accounts[0]["enabled"] = json!(false);
    peer.reconcile(&mut host).await;
    assert!(host.seen_models.is_empty());
    assert_eq!(
        host.get("history", "acct_a/modeltrace").unwrap()["runs"][0]["status"],
        "failed"
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
        ("history".into(), "acct_a/modeltrace".into()),
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
    for (hours, expected_count, judged, ok) in [(24, 96, 1, 1), (168, 84, 2, 1), (720, 120, 3, 1)] {
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
        let method = &body["accounts"][0]["methods"]["modeltrace"];
        let slots = method["slots"].as_array().unwrap();
        assert_eq!(slots.len(), expected_count);
        assert_eq!(
            method["rate"],
            json!({"judged":judged,"ok":ok}),
            "failures and empty time must not count as healthy"
        );
        let start = slots[0]["start_ms"].as_i64().unwrap();
        let end = slots.last().unwrap()["end_ms"].as_i64().unwrap();
        assert_eq!(end - start, hours * 3_600_000);
        assert!(
            slots
                .iter()
                .any(|s| s["runs"].as_array().unwrap().is_empty() && s["level"].is_null())
        );
        let all_runs: Vec<_> = slots
            .iter()
            .flat_map(|s| s["runs"].as_array().unwrap())
            .collect();
        let failed = all_runs.iter().find(|r| r["id"] == "failed").unwrap();
        assert_eq!(failed["status"], "failed");
        let running = all_runs.iter().find(|r| r["id"] == "running").unwrap();
        assert_eq!(running["status"], "running");
        assert!(
            running["level"].is_null(),
            "a running probe has no verdict yet"
        );
        for slot in slots {
            let runs = slot["runs"].as_array().unwrap();
            if !runs.is_empty() && runs.iter().all(|r| r["status"] == "failed") {
                assert_eq!(slot["level"], "unknown");
            }
        }
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
