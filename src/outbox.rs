//! Durable trigger receipts. One trigger owns every output and every physical
//! WeChat message it produces, so a single recall covers the whole response.
//! Only unstarted work is replayed automatically after a process interruption.
use crate::{
    config::{self, Config},
    service::Shared,
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

pub struct Store {
    db: Connection,
}
#[derive(Clone)]
pub struct Entry {
    pub id: u64,
    pub batch: String,
    pub account: String,
    pub name: String,
    pub group: bool,
    pub request: Value,
}
pub fn enabled(c: &Config) -> bool {
    c.raw["durable_queue"].as_bool().unwrap_or(true)
}
impl Store {
    fn open(data: &Path) -> Result<Self> {
        std::fs::create_dir_all(data)?;
        let db = Connection::open(data.join("outbox.sqlite3"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS batches(batch_id TEXT PRIMARY KEY,account TEXT NOT NULL,name TEXT NOT NULL,is_group INTEGER NOT NULL,source TEXT NOT NULL,delay REAL,recall_requested INTEGER NOT NULL DEFAULT 0,status TEXT NOT NULL DEFAULT 'active',error TEXT NOT NULL DEFAULT '');
          CREATE TABLE IF NOT EXISTS jobs(id INTEGER PRIMARY KEY,batch_id TEXT NOT NULL,account TEXT NOT NULL,name TEXT NOT NULL,is_group INTEGER NOT NULL,request TEXT NOT NULL,status TEXT NOT NULL,enqueued REAL NOT NULL,started REAL,finished REAL,error TEXT NOT NULL DEFAULT '');
          CREATE INDEX IF NOT EXISTS jobs_pending ON jobs(status,id);
          CREATE INDEX IF NOT EXISTS jobs_batch ON jobs(batch_id,id);
          CREATE TABLE IF NOT EXISTS friend_requests(request_key TEXT PRIMARY KEY,status TEXT NOT NULL,error TEXT NOT NULL DEFAULT '',updated REAL NOT NULL);
          CREATE TABLE IF NOT EXISTS friend_control(name TEXT PRIMARY KEY,value TEXT NOT NULL);")?;
        let present: i64 = db.query_row(
            "SELECT count(*) FROM pragma_table_info('batches') WHERE name='workflow_until'",
            [],
            |r| r.get(0),
        )?;
        if present == 0 {
            db.execute("ALTER TABLE batches ADD COLUMN workflow_until REAL", [])?;
        }
        for (name, sql) in [
            (
                "recall_delay",
                "ALTER TABLE jobs ADD COLUMN recall_delay REAL",
            ),
            (
                "recall_requested",
                "ALTER TABLE jobs ADD COLUMN recall_requested INTEGER NOT NULL DEFAULT 0",
            ),
            (
                "recall_status",
                "ALTER TABLE jobs ADD COLUMN recall_status TEXT NOT NULL DEFAULT ''",
            ),
            (
                "recall_error",
                "ALTER TABLE jobs ADD COLUMN recall_error TEXT NOT NULL DEFAULT ''",
            ),
        ] {
            let present: i64 = db.query_row(
                "SELECT count(*) FROM pragma_table_info('jobs') WHERE name=?1",
                [name],
                |r| r.get(0),
            )?;
            if present == 0 {
                db.execute(sql, [])?;
            }
        }
        Ok(Self { db })
    }
}
fn store<T>(shared: &Shared, f: impl FnOnce(&mut Store) -> Result<T>) -> Result<T> {
    let mut guard = shared.outbox.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Store::open(&shared.data)?);
    }
    f(guard.as_mut().unwrap())
}
pub fn account(shared: &Shared) -> Result<String> {
    if let Some(root) = shared.bound_root.read().unwrap().as_ref() {
        return Ok(hex::encode(Sha256::digest(
            root.to_string_lossy()
                .trim_start_matches(r"\\?\")
                .to_lowercase()
                .as_bytes(),
        ))[..24]
            .to_owned());
    }
    let ids = std::fs::read_dir(shared.data.join("db"))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir() && e.file_name().to_string_lossy().len() == 24)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if ids.len() != 1 {
        bail!("尚未验证唯一微信账号，无法持久化发送目标");
    }
    Ok(ids[0].clone())
}
fn source(params: &Value) -> Result<Option<String>> {
    for key in ["wechat_batch_id", "source_message_id", "trigger_id"] {
        if let Some(v) = params.get(key).filter(|v| !v.is_null()) {
            let s = v
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string());
            if s.is_empty() || s.len() > 256 {
                bail!("批次来源标识无效");
            }
            return Ok(Some(s));
        }
    }
    fn replies(v: &Value, out: &mut std::collections::BTreeSet<String>) {
        if v["type"] == "reply" {
            if let Some(id) = v["data"].get("id") {
                out.insert(
                    id.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| id.to_string()),
                );
            }
        }
        match v {
            Value::Array(a) => {
                for v in a {
                    replies(v, out)
                }
            }
            Value::Object(m) => {
                for v in m.values() {
                    replies(v, out)
                }
            }
            _ => (),
        }
    }
    let mut ids = std::collections::BTreeSet::new();
    if let Some(text) = params["message"].as_str() {
        replies(&json!(crate::delivery::parse_cq(text)), &mut ids)
    } else {
        replies(&params["message"], &mut ids);
    }
    if ids.len() > 1 {
        bail!("同一回复包含多个来源消息，无法确定触发批次");
    }
    Ok(ids.into_iter().next())
}
fn output_batch(account: &str, group: bool, name: &str, id: u64) -> String {
    hex::encode(Sha256::digest(format!(
        "output:{account}:{group}:{name}:{id}"
    )))
}
/// One durable receipt for every output of the same inbound trigger. Target and
/// account remain part of the key, so a trigger can never recall another chat.
fn trigger_batch(account: &str, group: bool, name: &str, source: &str) -> String {
    hex::encode(Sha256::digest(format!(
        "trigger:{account}:{group}:{name}:{source}"
    )))
}
fn auto_recall_delay(message: &Value) -> Option<f64> {
    let segments = if let Some(text) = message.as_str() {
        crate::delivery::parse_cq(text)
    } else {
        message.as_array()?.clone()
    };
    let mut text = String::new();
    for segment in segments {
        if segment["type"] == "text"
            && let Some(value) = segment["data"]["text"].as_str()
        {
            text.push_str(value);
            text.push('\n');
        }
    }
    let re = regex::Regex::new(r"(\d+)\s*(分钟|分|秒)\s*后撤回").ok()?;
    let mut delay = 0f64;
    for capture in re.captures_iter(&text) {
        let amount = capture.get(1)?.as_str().parse::<f64>().ok()?;
        let seconds = if capture.get(2)?.as_str() == "秒" {
            amount
        } else {
            amount * 60.
        };
        // Reserve time for native menu operations before WeChat's 120s limit.
        let candidate = if seconds >= 60. {
            seconds - 20.
        } else {
            seconds - 5.
        };
        delay = delay.max(candidate);
    }
    if delay <= 0. && text.contains("施法中") {
        delay = 25.;
    }
    (delay > 0.).then(|| delay.clamp(3., 100.))
}

pub fn enqueue(shared: &Shared, c: &Config, request: &Value) -> Result<Value> {
    c.binding()?;
    let action = request["action"].as_str().context("Missing action")?;
    let p = &request["params"];
    let group = action == "send_group_msg"
        || (action == "send_group_forward_msg" && number(&p["group_id"]) > 0)
        || (action == "send_msg" && (p["message_type"] == "group" || number(&p["group_id"]) > 0));
    let dest = number(&p[if group { "group_id" } else { "user_id" }]);
    let (name, _) = c
        .scope()
        .into_iter()
        .find(|(n, g)| {
            *g == group
                && if group {
                    config::group_id(n)
                } else {
                    config::contact_id(n)
                } == dest
        })
        .context("Unknown or non-whitelisted destination")?;
    if action == "send_private_msg" && number(&p["group_id"]) > 0 {
        bail!("临时群私聊不受支持");
    }
    let forward = action.ends_with("forward_msg");
    if forward {
        crate::actions::nodes(
            p.get("messages")
                .or_else(|| p.get("message"))
                .context("Missing nodes")?,
            shared,
        )?;
    } else if !p["message"].is_string() && !p["message"].is_array() {
        bail!("message需要文字或消息段数组");
    }
    let raw = json!({"action":action,"params":p}).to_string();
    if raw.len() > 16 * 1024 * 1024 {
        bail!("待发送内容超过16MB");
    }
    let identity = account(shared)?;
    let origin = source(p)?;
    let auto_delay = auto_recall_delay(&p["message"]);
    let receipt = store(shared, |s| {
        let tx = s.db.transaction()?;
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM jobs WHERE status IN ('queued','preparing','sending')",
            [],
            |r| r.get(0),
        )?;
        let bytes: i64 = tx.query_row(
            "SELECT COALESCE(sum(length(request)),0) FROM jobs",
            [],
            |r| r.get(0),
        )?;
        if count >= 5000 || bytes + raw.len() as i64 > 256 * 1024 * 1024 {
            bail!("持久化队列已达5000项或256MB上限，未接受请求");
        }
        let last: u64 = tx.query_row("SELECT COALESCE(max(id),0) FROM jobs", [], |r| r.get(0))?;
        let id = ((config::now() * 1000.) as u64).max(last + 1);
        let source = origin
            .clone()
            .unwrap_or_else(|| format!("independent:{id}"));
        let batch = origin
            .as_deref()
            .map(|s| trigger_batch(&identity, group, &name, s))
            .unwrap_or_else(|| output_batch(&identity, group, &name, id));
        tx.execute("INSERT OR IGNORE INTO batches(batch_id,account,name,is_group,source) VALUES(?1,?2,?3,?4,?5)",params![batch,identity,name,group,source])?;
        tx.execute("INSERT INTO jobs(id,batch_id,account,name,is_group,request,status,enqueued,recall_delay,recall_status) VALUES(?1,?2,?3,?4,?5,?6,'queued',?7,?8,?9)",params![id,batch,identity,name,group,raw,config::now(),auto_delay,if auto_delay.is_some(){"scheduled"}else{""}])?;
        if let Some(delay) = auto_delay
            && delay >= 60.
        {
            // The final "N分钟后撤回" marker covers the whole trigger: apply
            // the same early deadline to outputs that carry no marker of their
            // own, including @ mentions and progress notices.
            tx.execute("UPDATE jobs SET recall_delay=CASE WHEN recall_delay IS NULL THEN ?1 ELSE min(recall_delay,?1) END,recall_status=CASE WHEN recall_status='' THEN 'scheduled' ELSE recall_status END WHERE batch_id=?2 AND id<>?3 AND status IN ('queued','sent','uncertain')",params![delay,batch,id])?;
        }
        tx.commit()?;
        Ok(json!({
            "message_id":id,
            "batch_id":batch,
            "trigger_id":source,
            "recall_scope":"output",
            "recall_delay":auto_delay,
            "delivery_status":"queued",
            "confirmed_sent":false
        }))
    })?;
    shared.event("runtime",&json!({"text":format!("已持久化入队：{}，消息ID={}；尚未确认微信发出",name,receipt["message_id"]),"event":"durable_queued","receipt":receipt,"session":name}));
    Ok(receipt)
}
fn number(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
pub fn workflow(shared: &Shared, c: &Config, p: &Value, open: bool) -> Result<Value> {
    let group = number(&p["group_id"]) > 0;
    let dest = number(&p[if group { "group_id" } else { "user_id" }]);
    let (name, _) = c
        .scope()
        .into_iter()
        .find(|(n, g)| {
            *g == group
                && (if group {
                    config::group_id(n)
                } else {
                    config::contact_id(n)
                }) == dest
        })
        .context("Unknown or non-whitelisted workflow destination")?;
    let identity = account(shared)?;
    let source = source(p)?.context("回复流程需要明确的来源消息ID")?;
    let batch = trigger_batch(&identity, group, &name, &source);
    store(shared, |s| {
        let tx = s.db.transaction()?;
        if open {
            tx.execute("INSERT OR IGNORE INTO batches(batch_id,account,name,is_group,source) VALUES(?1,?2,?3,?4,?5)",params![batch,identity,name,group,source])?;
        }
        let changed=tx.execute("UPDATE batches SET workflow_until=?1 WHERE batch_id=?2 AND status='active' AND recall_requested=0",params![if open{Some(config::now()+300.)}else{None},batch])?;
        if changed == 0 {
            bail!("回复流程对应的批次不存在或已开始撤回");
        }
        tx.commit()?;
        Ok(())
    })?;
    shared.log(&format!(
        "查询流程追踪{}：目标={}，标识={batch}；只影响本次触发的统一撤回范围",
        if open { "开始" } else { "结束" },
        name
    ));
    Ok(
        json!({"batch_id":batch,"workflow_open":open,"tracking_ttl_seconds":300,"recall_scope":"trigger"}),
    )
}
pub fn friend_status(shared: &Shared, key: &str) -> Result<Option<(String, f64)>> {
    store(shared, |s| {
        Ok(s.db
            .query_row(
                "SELECT status,updated FROM friend_requests WHERE request_key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    })
}
pub fn friend_note(shared: &Shared, key: &str, status: &str, error: &str) -> Result<()> {
    store(shared, |s| {
        s.db.execute("INSERT INTO friend_requests(request_key,status,error,updated) VALUES(?1,?2,?3,?4) ON CONFLICT(request_key) DO UPDATE SET status=excluded.status,error=excluded.error,updated=excluded.updated",params![key,status,error,config::now()])?;
        Ok(())
    })
}
pub fn friend_pause_reason(shared: &Shared) -> Result<Option<String>> {
    store(shared, |s| {
        Ok(s.db
            .query_row(
                "SELECT value FROM friend_control WHERE name='security_pause'",
                [],
                |r| r.get(0),
            )
            .optional()?)
    })
}
pub fn pause_friends(shared: &Shared, reason: &str) -> Result<()> {
    store(shared, |s| {
        s.db.execute("INSERT INTO friend_control(name,value) VALUES('security_pause',?1) ON CONFLICT(name) DO UPDATE SET value=excluded.value",[reason])?;
        Ok(())
    })
}
pub fn resume_friends(data: &Path) -> Result<()> {
    let mut s = Store::open(data)?;
    let tx = s.db.transaction()?;
    tx.execute("DELETE FROM friend_control WHERE name='security_pause'", [])?;
    tx.execute(
        "DELETE FROM friend_requests WHERE status!='approved_only_chat'",
        [],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn status(shared: &Shared, id: u64) -> Result<Option<Value>> {
    store(shared, |s| {
        Ok(s.db.query_row("SELECT j.id,j.batch_id,j.name,j.is_group,j.status,j.enqueued,j.started,j.finished,j.error,b.source,j.recall_delay,j.recall_requested,j.recall_status,j.recall_error FROM jobs j JOIN batches b ON b.batch_id=j.batch_id WHERE j.id=?1",[id],|r|Ok(json!({"message_id":r.get::<_,u64>(0)?,"batch_id":r.get::<_,String>(1)?,"session":r.get::<_,String>(2)?,"group":r.get::<_,bool>(3)?,"delivery_status":r.get::<_,String>(4)?,"enqueued_at":r.get::<_,f64>(5)?,"started_at":r.get::<_,Option<f64>>(6)?,"finished_at":r.get::<_,Option<f64>>(7)?,"error":r.get::<_,String>(8)?,"trigger_id":r.get::<_,String>(9)?,"recall_delay":r.get::<_,Option<f64>>(10)?,"recall_requested":r.get::<_,bool>(11)?,"recall_status":r.get::<_,String>(12)?,"recall_error":r.get::<_,String>(13)?,"recall_scope":"output"}))).optional()?)
    })
}
pub fn recover(shared: &Shared) -> Result<()> {
    let (resumed, uncertain) = store(shared, |s| {
        let a = s.db.execute(
            "UPDATE jobs SET status='queued',started=NULL WHERE status='preparing'",
            [],
        )?;
        let b=s.db.execute("UPDATE jobs SET status='uncertain',error='程序在原生发送阶段中断；可能已发送，未自动重发',finished=?1 WHERE status='sending'",[config::now()])?;
        s.db.execute("UPDATE jobs SET recall_status='recall_failed',recall_error='撤回期间程序中断；请核对原消息' WHERE recall_status='recalling'",[])?;
        Ok((a, b))
    })?;
    shared.log(&format!(
        "持久化队列已恢复：安全待处理 {resumed} 项；发送状态待核对 {uncertain} 项"
    ));
    Ok(())
}
fn entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    let raw: String = r.get(5)?;
    Ok(Entry {
        id: r.get(0)?,
        batch: r.get(1)?,
        account: r.get(2)?,
        name: r.get(3)?,
        group: r.get(4)?,
        request: serde_json::from_str(&raw).unwrap_or(Value::Null),
    })
}
fn claim(shared: &Shared) -> Result<Option<Entry>> {
    let identity = account(shared)?;
    store(shared, |s| {
        let tx = s.db.transaction()?;
        let e=tx.query_row("SELECT j.id,j.batch_id,j.account,j.name,j.is_group,j.request FROM jobs j WHERE j.status='queued' AND j.account=?1 AND j.recall_requested=0 ORDER BY j.id LIMIT 1",[identity],entry).optional()?;
        if let Some(e) = &e {
            tx.execute(
                "UPDATE jobs SET status='preparing',started=?1 WHERE id=?2",
                params![config::now(), e.id],
            )?;
        }
        tx.commit()?;
        Ok(e)
    })
}
fn finish(shared: &Shared, id: u64, state: &str, error: &str) -> Result<()> {
    store(shared, |s| {
        s.db.execute("UPDATE jobs SET status=?1,error=?2,finished=?3,request=CASE WHEN ?1 IN ('sent','cancelled') THEN '{}' ELSE request END WHERE id=?4",params![state,error,config::now(),id])?;
        Ok(())
    })
}
pub fn request_recall(shared: &Shared, id: u64, delay: Option<f64>) -> Result<Option<Value>> {
    if delay.is_some_and(|d| !d.is_finite()) {
        bail!("撤回延迟不是有效数字");
    }
    let Some(item) = status(shared, id)? else {
        return Ok(None);
    };
    let batch = item["batch_id"].as_str().unwrap().to_owned();
    let seconds = delay
        .map(|ms| crate::actions::recall_delay(&shared.config.read().unwrap().raw, ms / 1000.));
    store(shared, |s| {
        let tx = s.db.transaction()?;
        if let Some(delay) = seconds {
            tx.execute("UPDATE jobs SET recall_delay=CASE WHEN recall_delay IS NULL THEN ?1 ELSE min(recall_delay,?1) END,recall_status=CASE WHEN recall_status='' THEN 'scheduled' ELSE recall_status END WHERE id=?2",params![delay,id])?;
        } else {
            tx.execute(
                "UPDATE jobs SET recall_requested=1,recall_status='requested' WHERE id=?1",
                [id],
            )?;
            tx.execute("UPDATE jobs SET status='cancelled',error='该次输出已请求撤回，取消尚未发送内容',finished=?1,recall_status='cancelled' WHERE id=?2 AND status IN ('queued','preparing')",params![config::now(),id])?;
        }
        tx.commit()?;
        Ok(())
    })?;
    shared.log(&format!(
        "输出撤回已登记：消息ID={id}，触发={batch}，延迟={seconds:?}秒；只撤回该次输出的全部拆分消息"
    ));
    Ok(Some(
        json!({"message_id":id,"batch_id":batch,"recall_status":"queued","delay_seconds":seconds,"recall_scope":"output","trigger_id":item["trigger_id"]}),
    ))
}
fn due_at(shared: &Shared, id: u64, delay: f64) -> Result<Option<f64>> {
    let ledger = shared.ledger.lock().unwrap();
    let Some(record) = ledger.sent.get(&id) else {
        return Ok(None);
    };
    let mut times = record
        .messages
        .iter()
        .map(|m| m.sent_at)
        .collect::<Vec<_>>();
    if times.is_empty() {
        return Ok(None);
    }
    times.sort_by(f64::total_cmp);
    let last = *times.last().unwrap();
    let cap = crate::actions::recall_limit(&shared.config.read().unwrap().raw);
    let hard = times
        .iter()
        .enumerate()
        .map(|(i, at)| at + cap - 3. * (i + 1) as f64)
        .min_by(f64::total_cmp)
        .unwrap();
    Ok(Some(
        (last + delay.min(cap) - (delay.min(cap) * 0.1).min(3.)).min(hard),
    ))
}
#[cfg(test)]
fn batch_deadline(times: &mut [f64], delay: f64, cap: f64, pending: bool) -> Option<f64> {
    times.sort_by(f64::total_cmp);
    let last = *times.last()?;
    let hard = times
        .iter()
        .enumerate()
        .map(|(i, at)| at + cap - 3. * (i + 1) as f64)
        .min_by(f64::total_cmp)?;
    Some(if pending {
        hard
    } else {
        (last + delay.min(cap) - (delay.min(cap) * 0.1).min(3.)).min(hard)
    })
}
pub fn cancelled(shared: &Shared, id: u64) -> Result<bool> {
    let value = store(shared, |s| {
        Ok(s.db.query_row("SELECT status,recall_delay,recall_requested,recall_status,enqueued FROM jobs WHERE id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<f64>>(1)?,r.get::<_,bool>(2)?,r.get::<_,String>(3)?,r.get::<_,f64>(4)?))).optional()?)
    })?;
    let Some((state, delay, requested, recall_status, enqueued)) = value else {
        return Ok(false);
    };
    if state == "cancelled"
        || requested
        || matches!(
            recall_status.as_str(),
            "requested" | "recalling" | "cancelled"
        )
    {
        return Ok(true);
    }
    if let Some(delay) = delay {
        if state == "queued" || state == "preparing" {
            let cap = crate::actions::recall_limit(&shared.config.read().unwrap().raw);
            return Ok(config::now() >= enqueued + delay.min(cap) - 3.);
        }
        return Ok(due_at(shared, id, delay)?.is_some_and(|due| due <= config::now()));
    }
    Ok(false)
}
pub fn summary(shared: &Shared) -> Result<Value> {
    store(shared, |s| {
        let mut counts = serde_json::Map::new();
        let mut q =
            s.db.prepare("SELECT status,count(*) FROM jobs GROUP BY status")?;
        for row in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (k, v) = row?;
            counts.insert(k, json!(v));
        }
        let reason: Option<String> =
            s.db.query_row(
                "SELECT value FROM friend_control WHERE name='security_pause'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        counts.insert("friend_paused_reason".into(), json!(reason));
        let open: i64 = s.db.query_row(
            "SELECT count(*) FROM batches WHERE status='active' AND workflow_until>?1",
            [config::now()],
            |r| r.get(0),
        )?;
        counts.insert("open_workflows".into(), json!(open));
        Ok(Value::Object(counts))
    })
}
pub fn process_one(shared: &Arc<Shared>, automation: &crate::delivery::Automation) -> Result<bool> {
    process_one_with(shared, |job| automation.send(shared, job))
}
pub(crate) fn process_one_with(
    shared: &Arc<Shared>,
    mut send: impl FnMut(&crate::bridge::Job) -> Result<()>,
) -> Result<bool> {
    let Some(e) = claim(shared)? else {
        return Ok(false);
    };
    let start = Instant::now();
    shared.log(&format!(
        "持久化任务开始：ID={}，目标={}，批次={}",
        e.id, e.name, e.batch
    ));
    let prepared = (|| -> Result<_> {
        if !shared.config.read().unwrap().allows(&e.name, e.group) {
            bail!("目标已移出白名单");
        }
        if e.account != account(shared)? {
            bail!("登录账号与持久化任务不一致");
        }
        let p = &e.request["params"];
        if e.request["action"]
            .as_str()
            .unwrap_or("")
            .ends_with("forward_msg")
        {
            let nodes = crate::actions::nodes(
                p.get("messages")
                    .or_else(|| p.get("message"))
                    .context("Missing forward nodes")?,
                shared,
            )?;
            let parts = crate::actions::forward_parts(&nodes, shared)?;
            Ok((parts, crate::actions::Operation::Forward(nodes)))
        } else {
            let parts = crate::delivery::prepare(&p["message"], p["auto_escape"] == true, shared)?;
            if parts
                .iter()
                .any(|p| matches!(p, crate::delivery::Part::Music(_)))
            {
                crate::music_backend::inspect(&shared.config.read().unwrap())?;
            }
            Ok((parts, crate::actions::Operation::Send))
        }
    })();
    let (parts, operation) = match prepared {
        Ok(p) => p,
        Err(error) => {
            finish(shared, e.id, "failed", &error.to_string())?;
            shared.log(&format!("持久化任务准备失败：ID={}，{error}", e.id));
            return Ok(true);
        }
    };
    if cancelled(shared, e.id)? {
        finish(shared, e.id, "cancelled", "该次输出已撤回，未发送")?;
        return Ok(true);
    }
    if shared.stop.load(Ordering::SeqCst) {
        finish(shared, e.id, "queued", "")?;
        return Ok(true);
    }
    let started = store(shared, |s| {
        Ok(s.db.execute("UPDATE jobs SET status='sending' WHERE id=?1 AND status='preparing' AND recall_requested=0", [e.id])?)
    })?;
    if started == 0 {
        return Ok(true);
    }
    let (reply, _) = std::sync::mpsc::sync_channel(1);
    let job = crate::bridge::Job {
        name: e.name.clone(),
        group: e.group,
        owner: 0,
        id: e.id,
        parts,
        operation,
        deadline: Instant::now() + Duration::from_secs(120),
        state: Arc::new(AtomicU8::new(1)),
        reply,
    };
    let content = job
        .parts
        .iter()
        .map(|p| match p {
            crate::delivery::Part::Text(t) => json!({"type":"text","text":t}),
            crate::delivery::Part::Image(b) => json!({"type":"image","bytes":b.len()}),
            crate::delivery::Part::Music(card) => json!({"type":"music","title":card.card.title,"artist":card.card.artist,"delivery":"native_type3_cgi"}),
        })
        .collect::<Vec<_>>();
    shared.event("outgoing",&json!({"session":e.name,"group":e.group,"message_id":e.id,"batch_id":e.batch,"content":content,"operation":"持久化发送"}));
    let result = send(&job);
    let error = result
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    finish(
        shared,
        e.id,
        if result.is_ok() { "sent" } else { "uncertain" },
        &error,
    )?;
    shared.event("delivery",&json!({"session":e.name,"group":e.group,"message_id":e.id,"batch_id":e.batch,"success":result.is_ok(),"error":error,"confirmation":crate::delivery::confirmation(&job.parts),"elapsed_ms":start.elapsed().as_millis()}));
    shared
        .last_send_us
        .store(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    if result.is_err() {
        shared.log(&format!(
            "持久化任务发送未确认：ID={}，{error}；未自动重发",
            e.id
        ));
    }
    Ok(true)
}
pub fn process_recall(
    shared: &Arc<Shared>,
    automation: &crate::delivery::Automation,
) -> Result<bool> {
    process_recall_with(shared, |id| {
        let result = automation.recall(shared, id);
        if result.is_err()
            && shared
                .ledger
                .lock()
                .unwrap()
                .sent
                .get(&id)
                .is_some_and(|r| r.messages.iter().any(|m| m.native_cgi.is_none()))
        {
            let c = shared.config.read().unwrap().clone();
            if let Ok(b) = c.binding() {
                if let Ok(h) = crate::native::validate_window(&b) {
                    automation.cancel_actions(h, &shared.data);
                }
            }
        }
        result
    })
}
pub fn confirm_late_recall(shared: &Shared, id: u64) -> Result<()> {
    let recalled = shared
        .ledger
        .lock()
        .unwrap()
        .sent
        .get(&id)
        .is_some_and(|r| r.recalled);
    if recalled {
        store(shared, |s| {
            s.db.execute("UPDATE jobs SET recall_status='recalled',recall_error='' WHERE id=?1 AND recall_status IN ('recalling','recall_failed','requested','scheduled')", [id])?;
            Ok(())
        })?;
    }
    Ok(())
}
fn process_recall_with(
    shared: &Arc<Shared>,
    mut recall: impl FnMut(u64) -> Result<()>,
) -> Result<bool> {
    let jobs = store(shared, |s| {
        let mut q=s.db.prepare("SELECT id,recall_delay,recall_requested,recall_status,enqueued FROM jobs WHERE recall_requested=1 OR recall_delay IS NOT NULL ORDER BY id LIMIT 100")?;
        Ok(q.query_map([], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, Option<f64>>(1)?,
                r.get::<_, bool>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, f64>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
    })?;
    for (id, delay, requested, recall_status, enqueued) in jobs {
        if matches!(
            recall_status.as_str(),
            "recalled" | "cancelled" | "recall_failed"
        ) {
            continue;
        }
        let due = if requested {
            Some(config::now())
        } else if let Some(delay) = delay {
            due_at(shared, id, delay)?.or_else(|| {
                let cap = crate::actions::recall_limit(&shared.config.read().unwrap().raw);
                Some(enqueued + delay.min(cap) - 3.)
            })
        } else {
            None
        };
        if due.is_none_or(|due| due > config::now()) {
            continue;
        }
        let cancelled_now = store(shared, |s| {
            let tx = s.db.transaction()?;
            tx.execute(
                "UPDATE jobs SET recall_status='recalling' WHERE id=?1",
                [id],
            )?;
            let changed=tx.execute("UPDATE jobs SET status='cancelled',error='该次输出已撤回，取消尚未发送内容',finished=?1,recall_status='cancelled' WHERE id=?2 AND status IN ('queued','preparing')",params![config::now(),id])?;
            tx.commit()?;
            Ok(changed > 0)
        })?;
        if cancelled_now || !shared.ledger.lock().unwrap().sent.contains_key(&id) {
            if !cancelled_now {
                store(shared, |s| {
                    s.db.execute("UPDATE jobs SET recall_status='recall_failed',recall_error='没有可核验的已发送记录' WHERE id=?1",[id])?;
                    Ok(())
                })?;
            }
            continue;
        }
        let result = recall(id);
        let (state, error) = match &result {
            Ok(()) => ("recalled", String::new()),
            Err(e) => ("recall_failed", e.to_string()),
        };
        store(shared, |s| {
            s.db.execute(
                "UPDATE jobs SET recall_status=?1,recall_error=?2 WHERE id=?3",
                params![state, error, id],
            )?;
            Ok(())
        })?;
        shared.log(&format!(
            "输出撤回{}：消息ID={id}{}{}",
            if result.is_ok() {
                "完成"
            } else {
                "未全部成功"
            },
            if error.is_empty() { "" } else { "；" },
            error
        ));
        return Ok(true);
    }
    Ok(false)
}
pub fn gui_summary(data: &Path) -> Result<Value> {
    let db = Connection::open_with_flags(
        data.join("outbox.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    db.busy_timeout(Duration::from_millis(100))?;
    let mut values = serde_json::Map::new();
    let mut q = db.prepare("SELECT status,count(*) FROM jobs GROUP BY status")?;
    for item in q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))? {
        let (k, v) = item?;
        values.insert(k, json!(v));
    }
    let paused: Option<String> = db
        .query_row(
            "SELECT value FROM friend_control WHERE name='security_pause'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    values.insert("friend_paused_reason".into(), json!(paused));
    Ok(Value::Object(values))
}
pub fn gui_text(data: &Path, token: &str) -> String {
    if !data.join("outbox.sqlite3").exists() {
        return "暂无发送任务。\r\n启动监听后，红色问答的回复会显示在这里。已入队、实际发送和撤回结果会分别记录。".into();
    }
    let read = (|| -> Result<String> {
        let db = Connection::open_with_flags(
            data.join("outbox.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.busy_timeout(Duration::from_millis(250))?;
        let mut q=db.prepare("SELECT j.id,j.name,j.status,j.batch_id,j.error,j.recall_status,j.recall_error,b.source FROM jobs j JOIN batches b ON b.batch_id=j.batch_id ORDER BY j.id DESC LIMIT 200")?;
        let mut lines=vec!["已入队表示内容已保存；“发送成功”表示微信已确认发出。\r\n每次输出拆分的图片、文字、艾特和提示共用一个撤回范围；其他输出独立，不会因为一次撤回而阻断后续发送。\r\n".to_owned()];
        for row in q.query_map([], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
            ))
        })? {
            let (id, name, state, batch, error, recall, recall_error, trigger) = row?;
            let label = match state.as_str() {
                "queued" => "待发送",
                "preparing" => "准备中",
                "sending" => "发送中",
                "sent" => "发送成功",
                "failed" => "准备失败",
                "uncertain" => "发送状态待核对",
                "cancelled" => "已取消",
                _ => &state,
            };
            let recall = match recall.as_str() {
                "" => "尚未撤回",
                "scheduled" => "已登记定时撤回",
                "requested" => "已登记撤回",
                "recalling" => "撤回中",
                "recalled" => "该次输出已撤回",
                "cancelled" => "已取消",
                "recall_failed" => "撤回未全部成功",
                _ => &recall,
            };
            lines.push(format!(
                "ID {id}　{name}　{label}\r\n触发 {trigger}　批次 {batch}　{recall}\r\n{error} {recall_error}\r\n"
            ));
        }
        Ok(lines.join("\r\n"))
    })();
    let text = read.unwrap_or_else(|e| format!("持久化队列尚未建立或暂不可读：{e}"));
    crate::telemetry::redact(&json!(text), token)
        .as_str()
        .unwrap_or("")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(path: &Path) -> (Config, Arc<Shared>) {
        let mut c = Config::default(path.join("config.json"));
        c.raw["whitelist"] = json!(["甲", "乙"]);
        c.raw["wechat_binding"] = json!({"pid":1,"created_at":1.,"hwnd":1});
        std::fs::create_dir_all(path.join("data/db/0123456789abcdef01234567")).unwrap();
        let (s, _) = Shared::new(c.clone());
        (c, s)
    }
    fn send(s: &Shared, c: &Config, name: &str, source: &str) -> Value {
        enqueue(s,c,&json!({"action":"send_private_msg","params":{"user_id":config::contact_id(name),"message":[{"type":"reply","data":{"id":source}},{"type":"text","data":{"text":"原文 ccc1 CCC1"}}]}})).unwrap()
    }
    fn track(s: &Shared, c: &Config, id: u64, at: f64, parts: usize) {
        s.ledger.lock().unwrap().sent.insert(
            id,
            crate::actions::Record {
                name: "甲".into(),
                group: false,
                binding: c.binding().unwrap(),
                messages: (0..parts)
                    .map(|n| crate::actions::Tracked {
                        native_cgi: None,
                        row: crate::database::Message {
                            local_id: 1 + n as i64,
                            server_id: 2 + n as i64,
                            sort_seq: 1 + n as i64,
                            create_time: at as i64,
                            kind: 1,
                            sender: "self".into(),
                            content: format!("part {n}"),
                            shard: "m".into(),
                        },
                        runtime_id: vec![],
                        sent_at: at,
                    })
                    .collect(),
                nodes: None,
                recalled: false,
            },
        );
        crate::actions::save(s).unwrap();
        finish(s, id, "sent", "").unwrap();
    }
    #[test]
    fn withdrawal_notices_schedule_early_recall() {
        assert_eq!(
            auto_recall_delay(&json!("该消息2分钟后撤回...")),
            Some(100.)
        );
        assert_eq!(auto_recall_delay(&json!("该消息30秒后撤回...")), Some(25.));
        assert_eq!(
            auto_recall_delay(&json!("@阿飞 施法中...请稍候...")),
            Some(25.)
        );
        assert_eq!(auto_recall_delay(&json!("普通回复，没有撤回提示")), None);
    }
    #[test]
    fn workflow_tracking_uses_one_trigger_batch_but_recall_stays_output_scoped() {
        let d = tempfile::tempdir().unwrap();
        let (c, s) = setup(d.path());
        let p = json!({"user_id":config::contact_id("甲"),"wechat_batch_id":"pipeline"});
        let flow = workflow(&s, &c, &p, true).unwrap();
        let receipt = send(&s, &c, "甲", "pipeline");
        assert_eq!(flow["batch_id"], receipt["batch_id"]);
        assert_eq!(receipt["recall_scope"], "output");
        assert_eq!(receipt["trigger_id"], "pipeline");
        let id = receipt["message_id"].as_u64().unwrap();
        let at = config::now() - 40.;
        track(&s, &c, id, at, 3);
        request_recall(&s, id, Some(1000.)).unwrap();
        let held = due_at(&s, id, 1.).unwrap().unwrap();
        assert!(held < config::now());
        assert!((held - (at + 0.9)).abs() < 0.1);
        assert_eq!(
            status(&s, id).unwrap().unwrap()["recall_status"],
            "scheduled"
        );
        workflow(&s, &c, &p, false).unwrap();
        assert_eq!(summary(&s).unwrap()["open_workflows"], 0);
    }
    #[test]
    fn same_trigger_outputs_share_trace_but_one_recall_never_blocks_later_sends() {
        let d = tempfile::tempdir().unwrap();
        let (c, s) = setup(d.path());
        let p = json!({"action":"send_private_msg","params":{"user_id":config::contact_id("甲"),"wechat_batch_id":"same","message":"body"}});
        let a = enqueue(&s, &c, &p).unwrap();
        let b = enqueue(&s, &c, &p).unwrap();
        assert_eq!(a["batch_id"], b["batch_id"]);
        let aid = a["message_id"].as_u64().unwrap();
        let bid = b["message_id"].as_u64().unwrap();
        track(&s, &c, aid, config::now() - 5., 1);
        request_recall(&s, aid, None).unwrap();
        assert_eq!(
            status(&s, bid).unwrap().unwrap()["delivery_status"],
            "queued"
        );
        assert!(
            enqueue(&s, &c, &json!({"action":"send_private_msg","params":{"user_id":config::contact_id("甲"),"wechat_batch_id":"same","message":"late final"}})).is_ok()
        );
        assert!(process_recall_with(&s, |_| Ok(())).unwrap());
        assert_eq!(
            status(&s, aid).unwrap().unwrap()["recall_status"],
            "recalled"
        );
    }
    #[test]
    fn one_outputs_three_physical_parts_share_one_recall_record() {
        let d = tempfile::tempdir().unwrap();
        let (c, s) = setup(d.path());
        let receipt = send(&s, &c, "甲", "500");
        let id = receipt["message_id"].as_u64().unwrap();
        track(&s, &c, id, config::now() - 5., 3);
        request_recall(&s, id, None).unwrap();
        let mut visited = vec![];
        assert!(
            process_recall_with(&s, |id| {
                visited.push(id);
                Ok(())
            })
            .unwrap()
        );
        assert_eq!(visited, [id]);
        assert_eq!(
            status(&s, id).unwrap().unwrap()["recall_status"],
            "recalled"
        );
        assert_eq!(s.ledger.lock().unwrap().sent[&id].messages.len(), 3);
    }
    #[test]
    fn delayed_recall_is_persistent_and_does_not_change_enqueue_deadline() {
        let d = tempfile::tempdir().unwrap();
        let (c, s) = setup(d.path());
        let a = send(&s, &c, "甲", "10");
        let id = a["message_id"].as_u64().unwrap();
        let v = request_recall(&s, id, Some(999999.)).unwrap().unwrap();
        assert_eq!(v["delay_seconds"], 120.);
        assert_eq!(status(&s, id).unwrap().unwrap()["recall_delay"], 120.);
        assert_eq!(
            status(&s, id).unwrap().unwrap()["delivery_status"],
            "queued"
        );
        assert!(!cancelled(&s, id).unwrap());
    }
    #[test]
    fn restart_resumes_only_work_that_cannot_have_been_submitted() {
        let d = tempfile::tempdir().unwrap();
        let (c, s) = setup(d.path());
        let a = send(&s, &c, "甲", "1");
        let b = send(&s, &c, "甲", "2");
        let remaining = send(&s, &c, "甲", "3");
        let aid = a["message_id"].as_u64().unwrap();
        let bid = b["message_id"].as_u64().unwrap();
        assert_eq!(claim(&s).unwrap().unwrap().id, aid);
        store(&s, |db| {
            db.db
                .execute("UPDATE jobs SET status='sending' WHERE id=?1", [aid])?;
            db.db
                .execute("UPDATE jobs SET status='preparing' WHERE id=?1", [bid])?;
            Ok(())
        })
        .unwrap();
        drop(s);
        let (next, _) = Shared::new(c);
        recover(&next).unwrap();
        assert_eq!(
            status(&next, aid).unwrap().unwrap()["delivery_status"],
            "uncertain"
        );
        assert_eq!(claim(&next).unwrap().unwrap().id, bid);
        finish(&next, bid, "sent", "").unwrap();
        assert_eq!(
            claim(&next).unwrap().unwrap().id,
            remaining["message_id"].as_u64().unwrap()
        );
    }
    #[test]
    fn sixty_four_receipts_cross_restart_and_execute_fifo_without_a_live_connection() {
        let d = tempfile::tempdir().unwrap();
        let (c, s) = setup(d.path());
        let mut ids = vec![];
        for i in 0..64 {
            ids.push(
                send(&s, &c, "甲", &i.to_string())["message_id"]
                    .as_u64()
                    .unwrap(),
            );
        }
        drop(s);
        let (s, _) = Shared::new(c);
        recover(&s).unwrap();
        let mut executed = vec![];
        s.ready.store(true, Ordering::SeqCst);
        while process_one_with(&s, |job| {
            executed.push(job.id);
            assert_eq!(job.owner, 0);
            Ok(())
        })
        .unwrap()
        {}
        assert_eq!(executed, ids);
        assert_eq!(summary(&s).unwrap()["sent"], 64);
        assert!(gui_text(&s.data, "").contains("发送成功"));
    }
    #[test]
    fn failed_output_recall_never_touches_its_sibling_output_or_cancels_late_send() {
        let d = tempfile::tempdir().unwrap();
        let (c, s) = setup(d.path());
        let a = send(&s, &c, "甲", "batch");
        let b = send(&s, &c, "甲", "batch");
        let ids = [
            a["message_id"].as_u64().unwrap(),
            b["message_id"].as_u64().unwrap(),
        ];
        track(&s, &c, ids[0], config::now() - 5., 2);
        request_recall(&s, ids[0], None).unwrap();
        assert!(process_recall_with(&s, |_| bail!("fixture native failure")).unwrap());
        assert_eq!(
            status(&s, ids[0]).unwrap().unwrap()["recall_status"],
            "recall_failed"
        );
        assert_eq!(
            status(&s, ids[1]).unwrap().unwrap()["delivery_status"],
            "queued"
        );
        assert!(enqueue(&s,&c,&json!({"action":"send_private_msg","params":{"user_id":config::contact_id("甲"),"wechat_batch_id":"batch","message":"late"}})).is_ok());
    }
    #[test]
    fn output_timer_reserves_each_part_before_its_own_cap() {
        let mut times = vec![100., 102., 104.];
        let due = batch_deadline(&mut times, 120., 120., false).unwrap();
        assert_eq!(due, 215.);
        for (i, at) in times.iter().enumerate() {
            assert!(due + 3. * (i + 1) as f64 <= at + 120.);
        }
        assert_eq!(batch_deadline(&mut times, 30., 120., false), Some(131.));
        assert_eq!(batch_deadline(&mut times, 30., 120., true), Some(215.));
    }
}
