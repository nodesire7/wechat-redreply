use super::config::{Profile, Settings};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub account: String,
    pub session: String,
    pub name: String,
    pub group: bool,
    pub uid: u64,
    pub group_id: u64,
    pub id: u64,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Input {
    pub source: Option<Source>,
    pub settings: Settings,
    pub profile: Profile,
    pub text: String,
    pub revision: i64,
}
pub(super) fn open(data: &Path) -> Result<Connection> {
    // Schema initialization is an infrequent migration, not part of every
    // message/task operation. Concurrent CREATE/PRAGMA upgrades can fail before
    // SQLite's ordinary write busy-handler even has a chance to queue a writer.
    static INITIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _initializing = INITIALIZE
        .lock()
        .map_err(|_| anyhow::anyhow!("AI 数据库初始化锁异常"))?;
    std::fs::create_dir_all(data)?;
    let mut db = Connection::open(data.join("ai.sqlite3"))?;
    db.busy_timeout(Duration::from_millis(1000))?;
    db.pragma_update(None, "synchronous", "FULL")?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version == 0 {
        db.pragma_update(None, "journal_mode", "WAL")?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch("
        CREATE TABLE IF NOT EXISTS sources(account TEXT NOT NULL,id INTEGER NOT NULL,payload TEXT NOT NULL,at REAL NOT NULL,PRIMARY KEY(account,id));
        CREATE INDEX IF NOT EXISTS sources_age ON sources(at);
        CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,revision INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS tasks(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE NOT NULL,request_key TEXT UNIQUE NOT NULL,fingerprint TEXT NOT NULL,account TEXT NOT NULL,session TEXT NOT NULL,kind TEXT NOT NULL,profile_id TEXT NOT NULL,payload TEXT NOT NULL,revision INTEGER NOT NULL,status TEXT NOT NULL,created REAL NOT NULL,deadline REAL NOT NULL,expires REAL NOT NULL,started REAL,finished REAL,result TEXT NOT NULL DEFAULT '',error TEXT NOT NULL DEFAULT '',usage TEXT NOT NULL DEFAULT '{}',message_id INTEGER,reserved INTEGER NOT NULL DEFAULT 0);
        CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status,seq);
        CREATE INDEX IF NOT EXISTS tasks_schedule ON tasks(kind,status,seq,id);
        CREATE INDEX IF NOT EXISTS tasks_session ON tasks(session,status,seq);
        CREATE INDEX IF NOT EXISTS tasks_rate ON tasks(started);
        CREATE INDEX IF NOT EXISTS tasks_budget ON tasks(started,reserved);
        CREATE INDEX IF NOT EXISTS tasks_generated_expiry ON tasks(expires) WHERE status='generated' AND kind='chat';
        CREATE INDEX IF NOT EXISTS tasks_retention ON tasks(finished,status,seq,id);
        CREATE TABLE IF NOT EXISTS turns(task_id TEXT PRIMARY KEY,session TEXT NOT NULL,profile_id TEXT NOT NULL,user_text TEXT NOT NULL,assistant_text TEXT NOT NULL,at REAL NOT NULL);
        CREATE INDEX IF NOT EXISTS turns_session ON turns(session,profile_id,at);
        CREATE INDEX IF NOT EXISTS turns_age ON turns(at);
        PRAGMA user_version=1;")?;
        tx.commit()?;
    }
    Ok(db)
}
pub fn observe(
    data: &Path,
    account: &str,
    peer: &crate::database::Peer,
    uid: u64,
    row: &crate::database::Message,
    id: u64,
) -> Result<()> {
    // Real wxids (not display names) form memory identity. A group member has
    // independent short context; public group history is not silently shared.
    let session = hex::encode(Sha256::digest(serde_json::to_vec(&(
        account,
        &peer.wxid,
        &row.sender,
        peer.group,
    ))?));
    let source = Source {
        account: account.into(),
        session,
        name: peer.name.clone(),
        group: peer.group,
        uid,
        group_id: if peer.group {
            crate::config::group_id(&peer.name)
        } else {
            0
        },
        id,
        text: row.content.chars().take(20000).collect(),
    };
    open(data)?.execute(
        "INSERT OR IGNORE INTO sources(account,id,payload,at) VALUES(?1,?2,?3,?4)",
        params![
            account,
            id,
            serde_json::to_string(&source)?,
            crate::config::now()
        ],
    )?;
    Ok(())
}
pub fn source(data: &Path, account: &str, id: u64) -> Result<Source> {
    let payload: String = open(data)?
        .query_row(
            "SELECT payload FROM sources WHERE account=?1 AND id=?2 AND at>?3",
            params![account, id, crate::config::now() - 86400.],
            |r| r.get(0),
        )
        .optional()?
        .context("未找到此账号24小时内的 AI 来源消息；启用AI后请重新发起指令")?;
    Ok(serde_json::from_str(&payload)?)
}
pub fn submit(
    data: &Path,
    settings: &Settings,
    profile: &Profile,
    source: Option<&Source>,
    p: &Value,
) -> Result<Value> {
    let text = p["text"]
        .as_str()
        .or_else(|| source.map(|s| s.text.as_str()))
        .context("缺少 AI 输入文本")?
        .trim();
    if text.is_empty() || text.len() > 20000 {
        bail!("AI 输入不能为空，且最多20000字节");
    }
    if text.len() + profile.system_prompt.len() + 128 > settings.input_budget {
        bail!("本条输入和系统提示词超过输入预算，请缩短内容或增加预算");
    }
    let request_id = p["request_id"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| source.map(|s| format!("message:{}", s.id)))
        .context("缺少 request_id")?;
    if request_id.is_empty() || request_id.len() > 160 {
        bail!("request_id 长度须为1–160字节");
    }
    let account = source.map_or("probe", |s| s.account.as_str());
    let key = hex::encode(Sha256::digest(serde_json::to_vec(&(account, &request_id))?));
    let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&(
        source.map(|s| (&s.session, s.id)),
        profile,
        text,
    ))?));
    let mut db = open(data)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some((id, old)) = tx
        .query_row(
            "SELECT id,fingerprint FROM tasks WHERE request_key=?1",
            [&key],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if old != fingerprint {
            bail!("相同 request_id 已用于不同的 AI 请求，未重复执行");
        }
        drop(tx);
        return get(data, &id);
    }
    let pending:i64=tx.query_row("SELECT count(*) FROM tasks WHERE status IN ('queued','running','awaiting_delivery') OR (status='generated' AND kind='chat')",[],|r|r.get(0))?;
    if pending >= 1000 {
        bail!("AI 持久化队列已达1000项，未接受新任务；未丢弃旧消息");
    }
    let id = format!("ai-{:032x}", rand::random::<u128>());
    let session = source.map_or_else(|| id.clone(), |s| s.session.clone());
    tx.execute("INSERT OR IGNORE INTO sessions(id) VALUES(?1)", [&session])?;
    let revision: i64 = tx.query_row(
        "SELECT revision FROM sessions WHERE id=?1",
        [&session],
        |r| r.get(0),
    )?;
    let input = Input {
        source: source.cloned(),
        settings: settings.clone(),
        profile: profile.clone(),
        text: text.into(),
        revision,
    };
    let now = crate::config::now();
    tx.execute("INSERT INTO tasks(id,request_key,fingerprint,account,session,kind,profile_id,payload,revision,status,created,deadline,expires) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'queued',?10,?11,?12)",params![id,key,fingerprint,account,session,if source.is_some(){"chat"}else{"probe"},profile.id,serde_json::to_string(&input)?,revision,now,now+settings.timeout_seconds as f64,now+settings.timeout_seconds as f64+300.])?;
    tx.commit()?;
    get(data, &id)
}
fn view(row: &rusqlite::Row<'_>, include_result: bool) -> rusqlite::Result<Value> {
    let input: Input = serde_json::from_str(&row.get::<_, String>(12)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(12, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let mut v = json!({"task_id":row.get::<_,String>(0)?,"account":row.get::<_,String>(1)?,"kind":row.get::<_,String>(2)?,"profile_id":row.get::<_,String>(3)?,"model":input.profile.model,"status":row.get::<_,String>(4)?,"created":row.get::<_,f64>(5)?,"deadline":row.get::<_,f64>(6)?,"expires":row.get::<_,f64>(7)?,"finished":row.get::<_,Option<f64>>(8)?,"error":row.get::<_,String>(9)?,"message_id":row.get::<_,Option<u64>>(10)?,"usage":serde_json::from_str::<Value>(&row.get::<_,String>(11)?).unwrap_or(json!({})),"session_name":input.source.as_ref().map_or("连接测试",|s|s.name.as_str()),"group":input.source.as_ref().is_some_and(|s|s.group),"source_message_id":input.source.as_ref().map(|s|s.id),"session_id":row.get::<_,String>(13)?,"user_id":input.source.as_ref().map(|s|s.uid),"generated_is_delivered":false});
    if include_result {
        v["text"] = row.get::<_, String>(14)?.into();
    }
    v.as_object_mut().unwrap().remove("generated_is_delivered");
    v["confirmed_delivered"] = json!(v["status"] == "delivered");
    Ok(v)
}
const VIEW: &str = "SELECT id,account,kind,profile_id,status,created,deadline,expires,finished,error,message_id,usage,payload,session,result FROM tasks";
pub fn get(data: &Path, id: &str) -> Result<Value> {
    open(data)?
        .query_row(&format!("{VIEW} WHERE id=?1"), [id], |r| view(r, true))
        .optional()?
        .context("AI 任务不存在")
}
pub fn list(data: &Path, context_minutes: u64) -> Result<Value> {
    let db = open(data)?;
    let mut q = db.prepare(&format!("{VIEW} ORDER BY seq DESC LIMIT 100"))?;
    let tasks = q
        .query_map([], |r| view(r, true))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut q=db.prepare("SELECT s.session,s.profile_id,s.n,s.at,p.payload FROM (SELECT session,profile_id,count(*) AS n,max(at) AS at,task_id FROM turns WHERE at>?1 GROUP BY session,profile_id ORDER BY max(at) DESC LIMIT 100) s JOIN tasks p ON p.id=s.task_id")?;
    let sessions=q.query_map([crate::config::now()-context_minutes as f64*60.],|r|{
        let input:Input=serde_json::from_str(&r.get::<_,String>(4)?).map_err(|e|rusqlite::Error::FromSqlConversionFailure(4,rusqlite::types::Type::Text,Box::new(e)))?;
        Ok(json!({"session_id":r.get::<_,String>(0)?,"profile_id":r.get::<_,String>(1)?,"turns":r.get::<_,i64>(2)?,"updated":r.get::<_,f64>(3)?,"name":input.source.as_ref().map(|s|s.name.as_str()),"group":input.source.as_ref().is_some_and(|s|s.group),"user_id":input.source.as_ref().map(|s|s.uid)}))
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!({"tasks":tasks,"contexts":sessions}))
}
pub fn cancel(data: &Path, id: &str) -> Result<Value> {
    let mut db = open(data)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    repair_links_in(&tx, data)?;
    let n=tx.execute("UPDATE tasks SET status='cancelled',finished=?2,error='用户已取消；未继续生成或提交发送' WHERE id=?1 AND (status IN ('queued','running') OR (status='generated' AND kind='chat'))",params![id,crate::config::now()])?;
    tx.commit()?;
    if n == 0 {
        bail!("此任务已结束或已提交发送；已提交的输出请在发送队列处理");
    }
    get(data, id)
}
pub fn clear_context(data: &Path, session: Option<&str>) -> Result<Value> {
    if session.is_some_and(|s| s.len() != 64) {
        bail!("上下文 ID 无效");
    }
    let mut db = open(data)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    repair_links_in(&tx, data)?;
    let count = tx.execute(
        "DELETE FROM turns WHERE (?1 IS NULL OR session=?1)",
        [session],
    )?;
    tx.execute(
        "UPDATE sessions SET revision=revision+1 WHERE (?1 IS NULL OR id=?1)",
        [session],
    )?;
    tx.execute("UPDATE tasks SET status='cancelled',finished=?2,error='上下文已清理，旧任务已取消' WHERE (?1 IS NULL OR session=?1) AND status IN ('queued','running','generated')",params![session,crate::config::now()])?;
    tx.commit()?;
    Ok(
        json!({"deleted_turns":count,"notice":"上下文已清理，未提交的相关任务已取消；已发送的微信消息不受影响"}),
    )
}
#[cfg(test)]
pub fn attach(data: &Path, id: &str, receipt: u64) -> Result<()> {
    let db = open(data)?;
    let count=db.execute("UPDATE tasks SET message_id=?2,status=CASE WHEN status='generated' THEN 'awaiting_delivery' ELSE status END WHERE id=?1 AND status IN ('generated','awaiting_delivery','delivered','uncertain') AND (message_id IS NULL OR message_id=?2)",params![id,receipt])?;
    if count != 1 {
        bail!("AI 发送回执已生成，但任务状态发生变化；请查看发送队列，勿手动重复发送");
    }
    Ok(())
}
pub fn submit_output(
    data: &Path,
    id: &str,
    f: impl FnOnce(&Value) -> Result<Value>,
) -> Result<Value> {
    // Reserve the state while enqueuing. Cancellation cannot report success
    // concurrently with a native output being accepted. The outbox has its own
    // persistent idempotency key, covering a crash between these two commits.
    let mut db = open(data)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    repair_links_in(&tx, data)?;
    let task = tx
        .query_row(&format!("{VIEW} WHERE id=?1"), [id], |r| view(r, true))
        .optional()?
        .context("AI 任务不存在")?;
    let receipt = f(&task)?;
    let message_id = receipt["message_id"].as_u64().context("发送回执无效")?;
    tx.execute("UPDATE tasks SET message_id=?2,status=CASE WHEN status='generated' THEN 'awaiting_delivery' ELSE status END WHERE id=?1",params![id,message_id])?;
    tx.commit()?;
    Ok(receipt)
}
pub fn input(data: &Path, id: &str) -> Result<Input> {
    let raw: String =
        open(data)?.query_row("SELECT payload FROM tasks WHERE id=?1", [id], |r| r.get(0))?;
    Ok(serde_json::from_str(&raw)?)
}
/// Transactional claim also coordinates the GUI's probes and listener workers.
pub fn claim(data: &Path, id: &str, settings: &Settings) -> Result<bool> {
    let mut db = open(data)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = crate::config::now();
    let info: Option<(String, String, f64, i64)> = tx
        .query_row(
            "SELECT status,session,deadline,seq FROM tasks WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((status, session, deadline, seq)) = info else {
        return Ok(false);
    };
    if status != "queued" || now >= deadline {
        return Ok(false);
    }
    let active: i64 = tx.query_row(
        "SELECT count(*) FROM tasks WHERE status='running'",
        [],
        |r| r.get(0),
    )?;
    let earlier:i64=tx.query_row("SELECT count(*) FROM tasks WHERE session=?1 AND seq<?2 AND status IN ('queued','running','generated','awaiting_delivery')",params![session,seq],|r|r.get(0))?;
    let recent: i64 = tx.query_row(
        "SELECT count(*) FROM tasks WHERE started>?1",
        [now - 60.],
        |r| r.get(0),
    )?;
    if active >= settings.concurrency as i64
        || earlier > 0
        || recent >= settings.requests_per_minute as i64
    {
        return Ok(false);
    }
    let raw: String = tx.query_row("SELECT payload FROM tasks WHERE id=?1", [id], |r| r.get(0))?;
    let input: Input = serde_json::from_str(&raw)?;
    let reserve = (input.settings.input_budget + input.settings.max_output_tokens) as i64;
    let used: i64 = tx.query_row(
        "SELECT coalesce(sum(reserved),0) FROM tasks WHERE started>=?1",
        [(now / 86400.).floor() * 86400.],
        |r| r.get(0),
    )?;
    if settings.daily_token_budget > 0 && used + reserve > settings.daily_token_budget as i64 {
        tx.execute("UPDATE tasks SET status='failed',finished=?2,error='已达每日 token 预留预算，未调用模型' WHERE id=?1",params![id,now])?;
        tx.commit()?;
        return Ok(false);
    }
    tx.execute(
        "UPDATE tasks SET status='running',started=?2,reserved=?3 WHERE id=?1 AND status='queued'",
        params![id, now, reserve],
    )?;
    tx.commit()?;
    Ok(true)
}
pub fn messages(data: &Path, id: &str, input: &Input) -> Result<Vec<Value>> {
    let mut messages = vec![];
    if !input.profile.system_prompt.is_empty() {
        messages.push(json!({"role":"system","content":input.profile.system_prompt}));
    }
    let mut pairs = vec![];
    let db = open(data)?;
    if let Some(source) = &input.source {
        let mut q=db.prepare("SELECT user_text,assistant_text FROM turns WHERE session=?1 AND profile_id=?2 AND at>?3 ORDER BY at DESC LIMIT ?4")?;
        pairs = q
            .query_map(
                params![
                    source.session,
                    input.profile.id,
                    crate::config::now() - input.settings.context_minutes as f64 * 60.,
                    input.settings.context_turns as i64
                ],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
    }
    let mut remaining = input
        .settings
        .input_budget
        .saturating_sub(input.text.len() + input.profile.system_prompt.len() + 128);
    let mut kept = vec![];
    for (user, assistant) in pairs {
        let cost = user.len() + assistant.len() + 64;
        if cost > remaining {
            break;
        }
        remaining -= cost;
        kept.push((user, assistant));
    }
    for (user, assistant) in kept.into_iter().rev() {
        messages.push(json!({"role":"user","content":user}));
        messages.push(json!({"role":"assistant","content":assistant}));
    }
    messages.push(json!({"role":"user","content":input.text}));
    // Reading the task makes the query fail explicitly if it was removed.
    let _: i64 = db.query_row("SELECT count(*) FROM tasks WHERE id=?1", [id], |r| r.get(0))?;
    Ok(messages)
}
pub fn finish(data: &Path, id: &str, result: Result<(String, Value)>) -> Result<()> {
    let db = open(data)?;
    match result {
        Ok((text, usage)) => {
            db.execute("UPDATE tasks SET status='generated',result=?2,usage=?3,finished=?4 WHERE id=?1 AND status='running'",params![id,text,usage.to_string(),crate::config::now()])?;
        }
        Err(e) => {
            db.execute("UPDATE tasks SET status='failed',error=?2,finished=?3 WHERE id=?1 AND status='running'",params![id,e.to_string(),crate::config::now()])?;
        }
    }
    Ok(())
}
pub fn recover(data: &Path) -> Result<()> {
    open(data)?.execute("UPDATE tasks SET status='uncertain',error='模型调用期间程序中断，可能已产生用量；未自动重试',finished=?1 WHERE kind='chat' AND status='running'",[crate::config::now()])?;
    Ok(())
}
pub fn pending(data: &Path) -> Result<Vec<String>> {
    let db = open(data)?;
    let now = crate::config::now();
    db.execute("UPDATE tasks SET status='uncertain',finished=?1,error='模型任务超时或执行进程已中断；可能产生用量，未自动重试' WHERE status='running' AND deadline<?2",params![now,now-1.])?;
    db.execute("UPDATE tasks SET status='expired',finished=?1,error='任务排队已超时，未自动发送' WHERE status='queued' AND deadline<?1",[now])?;
    db.execute("UPDATE tasks SET status='expired',finished=?1,error='生成结果已过领取期限，未自动发送' WHERE status='generated' AND kind='chat' AND expires<?1",[now])?;
    let mut q = db.prepare(
        "SELECT id FROM tasks WHERE status='queued' AND kind='chat' ORDER BY seq LIMIT 1000",
    )?;
    Ok(q.query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
pub fn receipts(data: &Path) -> Result<Vec<(String, u64)>> {
    let mut db = open(data)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    repair_links_in(&tx, data)?;
    tx.commit()?;
    let mut q = db.prepare(
        "SELECT id,message_id FROM tasks WHERE status='awaiting_delivery' ORDER BY seq LIMIT 1000",
    )?;
    Ok(q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}
fn repair_links_in(tx: &rusqlite::Transaction<'_>, data: &Path) -> Result<()> {
    let mut q = tx.prepare(
        "SELECT id FROM tasks WHERE kind='chat' AND status='generated' AND message_id IS NULL",
    )?;
    let ids = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(q);
    for (id, message_id) in crate::outbox::ai_links(data, &ids)? {
        tx.execute("UPDATE tasks SET status='awaiting_delivery',message_id=?2 WHERE id=?1 AND status='generated' AND message_id IS NULL",params![id,message_id])?;
    }
    Ok(())
}
pub fn delivery(data: &Path, id: &str, status: &str) -> Result<()> {
    let mut db = open(data)?;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let row:Option<(String,String,String,i64)>=tx.query_row("SELECT payload,result,session,revision FROM tasks WHERE id=?1 AND status='awaiting_delivery'",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let Some((raw, text, session, revision)) = row else {
        return Ok(());
    };
    let target = match status {
        "sent" => "delivered",
        "failed" => "failed",
        "cancelled" => "cancelled",
        "uncertain" => "uncertain",
        _ => return Ok(()),
    };
    let now = crate::config::now();
    if target == "delivered" {
        let input: Input = serde_json::from_str(&raw)?;
        let current: i64 = tx.query_row(
            "SELECT revision FROM sessions WHERE id=?1",
            [&session],
            |r| r.get(0),
        )?;
        if revision == current && input.settings.context_turns > 0 {
            tx.execute("INSERT OR IGNORE INTO turns(task_id,session,profile_id,user_text,assistant_text,at) VALUES(?1,?2,?3,?4,?5,?6)",params![id,session,input.profile.id,input.text,text,now])?;
            tx.execute("DELETE FROM turns WHERE session=?1 AND profile_id=?2 AND task_id NOT IN (SELECT task_id FROM turns WHERE session=?1 AND profile_id=?2 ORDER BY at DESC LIMIT ?3)",params![session,input.profile.id,input.settings.context_turns as i64])?;
        }
    }
    tx.execute(
        "UPDATE tasks SET status=?2,finished=?3,error=?4 WHERE id=?1",
        params![
            id,
            target,
            now,
            if target == "delivered" {
                ""
            } else {
                "微信发送未确认成功；未写入已送达上下文，请查看发送队列"
            }
        ],
    )?;
    tx.commit()?;
    Ok(())
}
pub fn prune(data: &Path, context_minutes: u64) -> Result<()> {
    let db = open(data)?;
    let now = crate::config::now();
    db.execute("DELETE FROM sources WHERE at<?1 OR rowid NOT IN (SELECT rowid FROM sources ORDER BY at DESC LIMIT 10000)",[now-86400.])?;
    db.execute(
        "DELETE FROM turns WHERE at<?1",
        [now - context_minutes as f64 * 60.],
    )?;
    db.execute("DELETE FROM tasks WHERE seq NOT IN (SELECT seq FROM tasks ORDER BY seq DESC LIMIT 2000) AND status IN ('failed','cancelled','expired','uncertain','delivered','generated') AND id NOT IN (SELECT task_id FROM turns) AND finished<?1",[now-86400.])?;
    Ok(())
}
