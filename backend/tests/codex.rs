use cpr_account_probe::codex::{
    self, CookieJar, Credentials, INSTRUCTIONS, StreamReader, TURN_STATE_HEADER,
};
use sha2::{Digest, Sha256};

fn creds() -> Credentials {
    Credentials {
        access_token: "tok-secret-123".into(),
        chatgpt_account_id: "acct-0000-1111".into(),
    }
}

fn sse(events: &[&str]) -> Vec<u8> {
    let mut out = String::new();
    for e in events {
        out.push_str("event: x\ndata: ");
        out.push_str(e);
        out.push_str("\n\n");
    }
    out.into_bytes()
}

fn read(bytes: &[u8]) -> Result<codex::StreamOutcome, cpr_account_probe::verdict::UpstreamError> {
    let mut r = StreamReader::default();
    // 拆成小块喂，确认跨块的行能拼起来
    for chunk in bytes.chunks(7) {
        if let Some(done) = r.feed(chunk) {
            return done;
        }
    }
    r.finish()
}

#[test]
fn instructions_are_the_official_codex_prompt() {
    let sum = hex::encode(Sha256::digest(INSTRUCTIONS.as_bytes()));
    assert_eq!(
        sum,
        "42842be69650ae563d212695e8d3f3591534908fd8ca33b63f742daf41f88b65"
    );
}

#[test]
fn echo_request_is_the_connection_test_plus_the_ticket() {
    let (headers, body) = codex::echo_request(
        &creds(),
        "0.146.0",
        "gpt-6-astra",
        "sess-1",
        "ticket-1",
        "__oailb=r7",
    )
    .unwrap();
    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| String::from_utf8(v.clone()).unwrap())
    };
    assert_eq!(get("Authorization").unwrap(), "Bearer tok-secret-123");
    assert_eq!(get("Chatgpt-Account-Id").unwrap(), "acct-0000-1111");
    assert_eq!(get("Originator").unwrap(), "codex-tui");
    assert_eq!(get("Version").unwrap(), "0.146.0");
    assert_eq!(
        get("User-Agent").unwrap(),
        "codex-tui/0.146.0 (Ubuntu 22.4.0; x86_64) xterm-256color"
    );
    assert_eq!(get("Openai-Beta").unwrap(), "responses=experimental");
    assert_eq!(get("Session_id").unwrap(), "sess-1");
    assert_eq!(get(TURN_STATE_HEADER).unwrap(), "ticket-1");
    assert_eq!(get("Cookie").unwrap(), "__oailb=r7");
    let names: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        names,
        [
            "Content-Type",
            "Authorization",
            "Accept",
            "Openai-Beta",
            "Originator",
            "User-Agent",
            "Version",
            "Chatgpt-Account-Id",
            "Session_id",
            TURN_STATE_HEADER,
            "Cookie"
        ]
    );
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["model"], "gpt-6-astra");
    assert_eq!(body["reasoning"]["effort"], "low");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["input"][0]["content"][0]["text"], "hi");
    assert_eq!(body["instructions"], INSTRUCTIONS);
}

#[test]
fn the_first_shot_carries_no_ticket_and_no_cookie() {
    let (headers, _) = codex::echo_request(&creds(), "0.146.0", "m", "s", "", "").unwrap();
    assert!(
        headers
            .iter()
            .all(|(k, _)| k != TURN_STATE_HEADER && k != "Cookie")
    );
}

#[test]
fn echo_request_rejects_bad_input() {
    assert!(codex::echo_request(&creds(), "0.146.0", "bad model", "s", "", "").is_err());
    let mut c = creds();
    c.access_token = "a\r\nb".into();
    assert!(codex::echo_request(&c, "0.146.0", "m", "s", "", "").is_err());
    assert!(codex::echo_request(&creds(), "0.146.0", "m", "s", "t\n", "").is_err());
}

#[test]
fn reads_the_stream() {
    let out = read(&sse(&[
        r#"{"type":"response.created","response":{"model":"gpt-6-astra"}}"#,
        r#"{"type":"response.output_text.delta","delta":"12, "}"#,
        r#"{"type":"response.output_text.delta","delta":"34"}"#,
        r#"{"type":"response.completed","response":{"status":"completed","model":"gpt-6-astra"}}"#,
    ]))
    .unwrap();
    assert_eq!(out.text, "12, 34");
    assert_eq!(out.status, "completed");
    assert_eq!(out.response_model, "gpt-6-astra");
}

#[test]
fn falls_back_to_the_final_output() {
    let out = read(&sse(&[
        r#"{"type":"response.completed","response":{"status":"completed","output":[{"content":[{"type":"output_text","text":"final"}]}]}}"#,
    ]))
    .unwrap();
    assert_eq!(out.text, "final");
}

#[test]
fn reads_a_ticket_from_the_metadata_event() {
    let out = read(&sse(&[
        r#"{"type":"response.metadata","headers":{"x-codex-turn-state":" t-9 "}}"#,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    ]))
    .unwrap();
    assert_eq!(out.metadata_turn_state, "t-9");
}

#[test]
fn stream_failures_are_errors() {
    let e = read(&sse(&[r#"{"type":"response.failed","response":{"error":{"code":"rate_limit_exceeded","message":"slow"}}}"#])).unwrap_err();
    assert_eq!(e.kind, "rate_limited");
    let e = read(&sse(&[
        r#"{"type":"error","error":{"message":"Our servers are currently overloaded"}}"#,
    ]))
    .unwrap_err();
    assert_eq!(e.kind, "overloaded");
    let e = read(&sse(&[
        r#"{"type":"response.output_text.delta","delta":"x"}"#,
    ]))
    .unwrap_err();
    assert_eq!(e.kind, "protocol");
    let e = read(b"data: [DONE]\n\n").unwrap_err();
    assert_eq!(e.kind, "protocol");
}

#[test]
fn cookie_jar_follows_the_route() {
    let mut jar = CookieJar::default();
    assert_eq!(jar.route(), "");
    jar.store(&[
        (
            "set-cookie".into(),
            b"__oailb=route-7; Path=/; HttpOnly".to_vec(),
        ),
        ("Set-Cookie".into(), b"other=1".to_vec()),
    ]);
    assert_eq!(jar.route(), "__oailb=route-7;");
    assert_eq!(jar.header(), "__oailb=route-7; other=1");
    jar.store(&[("set-cookie".into(), b"__oailb=route-8".to_vec())]);
    assert_eq!(jar.route(), "__oailb=route-8;");
    jar.store(&[("set-cookie".into(), b"other=; Max-Age=0".to_vec())]);
    assert_eq!(jar.header(), "__oailb=route-8");
}

#[test]
fn redacts_credentials_an_upstream_echoes() {
    let s = codex::redact(
        "bad token tok-secret-123 for acct-0000-1111, header was Bearer abc.def-ghi",
        &creds(),
    );
    assert!(
        !s.contains("tok-secret-123") && !s.contains("acct-0000-1111") && !s.contains("abc.def")
    );
    assert!(s.contains("Bearer ‹hidden›"));
}

#[test]
fn error_text_reads_common_shapes() {
    assert_eq!(
        codex::error_text(br#"{"error":{"code":"x","message":"y"}}"#),
        "x y"
    );
    assert_eq!(codex::error_text(br#"{"detail":"nope"}"#), "nope");
    assert_eq!(codex::error_text(b"plain"), "plain");
}
