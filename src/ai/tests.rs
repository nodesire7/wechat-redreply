use super::*;
use super::{
    config::{Profile, Settings},
    store, worker,
};
use crate::{
    config as app_config,
    database::{Message, Peer},
    service::Shared,
};
use std::{
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

fn settings() -> Settings {
    Settings {
        enabled: true,
        active_profile: "local".into(),
        profiles: vec![Profile {
            id: "local".into(),
            name: "本地测试".into(),
            base_url: "http://127.0.0.1:11434/v1".into(),
            model: "fixture".into(),
            allow_http: true,
            require_key: false,
            ..Default::default()
        }],
        ..Default::default()
    }
}
fn observed(
    data: &std::path::Path,
    account: &str,
    name: &str,
    wxid: &str,
    sender: &str,
    id: u64,
    group: bool,
) -> store::Source {
    let peer = Peer {
        name: name.into(),
        wxid: wxid.into(),
        group,
    };
    let row = Message {
        local_id: 1,
        server_id: 1,
        sort_seq: 1,
        create_time: app_config::now() as i64,
        kind: 1,
        sender: sender.into(),
        content: "你好\n\"双引号\" @ [CQ:image,file=evil]".into(),
        shard: "fixture".into(),
    };
    store::observe(data, account, &peer, 123, &row, id).unwrap();
    store::source(data, account, id).unwrap()
}
fn submit(
    data: &std::path::Path,
    s: &Settings,
    source: Option<&store::Source>,
    request: &str,
) -> String {
    store::submit(
        data,
        s,
        &s.profiles[0],
        source,
        &json!({"request_id":request,"text":"问题"}),
    )
    .unwrap()["task_id"]
        .as_str()
        .unwrap()
        .into()
}
fn generated(data: &std::path::Path, s: &Settings, id: &str, text: &str) {
    assert!(store::claim(data, id, s).unwrap());
    store::finish(data, id, Ok((text.into(), json!({"total_tokens":7})))).unwrap();
}
fn mock(
    status: &str,
    body: &str,
    delay: Duration,
) -> (
    String,
    std::sync::mpsc::Receiver<String>,
    std::thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let status = status.to_owned();
    let body = body.to_owned();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = vec![];
        let mut buf = [0; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            if n == 0 {
                break;
            };
            bytes.extend_from_slice(&buf[..n]);
            if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let h = String::from_utf8_lossy(&bytes[..i]);
                let len = h
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= i + 4 + len {
                    break;
                }
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
        std::thread::sleep(delay);
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    });
    (url, rx, handle)
}
#[test]
fn credentials_are_write_only_and_blank_save_preserves_key() {
    let d = tempfile::tempdir().unwrap();
    let mut c = app_config::Config::default(d.path().join("config.json"));
    let mut value = serde_json::to_value(settings()).unwrap();
    value["profiles"][0]["api_key"] = json!("fixture-secret");
    let (s, k) = config::prepare_save(&c, &value).unwrap();
    c.raw["ai"] = serde_json::to_value(s).unwrap();
    app_config::atomic(&d.path().join("data/ai-keys.json"), &k).unwrap();
    let mut public = config::public(&c).unwrap();
    assert!(public["profiles"][0]["has_key"] == true);
    assert!(!public.to_string().contains("fixture-secret"));
    assert!(!c.raw.to_string().contains("fixture-secret"));
    public["profiles"][0]["api_key"] = json!("");
    let (_, kept) = config::prepare_save(&c, &public).unwrap();
    assert_eq!(kept["local"], "fixture-secret");
    public["profiles"][0]["clear_key"] = true.into();
    let (_, removed) = config::prepare_save(&c, &public).unwrap();
    assert!(removed["local"].is_null());
}
#[test]
fn endpoints_require_explicit_http_and_no_embedded_credentials() {
    let mut p = settings().profiles.remove(0);
    assert_eq!(config::endpoint(&p).unwrap().path(), "/v1/chat/completions");
    p.base_url = "https://model.example/compatible-mode/v1/chat/completions/".into();
    assert_eq!(
        config::endpoint(&p).unwrap().path(),
        "/compatible-mode/v1/chat/completions"
    );
    for url in [
        "file:///secret",
        "https://user:password@model.example/v1",
        "https://model.example/v1?key=secret",
        "https://model.example/v1#fragment",
    ] {
        p.base_url = url.into();
        assert!(config::endpoint(&p).is_err());
    }
    p.base_url = "http://127.0.0.1:11434/v1".into();
    p.allow_http = false;
    assert!(config::endpoint(&p).is_err());
}
#[test]
fn request_id_is_idempotent_and_conflicting_reuse_is_rejected() {
    let d = tempfile::tempdir().unwrap();
    let s = settings();
    let src = observed(d.path(), "bot", "好友", "wxid-a", "wxid-a", 1, false);
    let a = submit(d.path(), &s, Some(&src), "once");
    let b = submit(d.path(), &s, Some(&src), "once");
    assert_eq!(a, b);
    assert!(
        store::submit(
            d.path(),
            &s,
            &s.profiles[0],
            Some(&src),
            &json!({"request_id":"once","text":"不同"})
        )
        .is_err()
    );
}
#[test]
fn stable_identity_survives_rename_and_isolates_account_group_and_member() {
    let d = tempfile::tempdir().unwrap();
    let a = observed(d.path(), "bot", "旧群", "room@chatroom", "wxid-a", 1, true);
    let renamed = observed(d.path(), "bot", "新群", "room@chatroom", "wxid-a", 2, true);
    assert_eq!(a.session, renamed.session);
    for b in [
        observed(d.path(), "bot2", "旧群", "room@chatroom", "wxid-a", 3, true),
        observed(d.path(), "bot", "旧群", "room@chatroom", "wxid-b", 4, true),
        observed(d.path(), "bot", "旧群", "room2@chatroom", "wxid-a", 5, true),
    ] {
        assert_ne!(a.session, b.session);
    }
}
#[test]
fn context_only_commits_after_delivery_and_next_task_waits() {
    let d = tempfile::tempdir().unwrap();
    let s = settings();
    let src = observed(d.path(), "bot", "群", "room", "member", 1, true);
    let a = submit(d.path(), &s, Some(&src), "first");
    let b = submit(d.path(), &s, Some(&src), "second");
    generated(d.path(), &s, &a, "第一条回答");
    assert!(!store::claim(d.path(), &b, &s).unwrap());
    let input = store::input(d.path(), &b).unwrap();
    assert_eq!(store::messages(d.path(), &b, &input).unwrap().len(), 2);
    store::attach(d.path(), &a, 55).unwrap();
    store::delivery(d.path(), &a, "queued").unwrap();
    assert!(!store::claim(d.path(), &b, &s).unwrap());
    store::delivery(d.path(), &a, "sent").unwrap();
    assert!(store::claim(d.path(), &b, &s).unwrap());
    let messages = store::messages(d.path(), &b, &input).unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[2]["content"], "第一条回答");
    store::delivery(d.path(), &a, "sent").unwrap();
    assert_eq!(
        store::list(d.path(), 30).unwrap()["contexts"][0]["turns"],
        1
    );
}
#[test]
fn failed_delivery_and_context_clear_never_create_phantom_turns() {
    let d = tempfile::tempdir().unwrap();
    let s = settings();
    let src = observed(d.path(), "bot", "好友", "peer", "member", 1, false);
    let a = submit(d.path(), &s, Some(&src), "first");
    generated(d.path(), &s, &a, "未送达");
    store::attach(d.path(), &a, 55).unwrap();
    store::delivery(d.path(), &a, "uncertain").unwrap();
    assert_eq!(store::list(d.path(), 30).unwrap()["contexts"], json!([]));
    let b = submit(d.path(), &s, Some(&src), "second");
    generated(d.path(), &s, &b, "旧任务");
    store::attach(d.path(), &b, 56).unwrap();
    store::clear_context(d.path(), Some(&src.session)).unwrap();
    store::delivery(d.path(), &b, "sent").unwrap();
    assert_eq!(store::list(d.path(), 30).unwrap()["contexts"], json!([]));
}
#[test]
fn cancelled_tasks_release_order_and_do_not_accept_late_generation() {
    let d = tempfile::tempdir().unwrap();
    let s = settings();
    let src = observed(d.path(), "bot", "好友", "peer", "member", 1, false);
    let a = submit(d.path(), &s, Some(&src), "first");
    let b = submit(d.path(), &s, Some(&src), "second");
    assert!(store::claim(d.path(), &a, &s).unwrap());
    store::cancel(d.path(), &a).unwrap();
    store::finish(d.path(), &a, Ok(("迟到".into(), json!({})))).unwrap();
    assert_eq!(store::get(d.path(), &a).unwrap()["status"], "cancelled");
    assert!(store::claim(d.path(), &b, &s).unwrap());
}
#[test]
fn restart_preserves_queued_but_never_replays_interrupted_model_call() {
    let d = tempfile::tempdir().unwrap();
    let s = settings();
    let a = submit(d.path(), &s, None, "first");
    let b = submit(d.path(), &s, None, "second");
    // Chat recovery is exercised with a real incoming source, not GUI probes.
    let src = observed(d.path(), "bot", "好友", "peer", "member", 1, false);
    let c = submit(d.path(), &s, Some(&src), "chat");
    assert!(store::claim(d.path(), &c, &s).unwrap());
    store::recover(d.path()).unwrap();
    assert_eq!(store::get(d.path(), &c).unwrap()["status"], "uncertain");
    assert_eq!(store::get(d.path(), &a).unwrap()["status"], "queued");
    assert_eq!(store::get(d.path(), &b).unwrap()["status"], "queued");
    assert!(!store::claim(d.path(), &c, &s).unwrap());
}
#[test]
fn shared_rate_concurrency_and_daily_reservation_are_enforced() {
    let d = tempfile::tempdir().unwrap();
    let mut s = settings();
    s.concurrency = 1;
    s.requests_per_minute = 1;
    s.daily_token_budget = s.input_budget + s.max_output_tokens;
    let a = submit(d.path(), &s, None, "a");
    let b = submit(d.path(), &s, None, "b");
    assert!(store::claim(d.path(), &a, &s).unwrap());
    assert!(!store::claim(d.path(), &b, &s).unwrap());
    store::finish(d.path(), &a, Err(anyhow::anyhow!("fixture"))).unwrap();
    assert!(!store::claim(d.path(), &b, &s).unwrap());
    s.requests_per_minute = 600;
    assert!(!store::claim(d.path(), &b, &s).unwrap());
    assert_eq!(store::get(d.path(), &b).unwrap()["status"], "failed");
}
#[tokio::test]
async fn compatible_http_sends_correct_payload_and_only_visible_answer() {
    let body = r#"{"choices":[{"message":{"content":"可见回答","reasoning_content":"不要泄露思考"},"finish_reason":"stop"}],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}"#;
    let (url, rx, h) = mock("200 OK", body, Duration::ZERO);
    let mut p = settings().profiles.remove(0);
    p.base_url = url;
    p.token_parameter = "max_completion_tokens".into();
    let result = worker::complete(
        &p,
        "fixture-secret",
        vec![json!({"role":"user","content":"你好"})],
        128,
        Duration::from_secs(3),
    )
    .await
    .unwrap();
    assert_eq!(result.0, "可见回答");
    assert_eq!(result.1["total_tokens"], 7);
    let request = rx.recv().unwrap();
    assert!(request.starts_with("POST /v1/chat/completions "));
    assert!(
        request
            .to_lowercase()
            .contains("authorization: bearer fixture-secret")
    );
    let payload: Value = serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(payload["max_completion_tokens"], 128);
    assert!(payload["max_tokens"].is_null());
    assert!(payload["temperature"].is_null());
    h.join().unwrap();
}
#[tokio::test]
async fn network_failure_never_echoes_provider_body_or_credentials() {
    let (url, _, h) = mock(
        "429 Too Many Requests",
        r#"{"error":"fixture-secret private prompt"}"#,
        Duration::ZERO,
    );
    let mut p = settings().profiles.remove(0);
    p.base_url = url;
    let error = worker::complete(&p, "fixture-secret", vec![], 128, Duration::from_secs(3))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("429"));
    assert!(!error.contains("fixture-secret"));
    assert!(!error.contains("private prompt"));
    h.join().unwrap();
}
#[tokio::test]
async fn slow_provider_respects_total_http_deadline() {
    let (url, _, h) = mock(
        "200 OK",
        r#"{"choices":[{"message":{"content":"太迟"}}]}"#,
        Duration::from_millis(180),
    );
    let mut p = settings().profiles.remove(0);
    p.base_url = url;
    let error = worker::complete(&p, "", vec![], 128, Duration::from_millis(50))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("超时"));
    h.join().unwrap();
}
#[test]
fn truncation_reasoning_only_and_tools_are_not_sent_as_answers() {
    for v in [
        json!({"choices":[{"message":{"content":"残缺"},"finish_reason":"length"}]}),
        json!({"choices":[{"message":{"reasoning_content":"思考"}}]}),
        json!({"choices":[{"message":{"content":"动作","tool_calls":[{"name":"send"}]}}]}),
    ] {
        assert!(worker::decode(v.to_string().as_bytes()).is_err());
    }
}
#[test]
fn redreply_extensions_keep_echo_and_native_queue_idempotency() {
    let d = tempfile::tempdir().unwrap();
    let mut c = app_config::Config::default(d.path().join("config.json"));
    c.raw["ai"] = serde_json::to_value(settings()).unwrap();
    c.raw["whitelist"] = json!(["好友"]);
    c.raw["wechat_binding"] = json!({"pid":1,"created_at":1.,"hwnd":1});
    let (shared, _) = Shared::new(c);
    *shared.account_wxid.write().unwrap() = Some("bot".into());
    *shared.bound_root.write().unwrap() = Some(d.path().join("account"));
    let src = observed(&shared.data, "bot", "好友", "peer", "member", 10, false);
    let p = json!({"source_message_id":10,"user_id":123,"text":"带双引号\"与换行\n[CQ:image,file=evil]"});
    let a = crate::bridge::api(
        &shared,
        json!({"action":"submit_ai_task","echo":"keep","params":p}),
        0,
    );
    assert_eq!(a["retcode"], 0);
    assert_eq!(a["echo"], "keep");
    let id = a["data"]["task_id"].as_str().unwrap();
    generated(
        &shared.data,
        &settings(),
        id,
        "[CQ:image,file=evil] 该消息30秒后撤回",
    );
    let send = json!({"action":"send_ai_task","params":{"task_id":id}});
    let first = crate::bridge::api(&shared, send.clone(), 0);
    let second = crate::bridge::api(&shared, send, 0);
    assert_eq!(first["retcode"], 0);
    assert_eq!(first["data"]["message_id"], second["data"]["message_id"]);
    // Simulate a crash after the outbox commit and before the AI-store commit.
    // Recovery must discover the existing output before allowing cancellation.
    store::open(&shared.data)
        .unwrap()
        .execute(
            "UPDATE tasks SET status='generated',message_id=NULL WHERE id=?1",
            [id],
        )
        .unwrap();
    assert!(store::cancel(&shared.data, id).is_err());
    let recovered = store::get(&shared.data, id).unwrap();
    assert_eq!(recovered["status"], "awaiting_delivery");
    assert_eq!(recovered["message_id"], first["data"]["message_id"]);
    let db = rusqlite::Connection::open(shared.data.join("outbox.sqlite3")).unwrap();
    let (request, delay): (String, Option<f64>) = db
        .query_row("SELECT request,recall_delay FROM jobs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    let request: Value = serde_json::from_str(&request).unwrap();
    assert_eq!(request["params"]["auto_escape"], true);
    assert_eq!(request["params"]["message"][0]["type"], "text");
    assert_eq!(delay, None);
    let bad = crate::bridge::api(
        &shared,
        json!({"action":"submit_ai_task","params":{"source_message_id":10,"user_id":999,"text":"冒用"}}),
        0,
    );
    assert_ne!(bad["retcode"], 0);
    assert_eq!(src.name, "好友");
    let uncertain = submit(
        &shared.data,
        &settings(),
        Some(&src),
        "interrupted-without-answer",
    );
    store::open(&shared.data)
        .unwrap()
        .execute(
            "UPDATE tasks SET status='uncertain' WHERE id=?1",
            [&uncertain],
        )
        .unwrap();
    let rejected = crate::bridge::api(
        &shared,
        json!({"action":"send_ai_task","params":{"task_id":uncertain}}),
        0,
    );
    assert_ne!(
        rejected["retcode"], 0,
        "an interrupted model call is not a sendable answer"
    );
}

#[test]
fn thirty_two_concurrent_retries_accept_exactly_one_task() {
    let d = tempfile::tempdir().unwrap();
    let s = settings();
    let source = observed(d.path(), "bot", "好友", "peer", "member", 100, false);
    let mut jobs = vec![];
    for _ in 0..32 {
        let data = d.path().to_path_buf();
        let s = s.clone();
        let source = source.clone();
        jobs.push(std::thread::spawn(move || {
            submit(&data, &s, Some(&source), "concurrent-same-id")
        }));
    }
    let ids = jobs
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(ids.len(), 1);
    let n: i64 = store::open(d.path())
        .unwrap()
        .query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
}

#[test]
fn slow_generation_does_not_block_control_actions_and_cancel_stays_cancelled() {
    use std::sync::atomic::Ordering;
    let (url, received, model) = mock(
        "200 OK",
        r#"{"choices":[{"message":{"content":"迟到的模型回答"},"finish_reason":"stop"}]}"#,
        Duration::from_millis(500),
    );
    let d = tempfile::tempdir().unwrap();
    let mut s = settings();
    s.profiles[0].base_url = url;
    let mut c = app_config::Config::default(d.path().join("config.json"));
    c.raw["ai"] = serde_json::to_value(&s).unwrap();
    c.raw["whitelist"] = json!(["好友"]);
    let (shared, _) = Shared::new(c);
    *shared.account_wxid.write().unwrap() = Some("bot".into());
    observed(&shared.data, "bot", "好友", "peer", "member", 1, false);
    let started = std::time::Instant::now();
    let task = crate::bridge::api(
        &shared,
        json!({"action":"submit_ai_task","params":{"source_message_id":1,"user_id":123,"text":"测试"}}),
        0,
    );
    assert_eq!(task["retcode"], 0);
    assert!(started.elapsed() < Duration::from_millis(500));
    let id = task["data"]["task_id"].as_str().unwrap().to_owned();
    let worker_shared = shared.clone();
    let worker = std::thread::spawn(move || super::run(worker_shared));
    received.recv_timeout(Duration::from_secs(3)).unwrap();
    let started = std::time::Instant::now();
    let health = crate::bridge::api(
        &shared,
        json!({"action":"get_version_info","echo":"health"}),
        0,
    );
    assert_eq!(health["retcode"], 0);
    assert!(started.elapsed() < Duration::from_millis(200));
    let cancelled = crate::bridge::api(
        &shared,
        json!({"action":"cancel_ai_task","params":{"task_id":id}}),
        0,
    );
    assert_eq!(cancelled["data"]["status"], "cancelled");
    model.join().unwrap();
    shared.stop.store(true, Ordering::SeqCst);
    worker.join().unwrap().unwrap();
    assert_eq!(
        store::get(&shared.data, &id).unwrap()["status"],
        "cancelled"
    );
    assert!(!shared.data.join("outbox.sqlite3").exists());
}

#[test]
fn idle_ai_queries_use_metadata_indexes_not_historical_payloads() {
    let d = tempfile::tempdir().unwrap();
    let db = store::open(d.path()).unwrap();
    for sql in [
        "SELECT id FROM tasks WHERE status='queued' AND kind='chat' ORDER BY seq LIMIT 1000",
        "SELECT coalesce(sum(reserved),0) FROM tasks WHERE started>=0",
        "SELECT rowid FROM tasks WHERE status='generated' AND kind='chat' AND expires<0",
    ] {
        let mut q = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let plan = q
            .query_map([], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
            .join(" ");
        assert!(plan.contains("INDEX"), "{plan}");
        assert!(!plan.contains("SCAN tasks"), "{plan}");
    }
}
