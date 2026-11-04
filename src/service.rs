use crate::{
    config::{self, Config},
    database::{Message, Peer},
    native,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
pub struct Client {
    pub id: u64,
    pub tx: tokio::sync::mpsc::Sender<Value>,
}
#[derive(Clone)]
pub struct Observed {
    pub message: Message,
    pub serial: u64,
    pub at: Instant,
}
#[derive(Default)]
pub struct Own {
    pub serial: u64,
    pub rows: HashMap<String, VecDeque<Observed>>,
}
pub struct Shared {
    pub config: RwLock<Config>,
    pub data: PathBuf,
    pub client: Mutex<Option<Client>>,
    pub peers: RwLock<Vec<Peer>>,
    pub members: Mutex<HashMap<u64, HashMap<u64, Value>>>,
    pub ready: AtomicBool,
    pub stop: AtomicBool,
    pub generation: AtomicU64,
    pub received: AtomicU64,
    pub poll_us: AtomicU64,
    pub account_checked: AtomicU64,
    pub last_send_us: AtomicU64,
    pub error: Mutex<String>,
    pub own: Mutex<Own>,
    pub changed: Condvar,
    pub queue: crossbeam_channel::Sender<crate::bridge::Job>,
    pub images: Mutex<VecDeque<crate::delivery::CachedImage>>,
    pub music_cache: RwLock<Vec<crate::music::NativeCard>>,
    pub timeline: Mutex<crate::message_view::Timelines>,
    pub bound_root: RwLock<Option<PathBuf>>,
    pub ledger: Mutex<crate::actions::Ledger>,
    pub recall_watch: Mutex<std::collections::HashMap<String, Vec<Message>>>,
    pub staging: RwLock<Option<Peer>>,
    pub created_at: f64,
    pub logging: Mutex<()>,
    pub outbox: Mutex<Option<crate::outbox::Store>>,
}
impl Shared {
    pub fn new(c: Config) -> (Arc<Self>, crossbeam_channel::Receiver<crate::bridge::Job>) {
        let (tx, rx) = crossbeam_channel::bounded(128);
        let data = c.path.parent().unwrap().join("data");
        let ledger = config::load(&data.join("messages.json"))
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        (
            Arc::new(Self {
                config: RwLock::new(c),
                data,
                client: Mutex::new(None),
                peers: RwLock::new(vec![]),
                members: Mutex::new(HashMap::new()),
                ready: AtomicBool::new(false),
                stop: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                received: AtomicU64::new(0),
                poll_us: AtomicU64::new(0),
                account_checked: AtomicU64::new(0),
                last_send_us: AtomicU64::new(0),
                error: Mutex::new(String::new()),
                own: Mutex::new(Own::default()),
                changed: Condvar::new(),
                queue: tx,
                images: Mutex::new(VecDeque::new()),
                music_cache: RwLock::new(Vec::new()),
                timeline: Mutex::new(Default::default()),
                bound_root: RwLock::new(None),
                ledger: Mutex::new(ledger),
                recall_watch: Mutex::new(HashMap::new()),
                staging: RwLock::new(None),
                created_at: native::Process::open(std::process::id())
                    .and_then(|p| p.creation())
                    .unwrap_or(0.),
                logging: Mutex::new(()),
                outbox: Mutex::new(None),
            }),
            rx,
        )
    }
    pub fn connected(&self) -> bool {
        self.client.lock().unwrap().is_some()
    }
    pub fn owner(&self, id: u64) -> bool {
        if id == 0 {
            return true;
        }
        self.client
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|c| c.id == id)
    }
    pub fn log(&self, text: &str) {
        let _ = fs::create_dir_all(&self.data);
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.data.join("runtime.log"))
        {
            let t = config::now();
            let _ = writeln!(f, "[{t:.3}] {text}");
        }
        self.event("runtime", &json!({"text":text}));
    }
    pub fn event(&self, kind: &str, detail: &Value) {
        let token = self.config.read().unwrap().token().to_owned();
        let _guard = self.logging.lock().unwrap();
        crate::telemetry::append(&self.data, kind, detail, &token);
    }
    pub fn set_error(&self, text: &str) {
        let mut old = self.error.lock().unwrap();
        if *old != text && !text.is_empty() {
            self.log(text);
        }
        *old = text.to_owned();
    }
    pub fn publish(&self, peer: &Peer, label: &str, uid: u64, row: &Message, id: u64) -> bool {
        let c = self.config.read().unwrap();
        if !c.allows(&peer.name, peer.group) {
            return false;
        }
        let mut event = json!({"time":row.create_time,"self_id":c.bot(),"post_type":"message","message_type":if peer.group{"group"}else{"private"},"sub_type":if peer.group{"normal"}else{"friend"},"message_id":id,"user_id":uid,"font":0,"message":[{"type":"text","data":{"text":row.content}}],"raw_message":row.content,"sender":{"user_id":uid,"nickname":if peer.group{label}else{&peer.name},"sex":"unknown","age":0}});
        if peer.group {
            let gid = config::group_id(&peer.name);
            event["group_id"] = gid.into();
            event["sender"]["role"] = "member".into();
            self.members.lock().unwrap().entry(gid).or_default().insert(uid,json!({"group_id":gid,"user_id":uid,"nickname":label,"card":"","role":"member","sex":"unknown","age":0,"area":"","join_time":0,"last_sent_time":row.create_time,"level":"0","unfriendly":false,"title":"","title_expire_time":0,"card_changeable":false}));
        }
        drop(c);
        let delivered = {
            let guard = self.client.lock().unwrap();
            let Some(client) = guard.as_ref() else {
                return false;
            };
            client.tx.try_send(event).is_ok()
        };
        if delivered {
            self.event("incoming",&json!({"session":peer.name,"group":peer.group,"sender":label,"user_id":uid,"message_id":id,"text":row.content}));
        }
        delivered
    }
    pub fn observe(&self, name: &str, message: &Message) {
        if let Err(e) = crate::actions::confirm_database_recall(self, name, message) {
            self.log(&format!("保存数据库撤回确认失败：{e}"));
        }
        let mut own = self.own.lock().unwrap();
        let old = own
            .rows
            .get(name)
            .and_then(|v| v.iter().find(|m| m.message.key() == message.key()))
            .cloned();
        if let Some(old) = old {
            if old.message.server_id == message.server_id
                && old.message.kind == message.kind
                && old.message.content == message.content
            {
                return;
            }
            if let Some(row) = own
                .rows
                .get_mut(name)
                .and_then(|v| v.iter_mut().find(|m| m.message.key() == message.key()))
            {
                row.message = message.clone();
            }
        } else {
            own.serial += 1;
            let row = Observed {
                message: message.clone(),
                serial: own.serial,
                at: Instant::now(),
            };
            let rows = own.rows.entry(name.to_owned()).or_default();
            rows.push_back(row);
            while rows.len() > 128 {
                rows.pop_front();
            }
        }
        self.changed.notify_all();
    }
    pub fn pending(&self, name: &str) -> Vec<Message> {
        self.own
            .lock()
            .unwrap()
            .rows
            .get(name)
            .map_or(vec![], |rows| {
                rows.iter()
                    .filter(|r| {
                        r.message.server_id == 0 && r.at.elapsed() < Duration::from_secs(15)
                    })
                    .map(|r| r.message.clone())
                    .collect()
            })
    }
    pub fn checkpoint(&self) -> u64 {
        self.own.lock().unwrap().serial
    }
    pub fn confirm(&self, name: &str, kind: i64, text: Option<&str>, before: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut own = self.own.lock().unwrap();
        let expected = text.map(normalize);
        loop {
            if own.rows.get(name).is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.serial > before
                        && row.message.kind == kind
                        && row.message.server_id > 0
                        && expected
                            .as_ref()
                            .is_none_or(|x| *x == normalize(&row.message.content))
                })
            }) {
                return true;
            }
            if Instant::now() >= deadline
                || self.stop.load(Ordering::Relaxed)
                || !self.ready.load(Ordering::SeqCst)
            {
                return false;
            }
            own = self
                .changed
                .wait_timeout(own, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }
    pub fn state(&self) -> Value {
        json!({"pid":std::process::id(),"created_at":self.created_at,"updated_at":config::now(),"engine":"rust","version":env!("CARGO_PKG_VERSION"),"running":true,"connected":self.connected(),"ready":self.ready.load(Ordering::SeqCst),"backend":"database","metrics":{"poll_ms":self.poll_us.load(Ordering::Relaxed) as f64/1000.,"received":self.received.load(Ordering::Relaxed),"last_send_ms":self.last_send_us.load(Ordering::Relaxed) as f64/1000.},"outbox":crate::outbox::summary(self).unwrap_or_default(),"error":self.error.lock().unwrap().clone()})
    }
}
fn normalize(s: &str) -> String {
    s.chars().filter(|x| !x.is_whitespace()).collect()
}
fn native_test(config: Config, mode: &str, report: Option<String>) -> Result<()> {
    if ![
        "send-and-recall",
        "forward",
        "forward-and-recall",
        "image-and-recall",
        "voice-and-recall",
        "scheduled-recall",
        "locate-only",
        "virtualized-recall",
        "fresh-chat-and-recall",
    ]
    .contains(&mode)
    {
        bail!("Unknown native-test mode");
    }
    let name = config.raw["native_test_target"]
        .as_str()
        .context("请显式设置 native_test_target 测试目标")?
        .to_owned();
    let (_, group) = config
        .scope()
        .into_iter()
        .find(|(n, _)| *n == name)
        .ok_or_else(|| anyhow::anyhow!("测试目标不在白名单"))?;
    let (shared, _) = Shared::new(config.clone());
    let receiver = shared.clone();
    let worker = std::thread::spawn(move || {
        if let Err(e) = crate::database::run(receiver.clone()) {
            receiver.ready.store(false, Ordering::SeqCst);
            receiver.set_error(&format!("{e}"));
        }
    });
    let mut input_before = None;
    let mut initial_chat_title = None;
    let mut operation_guard = None;
    let mut input_phases = Vec::new();
    let snapshot = |phase: &str| {
        let input = native::input_snapshot();
        let mut pid = 0;
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                windows::Win32::Foundation::HWND(input.foreground as *mut _),
                Some(&mut pid),
            );
        }
        json!({"phase":phase,"foreground":input.foreground,"foreground_pid":pid,"focus":input.focus,"cursor":input.cursor,"clipboard":input.clipboard})
    };
    let result = (|| -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(35);
        while !shared.ready.load(Ordering::SeqCst) {
            if !shared.error.lock().unwrap().is_empty() {
                bail!("{}", shared.error.lock().unwrap());
            }
            if Instant::now() >= deadline {
                bail!("数据库未就绪");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let automation = crate::delivery::Automation::new()?;
        input_before = Some(native::input_snapshot());
        input_phases.push(snapshot("before_preparation"));
        operation_guard = Some(crate::popup::Guard::install(
            native::validate_window(&config.binding()?)?,
            &shared.data,
        )?);
        if mode == "fresh-chat-and-recall" {
            let h = native::validate_window(&config.binding()?)?;
            crate::friends::tab(&automation, h, "通讯录")?;
            initial_chat_title = Some(automation.title(h).is_ok());
            input_phases.push(snapshot("after_contacts"));
        }
        if mode == "locate-only" {
            let args = std::env::args().collect::<Vec<_>>();
            let key = args
                .iter()
                .position(|s| s == "--message-key")
                .and_then(|i| args.get(i + 1))
                .context("--message-key required")?;
            let h = native::validate_window(&config.binding()?)?;
            input_before = Some(native::input_snapshot());
            let _guard = crate::popup::Guard::install(h, &shared.data)?;
            automation.select(h, &name, group)?;
            let _row = automation.locate_message(&shared, h, &name, key)?;
            return Ok(
                json!({"mode":mode,"target":name,"message_key":key,"aligned":true,"messages_sent":0,"messages_recalled":0}),
            );
        }
        let id = (config::now() * 1000.) as u64;
        let (tx, _) = std::sync::mpsc::sync_channel(1);
        let started = Instant::now();
        let (operation, parts) = if mode == "virtualized-recall" {
            let mut png = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(24, 24).write_to(&mut png, image::ImageFormat::Png)?;
            let long = (0..100)
                .map(|i| format!("行{i:03} - 列表虚拟化验证 {id}\n"))
                .collect::<String>();
            (
                crate::actions::Operation::Send,
                vec![
                    crate::delivery::Part::Text(format!("整批撤回验证 A ccc1 {id}")),
                    crate::delivery::Part::Text(format!("整批撤回验证 B CCC1 {id}")),
                    crate::delivery::Part::Image(png.into_inner()),
                    crate::delivery::Part::Text(long),
                ],
            )
        } else if mode.starts_with("forward") {
            let args = std::env::args().collect::<Vec<_>>();
            let file = args
                .iter()
                .position(|s| s == "--nodes-file")
                .and_then(|i| args.get(i + 1));
            let data = if let Some(file) = file {
                config::load(std::path::Path::new(file))?
            } else {
                config.raw.get("native_test_nodes").cloned().unwrap_or_else(||json!([{"type":"node","data":{"name":"Rust 原生转发测试","uin":"1","content":[{"type":"text","data":{"text":"原生合并转发验证 AbC\nmagnet:?xt=urn:btih:0123456789ABCDEF0123456789ABCDEF01234567"}}]}},{"type":"node","data":{"nickname":"第二节点","user_id":"2","content":"大小写验证 ccc1 / CCC1"}}]))
            };
            let nodes = crate::actions::nodes(&data, &shared)?;
            let parts = crate::actions::forward_parts(&nodes, &shared)?;
            (crate::actions::Operation::Forward(nodes), parts)
        } else if mode == "image-and-recall" || mode == "voice-and-recall" {
            let message = config
                .raw
                .get("native_test_message")
                .context("Missing native_test_message")?;
            (
                crate::actions::Operation::Send,
                crate::delivery::prepare(message, false, &shared)?,
            )
        } else {
            (
                crate::actions::Operation::Send,
                vec![crate::delivery::Part::Text(format!(
                    "Rust 原生发送与撤回验证 AbC {id}"
                ))],
            )
        };
        let job = crate::bridge::Job {
            name: name.clone(),
            group,
            owner: 0,
            id,
            parts,
            operation,
            deadline: Instant::now()
                + Duration::from_secs(if mode.starts_with("forward") { 120 } else { 25 }),
            state: Arc::new(std::sync::atomic::AtomicU8::new(0)),
            reply: tx,
        };
        input_before = Some(native::input_snapshot());
        input_phases.push(snapshot("before_send"));
        automation.send(&shared, &job)?;
        input_phases.push(snapshot("after_send"));
        let sent = started.elapsed().as_secs_f64();
        let h = native::validate_window(&config.binding()?)?;
        // A diagnostic screenshot must not prevent cleanup of a confirmed
        // test message on a remote desktop with unavailable composition.
        let _ = native::capture(h).and_then(|image| {
            image.save(shared.data.join(format!("native-test-{mode}.png")))?;
            Ok(())
        });
        input_phases.push(snapshot("after_capture"));
        if mode == "voice-and-recall" {
            let hold = config.raw["native_test_hold_seconds"]
                .as_u64()
                .unwrap_or(0)
                .min(60);
            std::thread::sleep(Duration::from_secs(hold));
        }
        if mode == "scheduled-recall" {
            let delay = config.raw["native_test_delay_ms"].as_f64().unwrap_or(5000.);
            crate::actions::schedule(&shared, id, delay)?;
            let deadline = Instant::now() + Duration::from_secs(125);
            while crate::actions::due_recall(&shared) != Some(id) {
                if Instant::now() >= deadline {
                    bail!("计划撤回未在两分钟内到期");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        if mode.ends_with("recall") {
            if mode == "virtualized-recall" {
                let mut ledger = shared.ledger.lock().unwrap();
                for tracked in &mut ledger
                    .sent
                    .get_mut(&id)
                    .context("Missing test record")?
                    .messages
                {
                    tracked.runtime_id.clear();
                }
            }
            automation.recall(&shared, id)?;
            input_phases.push(snapshot("after_recall"));
        }
        Ok(
            json!({"mode":mode,"target":name,"group":group,"message_id":id,"send_seconds":sent,"total_seconds":started.elapsed().as_secs_f64(),"confirmation":crate::delivery::confirmation(&job.parts),"database_confirmed":crate::delivery::confirmation(&job.parts)=="database","native_server_confirmed":crate::delivery::confirmation(&job.parts)!="database","recall_confirmed":mode.ends_with("recall"),"initial_chat_title_available":initial_chat_title}),
        )
    })();
    if result.is_err() {
        if let Ok(binding) = config.binding() {
            if let Ok(h) = native::validate_window(&binding) {
                if let Ok(automation) = crate::delivery::Automation::new() {
                    automation.cancel_actions(h, &shared.data);
                }
            }
        }
    }
    shared.stop.store(true, Ordering::SeqCst);
    let _ = worker.join();
    input_phases.push(snapshot("after_database_shutdown"));
    let mut value = match &result {
        Ok(v) => v.clone(),
        Err(e) => json!({"mode":mode,"success":false,"error":format!("{e:#}")}),
    };
    if let Some(before) = input_before {
        let after = native::input_snapshot();
        let mut foreground_pid = 0;
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                windows::Win32::Foundation::HWND(after.foreground as *mut _),
                Some(&mut foreground_pid),
            );
        }
        value["foreground_after_pid"] = foreground_pid.into();
        value["foreground_after_is_bound_wechat"] =
            (config.binding().is_ok_and(|b| b.pid == foreground_pid)).into();
        value["foreground_unchanged"] = (before.foreground == after.foreground).into();
        value["focus_unchanged"] = (before.focus == after.focus).into();
        value["cursor_unchanged"] = (before.cursor == after.cursor).into();
        value["clipboard_unchanged"] = (before.clipboard == after.clipboard).into();
    }
    value["input_phases"] = input_phases.into();
    if let Some(report) = report {
        config::atomic(std::path::Path::new(&report), &value)?;
    }
    println!("{value}");
    result.map(|_| ())
}
pub fn entry() -> Result<()> {
    native::dpi();
    let args: Vec<_> = std::env::args().collect();
    let option = |key: &str| {
        args.iter()
            .position(|s| s == key)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let path = option("--config")
        .map(PathBuf::from)
        .unwrap_or_else(|| config::base().join("config.json"));
    if args.iter().any(|s| s == "--licenses") {
        let output = config::base().join("THIRD_PARTY_LICENSES.txt");
        fs::write(&output, include_str!("../assets/THIRD_PARTY_LICENSES.txt"))?;
        println!("许可证已导出：{}", output.display());
        return Ok(());
    }
    if let Some(source) = option("--import-python") {
        let c = Config::read(PathBuf::from(&source))?;
        config::atomic(&path, &c.raw)?;
        let from = PathBuf::from(source).parent().unwrap().join(".wechat-db");
        if from.is_dir() {
            for account in fs::read_dir(from)? {
                let account = account?.path();
                let cursor = account.join("cursors.json");
                if cursor.exists() {
                    let target = path
                        .parent()
                        .unwrap()
                        .join("data/db")
                        .join(account.file_name().unwrap())
                        .join("cursors.json");
                    config::atomic(&target, &config::load(&cursor)?)?;
                }
            }
        }
        println!("已导入配置和消息游标；原版本未修改");
        return Ok(());
    }
    if !path.exists() {
        config::atomic(&path, &Config::default(path.clone()).raw)?;
    }
    let config = Config::read(path)?;
    if args.iter().any(|s| s == "--diagnose") {
        let report = crate::diagnostics::collect(&config);
        let destination = option("--report").map(PathBuf::from).unwrap_or_else(|| {
            config
                .path
                .parent()
                .unwrap()
                .join("wechat-diagnostics.json")
        });
        config::atomic(&destination, &report)?;
        println!("诊断完成；报告不含令牌、聊天正文或联系人姓名。");
        return Ok(());
    }
    if args.iter().any(|s| s == "--music-backend-probe") {
        let value = crate::music_backend::probe(&config)?;
        if let Some(path) = option("--report") {
            config::atomic(std::path::Path::new(&path), &value)?;
        }
        println!("{value}");
        return Ok(());
    }
    if args.iter().any(|s| s == "--music-backend-construct") {
        let value = crate::music_backend::rpc(&config, json!({"mode":"construct"}))?;
        if let Some(path) = option("--report") {
            config::atomic(std::path::Path::new(&path), &value)?;
        }
        println!("{value}");
        return Ok(());
    }
    if let Some(reference) = option("--music-self-test") {
        let request = option("--music-request");
        let value = crate::music_backend::self_test(
            &config,
            std::path::Path::new(&reference),
            request.as_deref().map(std::path::Path::new),
            option("--music-test-target").as_deref(),
            option("--music-hold-seconds")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        )?;
        if let Some(path) = option("--report") {
            config::atomic(std::path::Path::new(&path), &value)?;
        }
        println!("{value}");
        return Ok(());
    }
    if args.iter().any(|s| s == "--voice-probe") {
        crate::voice::inspect(&config)?;
        let value = crate::music_backend::rpc(&config, json!({"mode":"voice_construct"}))?;
        if let Some(path) = option("--report") {
            config::atomic(std::path::Path::new(&path), &value)?;
        }
        println!("{value}");
        return Ok(());
    }
    if args.iter().any(|s| s == "--input-state") {
        let binding = config.binding()?;
        let snap = native::input_snapshot();
        let mut pid = 0;
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                windows::Win32::Foundation::HWND(snap.foreground as *mut _),
                Some(&mut pid),
            );
        }
        println!(
            "{}",
            json!({"foreground_is_bound_wechat":pid==binding.pid,"foreground_pid":pid})
        );
        return Ok(());
    }
    if let Some(mode) = option("--native-music-probe") {
        return crate::music_ui::probe(
            &config,
            &mode,
            std::path::Path::new(&option("--report").context("--report required")?),
        );
    }
    if args.iter().any(|s| s == "--resume-friend-requests") {
        crate::outbox::resume_friends(&config.path.parent().unwrap().join("data"))?;
        return Ok(());
    }
    if let Some(reason) = option("--pause-friend-requests") {
        let (shared, _) = Shared::new(config);
        crate::outbox::pause_friends(&shared, &reason)?;
        return Ok(());
    }
    if args.iter().any(|s| s == "--friend-list") {
        return crate::friends::list(
            &config,
            std::path::Path::new(&option("--report").context("--report required")?),
        );
    }
    if args.iter().any(|s| s == "--friend-probe") {
        return crate::friends::probe(
            &config,
            std::path::Path::new(&option("--report").context("--report is required")?),
        );
    }
    if args.iter().any(|s| s == "--friend-scan-once") {
        let mut config = config;
        config.raw["friend_requests_auto_accept"] = true.into();
        let (shared, _) = Shared::new(config);
        let before = native::input_snapshot();
        let result = crate::friends::poll(&shared, &crate::delivery::Automation::new()?);
        let after = native::input_snapshot();
        let v = json!({"accepted":result.as_ref().ok(),"error":result.as_ref().err().map(ToString::to_string),"clipboard_unchanged":before.clipboard==after.clipboard,"cursor_unchanged":before.cursor==after.cursor,"foreground_unchanged":before.foreground==after.foreground,"focus_unchanged":before.focus==after.focus});
        config::atomic(
            std::path::Path::new(&option("--report").context("--report required")?),
            &v,
        )?;
        result?;
        return Ok(());
    }
    if let Some(pid) = option("--rebind-same-account") {
        let pid: u32 = pid.parse()?;
        let candidates: Vec<_> = native::windows()
            .into_iter()
            .filter(|w| w.binding.pid == pid)
            .collect();
        if candidates.len() != 1 {
            bail!("新微信主窗口不唯一，未绑定");
        }
        let data = config.path.parent().unwrap().join("data");
        let identities: Vec<_> = fs::read_dir(data.join("db"))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir() && e.file_name().to_string_lossy().len() == 24)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        if identities.len() != 1 {
            bail!("原账号缓存身份不唯一，未自动绑定");
        }
        let mut revised = config.clone();
        revised.raw["wechat_binding"] = serde_json::to_value(&candidates[0].binding)?;
        let mut db = crate::database::Database::new(&revised, &data.join("rebind-check"))?;
        if db.identity != identities[0] {
            bail!("新窗口不是原账号，未绑定");
        }
        db.peers(&revised)?;
        native::offscreen(native::validate_window(&candidates[0].binding)?)?;
        config::atomic(&config.path, &revised.raw)?;
        crate::lifecycle::note(
            &revised,
            "account_rebound",
            "已验证同一数据库账号并重新绑定，主窗口已离屏",
        );
        println!(
            "{}",
            json!({"bound":true,"same_account":true,"offscreen":true,"pid":pid})
        );
        return Ok(());
    }
    if let Some(path) = option("--capture-settings") {
        return crate::maintenance::capture_settings(&config, std::path::Path::new(&path));
    }
    if args.iter().any(|s| s == "--close-clean-settings") {
        println!(
            "{}",
            serde_json::json!({"closed":crate::maintenance::close_clean_settings(&config)?})
        );
        return Ok(());
    }
    if args.iter().any(|s| s == "--picker-probe") {
        println!(
            "{}",
            crate::delivery::Automation::new()?.file_picker_probe(&config)?
        );
        return Ok(());
    }
    if let Some(mode) = option("--native-test") {
        return native_test(config, &mode, option("--report"));
    }
    if args.iter().any(|s| s == "--restore-native") {
        crate::popup::probe(&config.path.parent().unwrap().join("data"))?;
        crate::native::restore_chat(
            &config.binding()?,
            &config.path.parent().unwrap().join("data"),
        )?;
        println!("{}", crate::delivery::Automation::new()?.inspect(&config)?);
        return Ok(());
    }
    if let Some(path) = option("--capture") {
        crate::native::capture(native::validate_window(&config.binding()?)?)?.save(path)?;
        return Ok(());
    }
    if args.iter().any(|s| s == "--native-inspect") {
        let automation = if args.iter().any(|s| s == "--uia8") {
            crate::delivery::Automation::new8()?
        } else {
            crate::delivery::Automation::new()?
        };
        println!("{}", automation.inspect(&config)?);
        return Ok(());
    }
    if args.iter().any(|s| s == "--native-menu-probe") {
        let result = crate::delivery::Automation::new()?
            .menu_probe(&config, &config.path.parent().unwrap().join("data"))?;
        if let Some(path) = option("--report") {
            config::atomic(std::path::Path::new(&path), &result)?;
        } else {
            println!("{result}");
        }
        return Ok(());
    }
    if args.iter().any(|s| s == "--guard-probe") {
        let result = crate::popup::probe(&config.path.parent().unwrap().join("data"))?;
        if let Some(path) = option("--report") {
            config::atomic(std::path::Path::new(&path), &result)?;
        } else {
            println!("{result}");
        }
        return Ok(());
    }
    if args.iter().any(|s| s == "--validate") {
        config.validate()?;
        println!("配置有效：{} 个会话", config.scope().len());
        return Ok(());
    }
    if args.iter().any(|s| s == "--print-url") {
        println!("{}", config.url());
        return Ok(());
    }
    if args.iter().any(|s| s == "--windows") {
        println!("{}", serde_json::to_string(&native::windows())?);
        return Ok(());
    }
    if args.iter().any(|s| s == "--probe") {
        let start = Instant::now();
        let data = config.path.parent().unwrap().join("data");
        let mut db = crate::database::Database::new(&config, &data)?;
        let peers = db.peers(&config)?;
        let init = start.elapsed().as_secs_f64();
        let t = Instant::now();
        let mut counts = vec![];
        for p in peers {
            let rows = db.messages(&p.wxid, -1)?;
            let tests: Vec<_> = rows
                .iter()
                .filter(|m| ["速度测试A", "速度测试B", "速度测试C"].contains(&m.content.as_str()))
                .map(|m| json!({"text":m.content,"source_id":m.local_id}))
                .collect();
            counts.push(json!({"name":p.name,"group":p.group,"recent_count":rows.len(),"test_messages":tests}));
        }
        println!(
            "{}",
            json!({"engine":"rust","initialization_seconds":init,"poll_ms":t.elapsed().as_secs_f64()*1000.,"peers":counts})
        );
        return Ok(());
    }
    if args.iter().any(|s| s == "--service") {
        run(config)
    } else if std::env::args().any(|s| s == "--native-gui") {
        crate::gui::run(config)
    } else {
        match crate::gui_web::run(config.clone()) {
            Ok(()) => Ok(()),
            Err(error) => {
                eprintln!("WebView2 界面启动失败，回退原生界面：{error:#}");
                crate::gui::run(config)
            }
        }
    }
}
fn run(config: Config) -> Result<()> {
    use windows::{Win32::System::Threading::CreateMutexW, core::PCWSTR};
    let name = config::wide(format!("Local\\WechatRedReplyRust-{}", config.port()));
    let handle = unsafe { CreateMutexW(None, true, PCWSTR(name.as_ptr()))? };
    let _guard = native::Handle(handle);
    if unsafe { windows::Win32::Foundation::GetLastError() }
        == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
    {
        bail!("该端口的 Rust 监听已经运行");
    }
    let (shared, rx) = Shared::new(config.clone());
    fs::create_dir_all(&shared.data)?;
    crate::outbox::recover(&shared)?;
    let mut exit_watches = vec![];
    if let Ok(binding) = config.binding() {
        match crate::lifecycle::ExitWatch::open(binding.pid, Some(binding.created_at), "微信") {
            Ok(w) => exit_watches.push(w),
            Err(e) => shared.log(&format!(
                "启动时绑定的微信进程不可观察：{e}；历史退出码不可追溯，请重新绑定"
            )),
        }
    }
    let args: Vec<_> = std::env::args().collect();
    if let Some(pid) = args
        .iter()
        .position(|s| s == "--gui-pid")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
    {
        if native::Process::open(pid)
            .and_then(|p| p.image())
            .ok()
            .is_some_and(|path| {
                std::env::current_exe().ok().is_some_and(|own| {
                    std::path::PathBuf::from(path).canonicalize().ok() == own.canonicalize().ok()
                })
            })
        {
            if let Ok(w) = crate::lifecycle::ExitWatch::open(pid, None, "设置窗口") {
                exit_watches.push(w);
            }
        }
    }
    crate::lifecycle::note(
        &config,
        "service_start",
        "Rust 监听进程启动；开始只读观察绑定进程退出码",
    );
    let stop = shared.data.join("stop.request");
    let _ = fs::remove_file(&stop);
    let ws = shared.clone();
    let ws_worker = std::thread::spawn(move || {
        if let Err(e) = crate::bridge::serve(ws.clone()) {
            ws.set_error(&format!("OneBot：{e}"));
            ws.stop.store(true, Ordering::SeqCst);
        }
    });
    let db = shared.clone();
    let db_worker = std::thread::spawn(move || {
        if let Err(e) = crate::database::run(db.clone()) {
            db.ready.store(false, Ordering::SeqCst);
            db.set_error(&format!("数据库：{e}"));
        }
    });
    let sender = shared.clone();
    let sender_worker = std::thread::spawn(move || crate::delivery::run(sender, rx));
    shared.log("Rust 独立监听启动（无 Python 依赖）");
    let mut signature = config::load(&config.path)?;
    let mut last = Instant::now();
    while !shared.stop.load(Ordering::Relaxed) && !stop.exists() {
        if last.elapsed() >= Duration::from_millis(500) {
            for watch in &mut exit_watches {
                watch.poll(&shared);
            }
            let _ = config::atomic(&shared.data.join("state.json"), &shared.state());
            last = Instant::now();
            if let Ok(raw) = config::load(&config.path) {
                if raw != signature {
                    let revised = Config {
                        raw: raw.clone(),
                        path: config.path.clone(),
                    };
                    if revised.validate().is_ok() {
                        if revised.r() != config.r()
                            || revised.raw["wechat_binding"] != config.raw["wechat_binding"]
                        {
                            shared.set_error("连接参数或账号绑定已变化，请重启监听以应用");
                        } else {
                            *shared.config.write().unwrap() = revised;
                            shared.generation.fetch_add(1, Ordering::SeqCst);
                            signature = raw;
                            shared.log("白名单及数据库间隔已热更新");
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    shared.stop.store(true, Ordering::SeqCst);
    shared.log("监听正在正常停止：停止请求或服务退出标记已设置");
    shared.changed.notify_all();
    // Let the sender restore drafts and remove native popup guards before
    // terminating the process; keep the database alive until that finishes.
    let _ = sender_worker.join();
    let _ = db_worker.join();
    let _ = ws_worker.join();
    shared.ready.store(false, Ordering::SeqCst);
    config::atomic(
        &shared.data.join("state.json"),
        &json!({"running":false,"connected":false,"ready":false,"updated_at":config::now()}),
    )?;
    Ok(())
}
