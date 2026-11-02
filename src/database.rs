//! Read-only SQLCipher/WCDB reception. Algorithm ported from the licensed
//! wechatauto reader; no Python runtime, injection, or cross-account fallback.
use crate::{
    config::{self, Config},
    native::Process,
    service::Shared,
};
use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::NoPadding};
use anyhow::{Context, Result, bail};
use hmac::{Hmac, Mac};
use md5::Md5;
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};

use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant, SystemTime},
};
const PAGE: usize = 4096;
const RESERVE: usize = 80;
pub fn verify_key(key: &[u8], page: &[u8]) -> bool {
    if page.len() != PAGE || ![32, 48].contains(&key.len()) {
        return false;
    }
    let salt = if key.len() == 48 {
        &key[32..]
    } else {
        &page[..16]
    };
    let salt: Vec<_> = salt.iter().map(|b| b ^ 0x3a).collect();
    let mut mac_key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha512>(&key[..32], &salt, 2, &mut mac_key);
    let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(&mac_key).unwrap();
    mac.update(&page[16..PAGE - RESERVE + 16]);
    mac.update(&1u32.to_le_bytes());
    mac.verify_slice(&page[PAGE - 64..]).is_ok()
}
pub fn decrypt_page(key: &[u8], page: &[u8], number: u32) -> Result<Vec<u8>> {
    if page.len() != PAGE {
        bail!("数据库页不完整");
    }
    let iv = &page[PAGE - RESERVE..PAGE - RESERVE + 16];
    let start = if number == 1 { 16 } else { 0 };
    let cipher = cbc::Decryptor::<aes::Aes256>::new_from_slices(&key[..32], iv)
        .map_err(|_| anyhow::anyhow!("无效页密钥"))?;
    let data = cipher
        .decrypt_padded_vec_mut::<NoPadding>(&page[start..PAGE - RESERVE])
        .map_err(|_| anyhow::anyhow!("页解密失败"))?;
    let mut out = Vec::with_capacity(PAGE);
    if number == 1 {
        out.extend_from_slice(if key.len() == 48 {
            &page[..16]
        } else {
            b"SQLite format 3\0"
        });
    }
    out.extend(data);
    out.resize(PAGE, 0);
    Ok(out)
}
fn readable(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    Ok(OpenOptions::new().read(true).share_mode(7).open(path)?)
}
#[derive(Clone, PartialEq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
}
fn stamp(path: &Path) -> Result<Stamp> {
    let m = fs::metadata(path)?;
    Ok(Stamp {
        len: m.len(),
        modified: m.modified()?,
    })
}
struct Snapshot {
    source: PathBuf,
    target: PathBuf,
    key: Vec<u8>,
    base: Option<Stamp>,
    wal: Option<Stamp>,
    salt: [u8; 8],
    frames: usize,
}
impl Snapshot {
    fn update(&mut self) -> Result<PathBuf> {
        let before = stamp(&self.source)?;
        let walpath = PathBuf::from(format!("{}-wal", self.source.display()));
        let wal = stamp(&walpath).ok().filter(|s| s.len >= 32);
        let mut header = [0u8; 32];
        if wal.is_some() {
            readable(&walpath)?.read_exact(&mut header)?;
        }
        let salt: [u8; 8] = header[16..24].try_into()?;
        let rebuild = self.base.as_ref() != Some(&before)
            || !self.target.exists()
            || salt != self.salt
            || wal
                .as_ref()
                .is_some_and(|s| s.len < (32 + self.frames * (PAGE + 24)) as u64)
            || self.wal.is_some() != wal.is_some();
        if !rebuild && self.wal == wal {
            return Ok(self.target.clone());
        }
        if rebuild {
            let temp = self.target.with_extension("building");
            let mut src = readable(&self.source)?;
            let mut out = File::create(&temp)?;
            let mut page = [0u8; PAGE];
            let mut number = 1;
            loop {
                match src.read_exact(&mut page) {
                    Ok(()) => {
                        out.write_all(&decrypt_page(&self.key, &page, number)?)?;
                        number += 1;
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e.into()),
                }
            }
            out.flush()?;
            drop(out);
            if number < 2 {
                bail!("数据库尚未写完");
            }
            replace_file(&temp, &self.target)?;
            self.frames = 0;
        }
        if let Some(w) = &wal {
            let total = (w.len.saturating_sub(32) as usize) / (PAGE + 24);
            let mut source = readable(&walpath)?;
            // Ignore a partially written transaction. Only complete commit batches
            // enter the decrypted snapshot, and ignore frames from an old WAL salt.
            let mut commit = self.frames;
            for frame in self.frames..total {
                source.seek(SeekFrom::Start((32 + frame * (PAGE + 24)) as u64))?;
                let mut fh = [0u8; 24];
                source.read_exact(&mut fh)?;
                if fh[8..16] != salt {
                    break;
                }
                if u32::from_be_bytes(fh[4..8].try_into()?) != 0 {
                    commit = frame + 1;
                }
            }
            let mut out = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.target)?;
            let mut last_pages = 0u32;
            for frame in self.frames..commit {
                source.seek(SeekFrom::Start((32 + frame * (PAGE + 24)) as u64))?;
                let mut fh = [0u8; 24];
                let mut page = [0u8; PAGE];
                source.read_exact(&mut fh)?;
                source.read_exact(&mut page)?;
                if fh[8..16] != salt {
                    bail!("数据库 WAL 已换代，等待重试");
                }
                let pg = u32::from_be_bytes(fh[..4].try_into()?);
                if pg == 0 || pg > 10_000_000 {
                    bail!("无效 WAL 页编号");
                }
                let plain = decrypt_page(&self.key, &page, pg)?;
                out.seek(SeekFrom::Start((pg as u64 - 1) * PAGE as u64))?;
                out.write_all(&plain)?;
                let size = u32::from_be_bytes(fh[4..8].try_into()?);
                if size > 0 {
                    last_pages = size;
                }
            }
            if last_pages > 0 {
                out.set_len(last_pages as u64 * PAGE as u64)?;
            }
            out.flush()?;
            self.frames = commit;
            let mut current = [0u8; 32];
            readable(&walpath)?.read_exact(&mut current)?;
            if current[16..24] != salt || stamp(&self.source)? != before {
                self.base = None;
                bail!("数据库正在 checkpoint，等待重试");
            }
        }
        self.base = Some(before);
        self.wal = wal;
        self.salt = salt;
        Ok(self.target.clone())
    }
    fn open(&mut self) -> Result<Connection> {
        let path = self.update()?;
        let c = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        c.pragma_update(None, "query_only", true)?;
        Ok(c)
    }
}
fn replace_file(src: &Path, dst: &Path) -> Result<()> {
    use windows::{Win32::Storage::FileSystem::*, core::PCWSTR};
    let a = config::wide(src);
    let b = config::wide(dst);
    unsafe {
        MoveFileExW(
            PCWSTR(a.as_ptr()),
            PCWSTR(b.as_ptr()),
            MOVEFILE_REPLACE_EXISTING,
        )?;
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize)]
pub struct Peer {
    pub name: String,
    pub group: bool,
    pub wxid: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub local_id: i64,
    pub server_id: i64,
    pub sort_seq: i64,
    pub create_time: i64,
    pub kind: i64,
    pub sender: String,
    pub content: String,
    pub shard: String,
}
impl Message {
    pub fn key(&self) -> String {
        format!("{}:{}", self.shard, self.local_id)
    }
}
pub struct Database {
    pub root: PathBuf,
    pub identity: String,
    pub wxid: String,
    pub cache: PathBuf,
    files: HashMap<String, Snapshot>,
}
impl Database {
    pub(crate) fn staging(&mut self, c: &Config) -> Result<Peer> {
        let name = c.raw["forward_staging_chat"].as_str().unwrap_or("");
        let mut temp = c.clone();
        temp.raw["whitelist"] = serde_json::json!([name]);
        temp.raw["blacklist"] = serde_json::json!([]);
        temp.raw["group_whitelist"] = serde_json::json!([]);
        let peer = self.peers(&temp)?.remove(0);
        if peer.wxid != self.wxid {
            bail!("中转会话不是当前登录账号自己；未发送到其他联系人");
        }
        Ok(peer)
    }
    pub fn new(c: &Config, data: &Path) -> Result<Self> {
        let process = Process::bound(&c.binding()?)?;
        let root = process.account_root()?;
        let normalized = root
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_owned();
        let root = PathBuf::from(normalized);
        let identity = hex::encode(Sha256::digest(
            root.to_string_lossy().to_lowercase().as_bytes(),
        ))[..24]
            .to_owned();
        let cache = data.join("db").join(&identity);
        fs::create_dir_all(&cache)?;
        let mut paths = vec![
            (
                "contact/contact.db".to_owned(),
                root.join("db_storage/contact/contact.db"),
            ),
            (
                "session/session.db".to_owned(),
                root.join("db_storage/session/session.db"),
            ),
        ];
        for f in fs::read_dir(root.join("db_storage/message"))? {
            let path = f?.path();
            let name = path.file_name().unwrap().to_string_lossy();
            if regex::Regex::new(r"^message_\d+\.db$")
                .unwrap()
                .is_match(&name)
            {
                paths.push((format!("message/{name}"), path));
            }
        }
        let pages: Vec<_> = paths
            .iter()
            .map(|(rel, path)| {
                let mut page = vec![0; PAGE];
                readable(path)?.read_exact(&mut page)?;
                Ok((rel.clone(), page))
            })
            .collect::<Result<_>>()?;
        let keys = crate::native::keys(&process, &pages)?;
        let mut files = HashMap::new();
        for (rel, path) in paths {
            files.insert(
                rel.clone(),
                Snapshot {
                    source: path,
                    target: cache.join(rel.replace('/', "__")),
                    key: keys[&rel].clone(),
                    base: None,
                    wal: None,
                    salt: [0; 8],
                    frames: 0,
                },
            );
        }
        let account = root.file_name().unwrap().to_string_lossy();
        let wxid = account
            .rsplit_once('_')
            .map_or(account.as_ref(), |(base, suffix)| {
                if suffix.len() == 4 {
                    base
                } else {
                    account.as_ref()
                }
            })
            .to_owned();
        let mut db = Self {
            root,
            identity,
            wxid,
            cache,
            files,
        };
        for f in db.files.values_mut() {
            let connection = f.open()?;
            let check: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            if check != "ok" {
                bail!("数据库快照完整性校验失败");
            }
        }
        Ok(db)
    }
    pub fn contact_choices(&mut self) -> Result<Vec<(String, bool)>> {
        let conn = self
            .files
            .get_mut("contact/contact.db")
            .context("contact database missing")?
            .open()?;
        let mut query = conn.prepare("SELECT COALESCE(NULLIF(remark,''),nick_name),username FROM contact WHERE COALESCE(NULLIF(remark,''),nick_name)<>'' ORDER BY COALESCE(NULLIF(remark,''),nick_name)")?;
        let rows = query
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter(|(_, id)| !id.starts_with("gh_") && id != "filehelper")
            .map(|(name, id)| (name, id.ends_with("@chatroom")))
            .collect())
    }
    pub fn peers(&mut self, c: &Config) -> Result<Vec<Peer>> {
        let conn = self
            .files
            .get_mut("contact/contact.db")
            .context("contact database missing")?
            .open()?;
        let mut out = vec![];
        for (name, group) in c.scope() {
            let mut query = conn.prepare(
                "SELECT username FROM contact WHERE COALESCE(NULLIF(remark,''),nick_name)=?1",
            )?;
            let ids = query
                .query_map([&name], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let ids: Vec<_> = ids
                .into_iter()
                .filter(|x| x.ends_with("@chatroom") == group)
                .collect();
            if ids.len() != 1 {
                bail!("会话【{name}】未找到或不唯一，请核对完整备注名");
            }
            out.push(Peer {
                name,
                group,
                wxid: ids[0].clone(),
            });
        }
        Ok(out)
    }
    pub fn nickname(&mut self, wxid: &str) -> String {
        let result = (|| -> Result<String> {
            let c = self.files.get_mut("contact/contact.db").unwrap().open()?;
            Ok(c.query_row("SELECT COALESCE(NULLIF(remark,''),NULLIF(nick_name,''),username) FROM contact WHERE username=?1",[wxid],|r|r.get(0))? )
        })();
        result.unwrap_or_else(|_| wxid.to_owned())
    }
    pub fn messages(&mut self, peer: &str, since: i64) -> Result<Vec<Message>> {
        let table = format!("Msg_{}", hex::encode(Md5::digest(peer.as_bytes())));
        let mut result = vec![];
        for (rel, file) in &mut self.files {
            if !rel.starts_with("message/") {
                continue;
            }
            let conn = file.open()?;
            let exists: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [&table],
                |r| r.get(0),
            )?;
            if exists == 0 {
                continue;
            }
            let mut names = HashMap::new();
            let mut statement = conn.prepare("SELECT rowid,user_name FROM Name2Id")?;
            for row in
                statement.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            {
                let (k, v) = row?;
                names.insert(k, v);
            }
            let direction = if since < 0 { "DESC" } else { "ASC" };
            let mut query=conn.prepare(&format!("SELECT local_id,server_id,sort_seq,create_time,local_type,real_sender_id,message_content,compress_content FROM {table} WHERE sort_seq>=?1 ORDER BY sort_seq {direction},local_id {direction} LIMIT 1000"))?;
            let rows = query.query_map([since.max(0)], |r| {
                let sender = names
                    .get(&r.get::<_, i64>(5).unwrap_or(0))
                    .cloned()
                    .unwrap_or_default();
                let raw = bytes(r.get_ref(6)?);
                let extra = bytes(r.get_ref(7)?);
                let mut content = decode(&raw);
                if content.is_empty() {
                    content = decode(&extra);
                }
                let prefix = format!("{sender}:\n");
                if !sender.is_empty() && content.starts_with(&prefix) {
                    content = content[prefix.len()..].to_owned();
                }
                Ok(Message {
                    local_id: r.get(0)?,
                    server_id: r.get::<_, i64>(1).unwrap_or(0),
                    sort_seq: r.get(2)?,
                    create_time: r.get(3)?,
                    kind: r.get::<_, i64>(4)? & 0xffffffff,
                    sender,
                    content,
                    shard: rel.replace('/', "__"),
                })
            })?;
            for row in rows {
                result.push(row?);
            }
        }
        result.sort_by_key(|m| (m.sort_seq, m.shard.clone(), m.local_id));
        Ok(result)
    }
}
fn bytes(v: ValueRef<'_>) -> Vec<u8> {
    match v {
        ValueRef::Text(v) | ValueRef::Blob(v) => v.to_vec(),
        _ => vec![],
    }
}
fn decode(data: &[u8]) -> String {
    if data.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        let mut d = zstd::stream::read::Decoder::new(data)
            .ok()
            .map(|d| d.take(2 * 1024 * 1024));
        if let Some(ref mut d) = d {
            let mut out = vec![];
            if d.read_to_end(&mut out).is_ok() {
                return String::from_utf8_lossy(&out).to_string();
            }
        }
    }
    String::from_utf8(data.to_vec()).unwrap_or_default()
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Watermark {
    seq: i64,
    seen: HashSet<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Cursors {
    #[serde(default)]
    watermarks: HashMap<String, Watermark>,
    #[serde(default)]
    member_ids: HashMap<String, HashMap<String, u64>>,
}
pub fn run(shared: Arc<Shared>) -> Result<()> {
    let initial = shared.config.read().unwrap().clone();
    let startup = config::now() as i64;
    let start = Instant::now();
    let mut db = Database::new(&initial, &shared.data)?;
    *shared.bound_root.write().unwrap() = Some(db.root.clone());
    let cursor_file = db.cache.join("cursors.json");
    let mut cursor: Cursors = config::load(&cursor_file)
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let mut peers = db.peers(&initial)?;
    config::atomic(
        &shared.data.join("contacts.json"),
        &serde_json::json!({"binding":initial.raw["wechat_binding"],"items":db.contact_choices()?}),
    )?;
    if let Ok(staging) = db.staging(&initial) {
        *shared.staging.write().unwrap() = Some(staging.clone());
        if !peers.iter().any(|p| p.wxid == staging.wxid) {
            peers.push(staging);
        }
    }
    let mut names = HashMap::new();
    let mut generation = shared.generation.load(Ordering::SeqCst);
    let mut failures = 0;
    for peer in &peers {
        baseline(&mut db, &mut cursor, peer, startup, &shared)?;
        // A prior process may have timed out just before a recall appeared in
        // SQLite. Reconcile known physical identities without issuing actions.
        let binding = initial.binding()?;
        let known: Vec<Message> = shared
            .ledger
            .lock()
            .unwrap()
            .sent
            .values()
            .filter(|r| {
                r.name == peer.name
                    && !r.recalled
                    && r.binding.pid == binding.pid
                    && (r.binding.created_at - binding.created_at).abs() < 0.01
            })
            .flat_map(|r| r.messages.iter())
            .filter(|m| m.native_cgi.is_none() && !m.was_recalled())
            .map(|m| m.row.clone())
            .collect();
        if let Some(low) = known.iter().map(|m| m.sort_seq).min() {
            for row in db.messages(&peer.wxid, low)? {
                if known.iter().any(|m| m.key() == row.key()) {
                    shared.observe(&peer.name, &row);
                }
            }
        }
    }
    config::atomic(&cursor_file, &serde_json::to_value(&cursor)?)?;
    *shared.peers.write().unwrap() = peers.clone();
    shared.ready.store(true, Ordering::SeqCst);
    shared.log(&format!(
        "Rust 数据库接收就绪：{} 个会话，初始化 {:.2}s",
        peers.len(),
        start.elapsed().as_secs_f64()
    ));
    while !shared.stop.load(Ordering::Relaxed) {
        let c = shared.config.read().unwrap().clone();
        let started = Instant::now();
        let step = (|| -> Result<()> {
            let bound = Process::bound(&c.binding()?)?;
            if shared.generation.load(Ordering::SeqCst) != generation {
                let mut revised = db.peers(&c)?;
                if let Ok(staging) = db.staging(&c) {
                    *shared.staging.write().unwrap() = Some(staging.clone());
                    if !revised.iter().any(|p| p.wxid == staging.wxid) {
                        revised.push(staging);
                    }
                } else {
                    *shared.staging.write().unwrap() = None;
                }
                for p in &revised {
                    if !peers
                        .iter()
                        .any(|old| old.name == p.name && old.group == p.group)
                    {
                        baseline(&mut db, &mut cursor, p, config::now() as i64, &shared)?;
                    }
                }
                peers = revised;
                *shared.peers.write().unwrap() = peers.clone();
                generation = shared.generation.load(Ordering::SeqCst);
            }
            // Revalidate the bound account once per second, independent of reception.
            if shared.account_checked.load(Ordering::Relaxed) + 1000
                < (config::now() * 1000.) as u64
            {
                let current = bound.account_root()?;
                if !same_root(&current, &db.root) {
                    bail!("微信登录账号已改变，请重新绑定；未切换账号");
                }
                shared
                    .account_checked
                    .store((config::now() * 1000.) as u64, Ordering::Relaxed);
            }
            let mut changed = false;
            for peer in &peers {
                let high = cursor.watermarks.get(&peer.wxid).map_or(0, |m| m.seq);
                let rows = db.messages(&peer.wxid, high.saturating_sub(1))?;
                for row in &rows {
                    note_view(&mut db, &shared, peer, row, &mut names);
                    if row.sender != db.wxid {
                        crate::music::remember(&shared, peer, row);
                    }
                    if row.sender == db.wxid {
                        shared.observe(&peer.name, row);
                    }
                }
                let marker = cursor.watermarks.entry(peer.wxid.clone()).or_default();
                for row in rows {
                    let key = row.key();
                    if row.sort_seq < marker.seq
                        || (row.sort_seq == marker.seq && marker.seen.contains(&key))
                    {
                        continue;
                    }
                    if row.kind == 1
                        && !row.sender.is_empty()
                        && row.sender != db.wxid
                        && !row.content.trim().is_empty()
                    {
                        if !shared.ready.load(Ordering::SeqCst) {
                            break;
                        }
                        let label = names
                            .entry(row.sender.clone())
                            .or_insert_with(|| db.nickname(&row.sender))
                            .clone();
                        let uid = if peer.group {
                            let members = cursor.member_ids.entry(peer.name.clone()).or_default();
                            if let Some(&id) = members.get(&row.sender) {
                                id
                            } else {
                                let mut id = config::member_id(&peer.name, &label);
                                if members.values().any(|&v| v == id) {
                                    id = config::member_id(&peer.name, &row.sender);
                                }
                                members.insert(row.sender.clone(), id);
                                id
                            }
                        } else {
                            config::contact_id(&peer.name)
                        };
                        let id = config::contact_id(&format!(
                            "wechat-source:{}:{}:{}",
                            db.identity, peer.wxid, key
                        ));
                        if !shared.publish(peer, &label, uid, &row, id) {
                            break;
                        }
                        shared.received.fetch_add(1, Ordering::Relaxed);
                    }
                    if row.sort_seq > marker.seq {
                        marker.seq = row.sort_seq;
                        marker.seen.clear();
                    }
                    marker.seen.insert(key);
                    changed = true;
                }
                // Pending local sends can receive their server id after the high-water
                // mark moved. Re-read their exact rows rather than treating id=0 as sent.
                let mut pending = shared.pending(&peer.name);
                if let Some(watch) = shared.recall_watch.lock().unwrap().get_mut(&peer.name) {
                    watch.retain(|row| config::now() - (row.create_time as f64) < 180.);
                    pending.extend(watch.clone());
                }
                if let Some(low) = pending.iter().map(|m| m.sort_seq).min() {
                    for row in db.messages(&peer.wxid, low)? {
                        note_view(&mut db, &shared, peer, &row, &mut names);
                        if row.sender == db.wxid || pending.iter().any(|m| m.key() == row.key()) {
                            shared.observe(&peer.name, &row);
                        }
                    }
                }
            }
            if changed {
                config::atomic(&cursor_file, &serde_json::to_value(&cursor)?)?;
            }
            Ok(())
        })();
        match step {
            Ok(()) => {
                failures = 0;
                shared.set_error("");
                shared.ready.store(true, Ordering::SeqCst);
            }
            Err(e) => {
                failures += 1;
                shared.ready.store(false, Ordering::SeqCst);
                shared.set_error(&format!("数据库接收：{e}"));
                // WAL/checkpoint and SQLite locks are transient. Preserve
                // cursors and retry without abandoning reception permanently.
            }
        }
        shared
            .poll_us
            .store(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        let delay = Duration::from_secs_f64(if failures == 0 {
            c.interval()
        } else {
            (c.interval() * failures as f64).min(2.)
        });
        std::thread::sleep(delay);
    }
    Ok(())
}
fn baseline(
    db: &mut Database,
    cursor: &mut Cursors,
    peer: &Peer,
    cutoff: i64,
    shared: &Shared,
) -> Result<()> {
    let rows = db.messages(&peer.wxid, -1)?;
    let mut view_names = HashMap::new();
    for row in &rows {
        note_view(db, shared, peer, row, &mut view_names);
        if row.sender != db.wxid {
            crate::music::remember(shared, peer, row);
        }
    }
    let recent = rows
        .iter()
        .rev()
        .filter(|r| r.kind == 1)
        .take(15)
        .collect::<Vec<_>>();
    for row in recent.into_iter().rev() {
        let sender = if row.sender == db.wxid {
            "本账号".to_owned()
        } else if peer.group {
            db.nickname(&row.sender)
        } else {
            peer.name.clone()
        };
        shared.event("history",&serde_json::json!({"session":peer.name,"group":peer.group,"sender":sender,"source_time":row.create_time,"text":row.content,"source_id":row.local_id}));
    }
    for row in &rows {
        if row.sender == db.wxid {
            shared.observe(&peer.name, row);
        }
    }
    if cursor.watermarks.contains_key(&peer.wxid) {
        return Ok(());
    }
    let mut marker = Watermark::default();
    for row in rows {
        if row.create_time < cutoff {
            if row.sort_seq > marker.seq {
                marker.seq = row.sort_seq;
                marker.seen.clear();
            }
            if row.sort_seq == marker.seq {
                marker.seen.insert(row.key());
            }
        }
    }
    cursor.watermarks.insert(peer.wxid.clone(), marker);
    Ok(())
}
pub fn same_root(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .eq_ignore_ascii_case(b.to_string_lossy().trim_start_matches(r"\\?\"))
}
fn note_view(
    db: &mut Database,
    shared: &Shared,
    peer: &Peer,
    row: &Message,
    names: &mut HashMap<String, String>,
) {
    let own = row.sender == db.wxid;
    let nick = if own || peer.group {
        names
            .entry(row.sender.clone())
            .or_insert_with(|| db.nickname(&row.sender))
            .clone()
    } else {
        peer.name.clone()
    };
    crate::message_view::remember(shared, &peer.name, row, own, &nick);
}
#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncryptMut;
    #[test]
    fn cipher_page_roundtrip() {
        let key = [8u8; 32];
        let iv = [4u8; 16];
        let plain = vec![7u8; 4000];
        let cipher = cbc::Encryptor::<aes::Aes256>::new_from_slices(&key, &iv).unwrap();
        let enc = cipher.encrypt_padded_vec_mut::<NoPadding>(&plain);
        let mut page = vec![9u8; 16];
        page.extend(enc);
        page.extend(iv);
        page.resize(4096, 0);
        let mut mac_key = [0; 32];
        pbkdf2::pbkdf2_hmac::<Sha512>(&key, &vec![9 ^ 0x3a; 16], 2, &mut mac_key);
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(&mac_key).unwrap();
        mac.update(&page[16..4032]);
        mac.update(&1u32.to_le_bytes());
        page[4032..].copy_from_slice(&mac.finalize().into_bytes());
        assert!(verify_key(&key, &page));
        assert!(!verify_key(&[0; 32], &page));
        let decoded = decrypt_page(&key, &page, 1).unwrap();
        assert_eq!(&decoded[..16], b"SQLite format 3\0");
        assert_eq!(&decoded[16..4016], plain);
    }
    #[test]
    fn zstd_preserves_case_and_newlines() {
        let s = "ccc1\nCCC1";
        let z = zstd::encode_all(s.as_bytes(), 1).unwrap();
        assert_eq!(decode(&z), s);
    }
    #[test]
    fn wal_applies_only_committed_frames_and_rebuilds_after_checkpoint() {
        fn encrypted(key: &[u8], first: bool, value: u8) -> Vec<u8> {
            let start = if first { 16 } else { 0 };
            let iv = [4u8; 16];
            let mut page = vec![9u8; start];
            let cipher = cbc::Encryptor::<aes::Aes256>::new_from_slices(key, &iv).unwrap();
            page.extend(
                cipher.encrypt_padded_vec_mut::<NoPadding>(&vec![value; PAGE - RESERVE - start]),
            );
            page.extend(iv);
            page.resize(PAGE, 0);
            page
        }
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("source.db");
        let target = d.path().join("plain.db");
        let walpath = d.path().join("source.db-wal");
        let key = vec![8u8; 32];
        fs::write(&source, encrypted(&key, true, 1)).unwrap();
        let mut wal = vec![0u8; 32];
        wal[16..24].copy_from_slice(&[3; 8]);
        let append = |wal: &mut Vec<u8>, commit: u32, value: u8| {
            let mut frame = vec![0u8; 24];
            frame[..4].copy_from_slice(&2u32.to_be_bytes());
            frame[4..8].copy_from_slice(&commit.to_be_bytes());
            frame[8..16].copy_from_slice(&[3; 8]);
            wal.extend(frame);
            wal.extend(encrypted(&key, false, value));
        };
        append(&mut wal, 0, 20);
        fs::write(&walpath, &wal).unwrap();
        let mut snapshot = Snapshot {
            source: source.clone(),
            target: target.clone(),
            key: key.clone(),
            base: None,
            wal: None,
            salt: [0; 8],
            frames: 0,
        };
        snapshot.update().unwrap();
        assert_eq!(fs::metadata(&target).unwrap().len(), PAGE as u64);
        assert_eq!(snapshot.frames, 0);
        append(&mut wal, 2, 31);
        fs::write(&walpath, &wal).unwrap();
        snapshot.update().unwrap();
        let plain = fs::read(&target).unwrap();
        assert_eq!(plain.len(), PAGE * 2);
        assert_eq!(plain[PAGE], 31);
        assert_eq!(snapshot.frames, 2);
        wal.extend([0u8; 10]);
        fs::write(&walpath, &wal).unwrap();
        snapshot.update().unwrap();
        assert_eq!(snapshot.frames, 2);
        fs::write(&source, encrypted(&key, true, 7)).unwrap();
        fs::write(&walpath, []).unwrap();
        snapshot.update().unwrap();
        assert_eq!(fs::metadata(&target).unwrap().len(), PAGE as u64);
        assert_eq!(snapshot.frames, 0);
        assert_eq!(fs::read(&target).unwrap()[16], 7);
    }
}
