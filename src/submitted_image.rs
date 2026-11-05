//! Keep an image in its original output even when WeChat's server ACK is late.
//! A match requires account, target, invocation time and the exact image MD5.
use crate::{
    actions::{Record, Tracked},
    bridge::Job,
    config::{self, Binding},
    database::Message,
    service::Shared,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone, Serialize, Deserialize)]
pub struct Submission {
    pub output_id: u64,
    pub name: String,
    pub binding: Binding,
    pub sender: String,
    pub submitted_at: f64,
    pub md5: String,
}
fn same_binding(a: &Binding, b: &Binding) -> bool {
    a.pid == b.pid && a.hwnd == b.hwnd && (a.created_at - b.created_at).abs() < 0.01
}
pub fn matches_image(row: &Message, fingerprint: &str) -> bool {
    if row.kind != 3 || row.content.len() > 2 * 1024 * 1024 {
        return false;
    }
    let mut reader = quick_xml::Reader::from_str(&row.content);
    for _ in 0..2048 {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e) | quick_xml::events::Event::Empty(e))
                if e.name().as_ref() == b"img" =>
            {
                return e.attributes().filter_map(|a| a.ok()).any(|a| {
                    a.key.as_ref() == b"md5" && a.value.eq_ignore_ascii_case(fingerprint.as_bytes())
                });
            }
            Ok(quick_xml::events::Event::Eof) | Err(_) => break,
            _ => (),
        }
    }
    false
}
/// Call immediately before submitting the Send button, after all safety checks.
/// Self-chat staging is a different target and must never become the output.
pub fn register(shared: &Shared, job: &Job, target: &str, raw: &[u8]) -> Result<()> {
    if target != job.name || !matches!(&job.operation, crate::actions::Operation::Send) {
        return Ok(());
    }
    let binding = shared.config.read().unwrap().binding()?;
    let sender = shared
        .account_wxid
        .read()
        .unwrap()
        .clone()
        .context("本账号身份尚未核验，未提交图片")?;
    let submission = Submission {
        output_id: job.id,
        name: target.into(),
        binding: binding.clone(),
        sender,
        submitted_at: config::now(),
        md5: crate::attachments::image_fingerprint(raw)?,
    };
    {
        let mut ledger = shared.ledger.lock().unwrap();
        let record = ledger.sent.entry(job.id).or_insert_with(|| Record {
            name: job.name.clone(),
            group: job.group,
            binding,
            messages: vec![],
            nodes: None,
            recalled: false,
        });
        record.recalled = false;
        ledger.image_submissions.push(submission);
    }
    crate::actions::save(shared)
}
pub fn observe(shared: &Shared, name: &str, row: &Message) -> Result<()> {
    if row.kind != 3 {
        return Ok(());
    }
    if shared.account_wxid.read().unwrap().as_deref() != Some(row.sender.as_str()) {
        return Ok(());
    }
    let binding = shared.config.read().unwrap().binding()?;
    let mut changed = None;
    {
        let mut ledger = shared.ledger.lock().unwrap();
        // A known SQLite identity keeps ownership when a delayed ACK arrives.
        let mut assigned = false;
        for (&id, record) in &mut ledger.sent {
            if record.name != name {
                continue;
            }
            for part in &mut record.messages {
                if part.row.key() != row.key() {
                    continue;
                }
                assigned = true;
                if same_binding(&record.binding, &binding)
                    && part.row.server_id == 0
                    && row.server_id > 0
                    && part.row.sender == row.sender
                    && part.row.sort_seq == row.sort_seq
                {
                    part.row = row.clone();
                    part.sent_at = row.create_time as f64;
                    changed = Some(id);
                }
            }
        }
        if !assigned {
            let candidates = ledger
                .image_submissions
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    s.name == name
                        && s.sender == row.sender
                        && same_binding(&s.binding, &binding)
                        && row.create_time as f64 >= s.submitted_at.floor() - 1.
                        && row.create_time as f64 <= s.submitted_at.floor() + 5.
                        && matches_image(row, &s.md5)
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            if candidates.len() == 1 {
                let s = ledger.image_submissions.remove(candidates[0]);
                let record = ledger
                    .sent
                    .get_mut(&s.output_id)
                    .context("已提交图片的输出记录不存在")?;
                record.messages.push(Tracked {
                    row: row.clone(),
                    runtime_id: vec![],
                    sent_at: if row.create_time > 0 {
                        row.create_time as f64
                    } else {
                        s.submitted_at
                    },
                    native_cgi: None,
                });
                record.recalled = false;
                changed = Some(s.output_id);
            }
        }
    }
    if let Some(id) = changed {
        crate::actions::save(shared)?;
        shared.log(&format!(
            "图片回执已关联原输出：消息ID={id}，本地消息={}，服务端确认={}",
            row.local_id,
            row.server_id > 0
        ));
    }
    Ok(())
}

pub fn pending_since(shared: &Shared, id: u64) -> Option<f64> {
    let ledger = shared.ledger.lock().unwrap();
    ledger
        .image_submissions
        .iter()
        .filter(|s| s.output_id == id)
        .map(|s| s.submitted_at)
        .chain(
            ledger
                .sent
                .get(&id)
                .into_iter()
                .flat_map(|r| r.messages.iter())
                .filter(|m| m.row.kind == 3 && m.row.server_id == 0)
                .map(|m| m.sent_at),
        )
        .min_by(f64::total_cmp)
}
pub fn pending_rows(shared: &Shared, name: &str) -> Vec<Message> {
    shared
        .ledger
        .lock()
        .unwrap()
        .sent
        .values()
        .filter(|r| r.name == name)
        .flat_map(|r| r.messages.iter())
        .filter(|m| m.row.kind == 3 && m.row.server_id == 0 && config::now() - m.sent_at < 120.)
        .map(|m| m.row.clone())
        .collect()
}
pub fn assigned_to(shared: &Shared, id: u64, row: &Message) -> bool {
    shared
        .ledger
        .lock()
        .unwrap()
        .sent
        .get(&id)
        .is_some_and(|r| {
            r.messages
                .iter()
                .any(|m| m.row.key() == row.key() && m.row.sender == row.sender)
        })
}
pub fn wait(
    shared: &Shared,
    name: &str,
    fingerprint: &str,
    before: u64,
    submitted_at: f64,
    output: Option<u64>,
    until: Instant,
) -> bool {
    let mut own = shared.own.lock().unwrap();
    loop {
        if own.rows.get(name).is_some_and(|rows| {
            rows.iter().any(|r| {
                r.serial > before
                    && r.message.server_id > 0
                    && r.message.create_time as f64 >= submitted_at.floor() - 1.
                    && r.message.create_time as f64 <= submitted_at.floor() + 5.
                    && matches_image(&r.message, fingerprint)
                    && output.is_none_or(|id| assigned_to(shared, id, &r.message))
            })
        }) {
            return true;
        }
        if Instant::now() >= until || shared.stop.load(std::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        own = shared
            .changed
            .wait_timeout(own, Duration::from_millis(50))
            .unwrap()
            .0;
    }
}
#[derive(Debug)]
pub struct AwaitingConfirmation;
impl std::fmt::Display for AwaitingConfirmation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("已提交图片尚待微信确认，撤回将继续等待；未标记整条输出已撤回")
    }
}
impl std::error::Error for AwaitingConfirmation {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicU8};
    fn setup(path: &std::path::Path) -> (crate::config::Config, Arc<Shared>, Job, Vec<u8>) {
        let mut c = crate::config::Config::default(path.join("config.json"));
        c.raw["wechat_binding"] = serde_json::json!({"pid":1,"created_at":1.,"hwnd":1});
        c.raw["whitelist"] = serde_json::json!(["甲"]);
        let (shared, _) = Shared::new(c.clone());
        *shared.account_wxid.write().unwrap() = Some("wxid_self".into());
        let (reply, _) = std::sync::mpsc::sync_channel(1);
        let job = Job {
            name: "甲".into(),
            group: false,
            owner: 0,
            id: 101,
            parts: vec![],
            operation: crate::actions::Operation::Send,
            deadline: Instant::now() + Duration::from_secs(120),
            state: Arc::new(AtomicU8::new(1)),
            reply,
        };
        let mut raw = std::io::Cursor::new(vec![]);
        image::RgbImage::new(2, 2)
            .write_to(&mut raw, image::ImageFormat::Png)
            .unwrap();
        (c, shared, job, raw.into_inner())
    }
    #[test]
    fn late_image_ack_survives_restart_and_stays_in_its_original_output() {
        let d = tempfile::tempdir().unwrap();
        let (c, s, job, raw) = setup(d.path());
        register(&s, &job, "甲", &raw).unwrap();
        let at = pending_since(&s, job.id).unwrap();
        let mut row = Message {
            local_id: 42,
            server_id: 0,
            sort_seq: 99,
            create_time: at as i64,
            kind: 3,
            sender: "wxid_self".into(),
            content: format!(
                "<msg><img md5=\"{}\"/></msg>",
                crate::attachments::image_fingerprint(&raw).unwrap()
            ),
            shard: "m".into(),
        };
        s.observe("甲", &row);
        assert_eq!(
            s.ledger.lock().unwrap().sent[&101].messages.len(),
            1,
            "a submitted image must be tracked before the server ACK"
        );
        assert!(pending_since(&s, 101).is_some());
        drop(s);
        let (resumed, _) = Shared::new(c);
        *resumed.account_wxid.write().unwrap() = Some("wxid_self".into());
        assert_eq!(pending_rows(&resumed, "甲").len(), 1);
        row.server_id = 1234567890123456789;
        resumed.observe("甲", &row);
        assert_eq!(
            resumed.ledger.lock().unwrap().sent[&101].messages[0]
                .row
                .server_id,
            row.server_id
        );
        assert!(pending_since(&resumed, 101).is_none());
        assert!(!resumed.ledger.lock().unwrap().sent[&101].recalled);
        let tracked = resumed.ledger.lock().unwrap().sent[&101].messages[0].clone();
        crate::actions::append_sent(&resumed, &job, &job_binding(&resumed), tracked).unwrap();
        assert_eq!(
            resumed.ledger.lock().unwrap().sent[&101].messages.len(),
            1,
            "normal sender tracking must not duplicate the observer receipt"
        );
    }
    fn job_binding(s: &Shared) -> Binding {
        s.config.read().unwrap().binding().unwrap()
    }
    #[test]
    fn different_sender_target_time_binding_and_ambiguous_images_are_not_assigned() {
        let d = tempfile::tempdir().unwrap();
        let (_, s, mut job, raw) = setup(d.path());
        register(&s, &job, "甲", &raw).unwrap();
        let at = pending_since(&s, 101).unwrap();
        let row = Message {
            local_id: 42,
            server_id: 42,
            sort_seq: 99,
            create_time: at as i64,
            kind: 3,
            sender: "wxid_self".into(),
            content: format!(
                "<msg><img md5=\"{}\"/></msg>",
                crate::attachments::image_fingerprint(&raw).unwrap()
            ),
            shard: "m".into(),
        };
        s.observe("乙", &row);
        let mut wrong = row.clone();
        wrong.sender = "wxid_other".into();
        s.observe("甲", &wrong);
        wrong = row.clone();
        wrong.create_time -= 10;
        s.observe("甲", &wrong);
        wrong = row.clone();
        wrong.content = "<msg><img md5=\"00000000000000000000000000000000\"/></msg>".into();
        s.observe("甲", &wrong);
        s.config.write().unwrap().raw["wechat_binding"]["pid"] = 2.into();
        s.observe("甲", &row);
        s.config.write().unwrap().raw["wechat_binding"]["pid"] = 1.into();
        assert!(s.ledger.lock().unwrap().sent[&101].messages.is_empty());
        job.id = 102;
        register(&s, &job, "甲", &raw).unwrap();
        s.observe("甲", &row);
        let l = s.ledger.lock().unwrap();
        assert!(l.sent[&101].messages.is_empty() && l.sent[&102].messages.is_empty());
        assert_eq!(
            l.image_submissions.len(),
            2,
            "ambiguous outcomes must stay pending"
        );
    }
    #[test]
    fn image_wait_tolerates_transient_not_ready_and_uses_the_exact_output_receipt() {
        let d = tempfile::tempdir().unwrap();
        let (_, s, job, raw) = setup(d.path());
        register(&s, &job, "甲", &raw).unwrap();
        let at = pending_since(&s, 101).unwrap();
        let hash = crate::attachments::image_fingerprint(&raw).unwrap();
        let row = Message {
            local_id: 42,
            server_id: 123,
            sort_seq: 99,
            create_time: at as i64,
            kind: 3,
            sender: "wxid_self".into(),
            content: format!("<msg><img md5=\"{hash}\"/></msg>"),
            shard: "m".into(),
        };
        let before = s.checkpoint();
        let actor = s.clone();
        let receiver = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            actor.observe("甲", &row);
        });
        assert!(!s.ready.load(std::sync::atomic::Ordering::SeqCst));
        assert!(wait(
            &s,
            "甲",
            &hash,
            before,
            at,
            Some(101),
            Instant::now() + Duration::from_secs(1)
        ));
        assert!(!wait(
            &s,
            "甲",
            &hash,
            before,
            at,
            Some(102),
            Instant::now() + Duration::from_millis(1)
        ));
        receiver.join().unwrap();
    }
}
