//! Version-bound native CGI channel. A probe does not invoke client functions.
use crate::{config::Config, native};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{mem::size_of, path::PathBuf};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            DataExchange::COPYDATASTRUCT, Diagnostics::ToolHelp::*, LibraryLoader::*, Memory::*,
        },
        UI::WindowsAndMessaging::*,
    },
    core::{PCSTR, PCWSTR},
};
const PROFILE_4209: &str = include_str!("../assets/music-cgi-4.2.0.9.json");
const PROFILE_4212: &str = include_str!("../assets/music-cgi-4.2.0.12.json");
fn profile_for(path: &std::path::Path) -> Result<Value> {
    let hash = hex::encode(Sha256::digest(std::fs::read(path)?));
    for raw in [PROFILE_4209, PROFILE_4212] {
        let value: Value = serde_json::from_str(raw)?;
        if value["sha256"].as_str() == Some(hash.as_str()) {
            return Ok(value);
        }
    }
    bail!("当前微信版本不在音乐原生后端的已核对版本中")
}
#[cfg(test)]
mod recall_tests {
    use super::*;
    fn row() -> crate::database::Message {
        crate::database::Message {
            local_id: 42,
            server_id: 9_123_456_789_012_345_678,
            sort_seq: 1,
            create_time: 1_700_000_000,
            kind: 3,
            sender: "wxid_self".into(),
            content: "image".into(),
            shard: "test".into(),
        }
    }
    #[test]
    fn database_revoke_preserves_full_server_identity_and_does_not_forge_a_database_change() {
        let image = row();
        let receipt = database_recall_receipt("wxid_self", "12345@chatroom", &image).unwrap();
        assert_eq!(receipt.server_id, "9123456789012345678");
        assert_eq!(receipt.create_time, 1_700_000_000);
        assert_eq!(receipt.to, "12345@chatroom");
        assert!(!receipt.recalled);
        assert_eq!(image.kind, 3);
        let mut wrong = row();
        wrong.sender = "wxid_other".into();
        assert!(database_recall_receipt("wxid_self", "wxid_peer", &wrong).is_err());
        wrong = row();
        wrong.server_id = 0;
        assert!(database_recall_receipt("wxid_self", "wxid_peer", &wrong).is_err());
        wrong = row();
        wrong.kind = 10000;
        assert!(database_recall_receipt("wxid_self", "wxid_peer", &wrong).is_err());
    }
    #[test]
    fn persisted_native_confirmation_survives_restart_and_must_match_the_server_id() {
        let d = tempfile::tempdir().unwrap();
        let c = Config::default(d.path().join("config.json"));
        let (shared, _) = crate::service::Shared::new(c);
        let receipt = database_recall_receipt("wxid_self", "wxid_peer", &row()).unwrap();
        assert!(!recall_was_confirmed(&shared, &receipt).unwrap());
        crate::config::atomic(
            &recall_intent(&shared, &receipt),
            &json!({"state":"unconfirmed","server_id":receipt.server_id}),
        )
        .unwrap();
        assert!(!recall_was_confirmed(&shared, &receipt).unwrap());
        crate::config::atomic(
            &recall_intent(&shared, &receipt),
            &json!({"state":"confirmed","server_id":receipt.server_id}),
        )
        .unwrap();
        assert!(recall_was_confirmed(&shared, &receipt).unwrap());
        let mut wrong = receipt.clone();
        wrong.server_id = "1".into();
        assert!(!recall_was_confirmed(&shared, &wrong).unwrap());
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Receipt {
    pub from: String,
    pub to: String,
    pub client_id: String,
    pub local_id: u32,
    pub create_time: u32,
    pub server_id: String,
    #[serde(default)]
    pub recalled: bool,
}
/// Promote an already-confirmed own database message to the native revoke
/// channel. The caller verifies the account, target and original output scope.
/// This is a server receipt; it never rewrites a WeChat database row.
pub fn database_recall_receipt(
    from: &str,
    to: &str,
    row: &crate::database::Message,
) -> Result<Receipt> {
    if !from.starts_with("wxid_")
        || row.sender != from
        || to.is_empty()
        || !matches!(row.kind, 1 | 3 | 49)
        || row.server_id <= 0
    {
        bail!("原生撤回只接受当前账号已确认的文字、图片或卡片消息");
    }
    let local_id = u32::try_from(row.local_id)
        .ok()
        .filter(|v| *v > 0)
        .context("撤回本地消息标识无效")?;
    let create_time = u32::try_from(row.create_time)
        .ok()
        .filter(|v| *v > 0)
        .context("撤回原消息时间无效")?;
    Ok(Receipt {
        from: from.into(),
        to: to.into(),
        client_id: format!("{to}_{create_time}_{local_id}"),
        local_id,
        create_time,
        server_id: (row.server_id as u64).to_string(),
        recalled: false,
    })
}
fn recall_intent(shared: &crate::service::Shared, receipt: &Receipt) -> PathBuf {
    shared.data.join("native-recall").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(receipt.client_id.as_bytes()))
    ))
}
pub fn recall_was_confirmed(shared: &crate::service::Shared, receipt: &Receipt) -> Result<bool> {
    let path = recall_intent(shared, receipt);
    if !path.exists() {
        return Ok(false);
    }
    let value = crate::config::load(&path)?;
    Ok(value["state"] == "confirmed" && value["server_id"] == receipt.server_id)
}
pub fn deliver(
    shared: &crate::service::Shared,
    job: &crate::bridge::Job,
    music: &crate::music::OutgoingMusic,
    index: usize,
) -> Result<crate::actions::Tracked> {
    let c = shared.config.read().unwrap().clone();
    let _ = inspect(&c)?;
    job.check_submission(shared)?;
    let from = shared
        .staging
        .read()
        .unwrap()
        .as_ref()
        .context("本账号尚未核验")?
        .wxid
        .clone();
    let to = shared
        .peers
        .read()
        .unwrap()
        .iter()
        .find(|p| p.name == job.name && p.group == job.group)
        .context("音乐目标身份尚未核验")?
        .wxid
        .clone();
    let digest =
        Sha256::digest(format!("{}:{index}:{}", job.id, c.binding()?.created_at).as_bytes());
    let local_id = u32::from_le_bytes(digest[..4].try_into()?).max(1);
    let sent_at = crate::config::now();
    let create_time = sent_at as u32;
    let client_id = format!("{to}_{create_time}_{local_id}");
    if !music.playback_verified {
        crate::music::verify_playback(&music.card)?;
    }
    let xml = crate::music::native_appmsg_with_mode(&music.card, &music.app_id, music.link_mode)?;
    let mut receipt = Receipt {
        from,
        to,
        client_id,
        local_id,
        create_time,
        server_id: String::new(),
        recalled: false,
    };
    let intent = shared.data.join("native-music").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(receipt.client_id.as_bytes()))
    ));
    crate::config::atomic(
        &intent,
        &json!({"state":"admitted","receipt":receipt,"xml_sha256":hex::encode(Sha256::digest(xml.as_bytes()))}),
    )?;
    job.check_submission(shared)?;
    let response = rpc(
        &c,
        json!({"mode":"send","from":receipt.from,"to":receipt.to,"client_id":receipt.client_id,"time":create_time,"xml":xml}),
    )?;
    let id = response["server_id"]
        .as_str()
        .context("原生音乐回执缺少服务器ID")?
        .parse::<u64>()?;
    if id == 0 || response["server_accepted"] != true {
        bail!("原生音乐未确认服务端成功，不重发")
    }
    receipt.server_id = id.to_string();
    crate::config::atomic(
        &intent,
        &json!({"state":"server_confirmed","receipt":receipt,"title":music.card.title,"artist":music.card.artist}),
    )?;
    shared.event("delivery",&json!({"session":job.name,"group":job.group,"message_id":job.id,"title":music.card.title,"artist":music.card.artist,"server_id":receipt.server_id,"source":"wechat_native_cgi","success":true,"text":format!("[原生音乐] {} / {}\n服务端确认发送，消息编号：{}",music.card.title,music.card.artist,receipt.server_id),"input":response}));
    shared.log(&format!(
        "微信原生音乐已确认：{} / {}，服务器ID={}（原生网络回执，非本机数据库记录）",
        music.card.title, music.card.artist, receipt.server_id
    ));
    Ok(crate::actions::Tracked {
        row: crate::database::Message {
            local_id: local_id as i64,
            server_id: id as i64,
            sort_seq: job.id as i64,
            create_time: create_time as i64,
            kind: 49,
            sender: receipt.from.clone(),
            content: format!("<msg>{xml}</msg>"),
            shard: format!("native-cgi-{}-{index}", job.id),
        },
        runtime_id: vec![],
        sent_at,
        native_cgi: Some(receipt),
    })
}
pub fn recall(
    shared: &crate::service::Shared,
    name: &str,
    group: bool,
    receipt: &Receipt,
    sent_at: f64,
) -> Result<()> {
    let c = shared.config.read().unwrap().clone();
    let limit = crate::actions::recall_limit(&c.raw);
    if crate::config::now() - sent_at >= limit
        || sent_at > crate::config::now() + 1.
        || shared.stop.load(std::sync::atomic::Ordering::SeqCst)
    {
        bail!("原生消息撤回超过时限或正在停止，未提交")
    }
    let from = shared
        .staging
        .read()
        .unwrap()
        .as_ref()
        .context("本账号尚未核验")?
        .wxid
        .clone();
    let target = shared
        .peers
        .read()
        .unwrap()
        .iter()
        .find(|p| p.name == name && p.group == group)
        .context("原生消息撤回目标身份未核验")?
        .wxid
        .clone();
    if from != receipt.from || target != receipt.to || !c.allows(name, group) {
        bail!("原生消息账号或目标发生变化，未撤回其他消息")
    }
    let binding = c.binding()?;
    let current = crate::database::account_root(&c, &native::Process::bound(&binding)?)?;
    if shared
        .bound_root
        .read()
        .unwrap()
        .as_ref()
        .is_none_or(|r| !crate::database::same_root(r, &current))
    {
        bail!("微信账号已改变，未撤回原生消息")
    }
    // Persist admission before touching the native manager. A timeout must not
    // turn into another blind submission on a later RedLang delete or restart.
    let _ = inspect(&c)?;
    let age = crate::config::now() - sent_at;
    if age >= limit || shared.stop.load(std::sync::atomic::Ordering::SeqCst) {
        bail!("原生撤回准备后已到时限，未提交");
    }
    let intent = recall_intent(shared, receipt);
    if intent.exists() {
        let prior = crate::config::load(&intent)?;
        if prior["state"] == "confirmed" && prior["server_id"] == receipt.server_id {
            return Ok(());
        }
        bail!("原生撤回已经提交，结果尚待核对；未重复提交");
    }
    crate::config::atomic(
        &intent,
        &json!({"state":"submitted","server_id":receipt.server_id,"submitted_at":crate::config::now()}),
    )?;
    shared.log(&format!(
        "原生撤回提交：服务器消息ID={}，发出后{age:.1}秒，时限剩余{:.1}秒",
        receipt.server_id,
        (limit - age).max(0.)
    ));
    let result = rpc(
        &c,
        json!({"mode":"send","operation":"revoke","from":receipt.from,"to":receipt.to,"client_id":receipt.client_id,"local_id":receipt.local_id,"time":receipt.create_time,"server_id":receipt.server_id,"limit_seconds":limit as u64}),
    );
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            let _ = crate::config::atomic(
                &intent,
                &json!({"state":"unconfirmed","server_id":receipt.server_id,"error":error.to_string()}),
            );
            return Err(error);
        }
    };
    if result["revoke_confirmed"] != true {
        bail!("原生消息撤回未确认，不重复执行")
    }
    crate::config::atomic(
        &intent,
        &json!({"state":"confirmed","server_id":receipt.server_id,"confirmed_at":crate::config::now()}),
    )?;
    let path = shared.data.join("native-music").join(format!(
        "{}.json",
        hex::encode(Sha256::digest(receipt.client_id.as_bytes()))
    ));
    let mut value = crate::config::load(&path).unwrap_or_else(|_| json!({}));
    let mut saved = receipt.clone();
    saved.recalled = true;
    value["state"] = "recalled".into();
    value["receipt"] = serde_json::to_value(saved)?;
    crate::config::atomic(&path, &value)?;
    shared.log(&format!(
        "微信原生消息撤回已由服务端确认：{}",
        receipt.server_id
    ));
    Ok(())
}
#[derive(Clone)]
pub struct Profile {
    pub base: usize,
    pub managers: Vec<usize>,
    pub pid: u32,
    pub anchor: usize,
    pub dispatch: usize,
    pub dispatch_prefix: Vec<u8>,
    pub version: String,
}
fn word(data: &[u8], at: usize) -> Option<usize> {
    data.get(at..at + 8)
        .and_then(|s| s.try_into().ok())
        .map(usize::from_le_bytes)
}
static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<(u32, u64, isize, Profile)>>> =
    std::sync::OnceLock::new();
fn manager_valid(process: &native::Process, anchor: usize, address: usize) -> bool {
    if address % 8 != 0 {
        return false;
    }
    let Some(data) = process.read(address, 0x90) else {
        return false;
    };
    if data.len() < 0x90
        || word(&data, 0) != Some(anchor)
        || data[8] & 1 == 0
        || word(&data, 0x60).is_none_or(|n| n > 100000)
    {
        return false;
    }
    let Some(head) = word(&data, 0x58) else {
        return false;
    };
    let Some(tree) = process.read(head, 32) else {
        return false;
    };
    tree.len() == 32
        && tree[25] == 1
        && (0..3).all(|i| {
            word(&tree, i * 8).is_some_and(|p| p > 0x10000 && process.read(p, 32).is_some())
        })
}
pub fn inspect(c: &Config) -> Result<Profile> {
    let binding = c.binding()?;
    let process = native::Process::bound(&binding)?;
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    if let Some((pid, birth, hwnd, p)) = cache.lock().unwrap().as_ref() {
        if *pid == binding.pid
            && *birth == binding.created_at.to_bits()
            && *hwnd == binding.hwnd
            && manager_valid(&process, p.anchor, p.managers[0])
            && process.read(p.dispatch, 32).as_deref() == Some(p.dispatch_prefix.as_slice())
        {
            return Ok(p.clone());
        }
    }
    let mut module = None;
    unsafe {
        let snapshot = native::Handle(CreateToolhelp32Snapshot(
            TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32,
            binding.pid,
        )?);
        let mut item = MODULEENTRY32W {
            dwSize: size_of::<MODULEENTRY32W>() as u32,
            ..Default::default()
        };
        if Module32FirstW(snapshot.0, &mut item).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &item.szModule[..item
                        .szModule
                        .iter()
                        .position(|v| *v == 0)
                        .unwrap_or(item.szModule.len())],
                );
                if name.eq_ignore_ascii_case("Weixin.dll") {
                    let path = PathBuf::from(String::from_utf16_lossy(
                        &item.szExePath[..item
                            .szExePath
                            .iter()
                            .position(|v| *v == 0)
                            .unwrap_or(item.szExePath.len())],
                    ));
                    module = Some((item.modBaseAddr as usize, path));
                    break;
                }
                if Module32NextW(snapshot.0, &mut item).is_err() {
                    break;
                }
            }
        }
    }
    let (base, path) = module.context("绑定进程未载入Weixin.dll")?;
    let profile = profile_for(&path)?;
    for f in profile["functions"]
        .as_object()
        .context("原生音乐版本资料无效")?
        .values()
    {
        let rva = usize::from_str_radix(
            f["rva"]
                .as_str()
                .context("missing rva")?
                .trim_start_matches("0x"),
            16,
        )?;
        let expected = hex::decode(f["first_32_bytes"].as_str().context("missing bytes")?)?;
        if process.read(base + rva, 32).as_deref() != Some(expected.as_slice()) {
            bail!("微信音乐后端函数与已核对版本不同，未调用")
        }
    }
    let anchor_rva = usize::from_str_radix(
        profile["manager_anchor"]
            .as_str()
            .context("missing manager_anchor")?
            .trim_start_matches("0x"),
        16,
    )?;
    let dispatch_rva = usize::from_str_radix(
        profile["functions"]["manager_dispatch"]["rva"]
            .as_str()
            .context("missing manager_dispatch")?
            .trim_start_matches("0x"),
        16,
    )?;
    let dispatch_prefix = hex::decode(
        profile["functions"]["manager_dispatch"]["first_32_bytes"]
            .as_str()
            .context("missing manager_dispatch bytes")?,
    )?;
    let anchor = base + anchor_rva;
    let needle = anchor.to_le_bytes().to_vec();
    let mut candidates = process.find(&[needle]).pop().unwrap_or_default();
    candidates.retain(|&address| manager_valid(&process, anchor, address));
    candidates.sort_unstable();
    candidates.dedup();
    if candidates.len() != 1 {
        bail!(
            "未能确认唯一启用中的原生网络管理器（{}项），未调用",
            candidates.len()
        )
    }
    let result = Profile {
        base,
        managers: candidates,
        pid: binding.pid,
        anchor,
        dispatch: base + dispatch_rva,
        dispatch_prefix,
        version: profile["version"].as_str().unwrap_or("unknown").to_owned(),
    };
    *cache.lock().unwrap() = Some((
        binding.pid,
        binding.created_at.to_bits(),
        binding.hwnd,
        result.clone(),
    ));
    Ok(result)
}
pub fn probe(c: &Config) -> Result<Value> {
    let p = inspect(c)?;
    Ok(
        json!({"version":p.version,"bound_pid":p.pid,"module_hash_and_loaded_code_match":true,"enabled_manager_count":p.managers.len(),"native_functions_called":false,"clipboard_accessed":false,"global_input_used":false}),
    )
}

struct Mapping {
    _handle: native::Handle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
        }
    }
}
pub fn rpc(c: &Config, mut request: Value) -> Result<Value> {
    use std::{
        sync::atomic::{AtomicU32, Ordering},
        time::{Duration, Instant},
    };
    let p = inspect(c)?;
    let binding = c.binding()?;
    let h = native::validate_window(&binding)?;
    let name = format!(
        "Local\\WechatRedReplyMusic-{}",
        hex::encode(rand::random::<[u8; 16]>())
    );
    let n = crate::config::wide(&name);
    unsafe {
        let handle = native::Handle(CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            None,
            PAGE_READWRITE,
            0,
            8192,
            PCWSTR(n.as_ptr()),
        )?);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            bail!("音乐RPC映射名称冲突")
        }
        let view = MapViewOfFile(handle.0, FILE_MAP_ALL_ACCESS, 0, 0, 8192);
        if view.Value.is_null() {
            bail!("音乐RPC映射失败")
        }
        let map = Mapping {
            _handle: handle,
            view,
        };
        let status = &*(map.view.Value as *const AtomicU32);
        status.store(0, Ordering::Relaxed);
        request["mapping"] = name.into();
        request["pid"] = p.pid.into();
        request["base"] = (p.base as u64).into();
        request["manager"] = (p.managers[0] as u64).into();
        request["profile"] = p.version.clone().into();
        let bytes = serde_json::to_vec(&request)?;
        let module = crate::popup::load_embedded(&c.path.parent().unwrap().join("data"))?;
        let function = GetProcAddress(module, PCSTR(c"MusicCgiHook".as_ptr() as _))
            .context("音乐RPC入口未内嵌")?;
        let hook = SetWindowsHookExW(
            WH_CALLWNDPROC,
            Some(std::mem::transmute(function)),
            Some(module.into()),
            GetWindowThreadProcessId(h, None),
        )?;
        struct Hook {
            hook: HHOOK,
            module: HMODULE,
        }
        impl Drop for Hook {
            fn drop(&mut self) {
                unsafe {
                    let _ = UnhookWindowsHookEx(self.hook);
                    let _ = FreeLibrary(self.module);
                }
            }
        }
        let _hook = Hook { hook, module };
        let data = COPYDATASTRUCT {
            dwData: 0x574d555349433432,
            cbData: bytes.len() as u32,
            lpData: bytes.as_ptr() as *mut _,
        };
        let before = native::input_snapshot();
        let mut result = 0;
        if SendMessageTimeoutW(
            h,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&data as *const _ as isize),
            SMTO_ABORTIFHUNG,
            3000,
            Some(&mut result),
        )
        .0 == 0
        {
            bail!("音乐RPC未响应；不得盲目重发")
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while status.load(Ordering::Acquire) < 2 {
            if Instant::now() >= deadline {
                bail!("音乐原生任务未确认完成；不得盲目重发")
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        let len = (&*((map.view.Value as *const u8).add(4) as *const AtomicU32))
            .load(Ordering::Relaxed) as usize;
        if len == 0 || len > 8000 {
            bail!("音乐RPC返回长度无效")
        }
        let mut value: Value = serde_json::from_slice(std::slice::from_raw_parts(
            (map.view.Value as *const u8).add(8),
            len,
        ))?;
        let after = native::input_snapshot();
        value["clipboard_unchanged"] = (before.clipboard == after.clipboard).into();
        value["cursor_unchanged"] = (before.cursor == after.cursor).into();
        value["foreground_unchanged"] = (before.foreground == after.foreground).into();
        value["focus_unchanged"] = (before.focus == after.focus).into();
        if status.load(Ordering::Acquire) == 3 {
            bail!("{}", value["error"].as_str().unwrap_or("音乐RPC失败"))
        }
        Ok(value)
    }
}

/// Bounded first delivery to the explicitly verified self chat, not a production
/// success shortcut: native acceptance and local type3 confirmation are separate.
pub fn self_test(
    c: &Config,
    reference: &std::path::Path,
    request: Option<&std::path::Path>,
    target: Option<&str>,
    hold: u64,
) -> Result<Value> {
    use std::time::{Duration, Instant};
    let reference = std::fs::read_to_string(reference)?;
    let original = crate::music::parse_native(&reference)?.context("参考不是原生音乐卡")?;
    let app_id = crate::music::native_app_id(&reference).context("样例缺少原生应用标识")?;
    let card = if let Some(path) = request {
        let v = crate::config::load(path)?;
        let segment = if v["action"].is_string() {
            v["params"]["message"]
                .as_array()
                .context("请求没有消息段")?
                .iter()
                .find(|s| matches!(s["type"].as_str(), Some("music" | "json" | "xml")))
                .context("没有音乐消息段")?
                .clone()
        } else {
            v
        };
        match crate::music::parse_segment(&segment)?.context("请求不是音乐卡")? {
            crate::music::MusicRequest::Card(c) => c,
            _ => bail!("测试需要已解析曲目元数据"),
        }
    } else {
        let segment = json!({"type":"json","data":{"data":{"app":"com.tencent.structmsg","view":"music","meta":{"music":{"title":original.title,"desc":original.artist,"jumpUrl":original.page_url,"musicUrl":original.audio_url,"preview":original.image_url}}}}});
        match crate::music::parse_segment(&segment)?.context("QQ样例解析失败")? {
            crate::music::MusicRequest::Card(c) => c,
            _ => unreachable!(),
        }
    };
    let xml = crate::music::native_appmsg(&card, &app_id)?;
    let mut db =
        crate::database::Database::new(c, &c.path.parent().unwrap().join("data/music-probe"))?;
    let stage = db.staging(c)?;
    if stage.wxid != db.wxid {
        bail!("测试中转目标不是本账号自己")
    }
    let target = if let Some(name) = target {
        db.peers(c)?
            .into_iter()
            .find(|p| p.name == name && c.allows(&p.name, p.group))
            .context("音乐测试目标不在白名单")?
    } else {
        stage.clone()
    };
    let _ = inspect(c)?;
    // This local identifier belongs to our native outbox namespace; no row is
    // fabricated in WeChat's database.
    let local = rand::random::<u32>().max(1);
    let sent_time = crate::config::now() as u32;
    let client = format!("{}_{}_{}", target.wxid, sent_time, local);
    let begin = Instant::now();
    let mut report = rpc(
        c,
        json!({"mode":"send","from":db.wxid,"to":target.wxid,"client_id":client,"time":sent_time,"xml":xml}),
    )?;
    let server = report["server_id"]
        .as_str()
        .context("缺少服务器ID")?
        .parse::<u64>()?;
    report["title"] = card.title.clone().into();
    report["artist"] = card.artist.clone().into();
    report["source_card_forwarded"] = false.into();
    report["target_verified_self"] = (target.wxid == stage.wxid).into();
    report["test_target"] = target.name.clone().into();
    report["qq_json_converted"] = request.is_none().into();
    crate::config::atomic(
        &c.path
            .parent()
            .unwrap()
            .join("data/music-probe/live-progress.json"),
        &report,
    )?;
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(row) = db
            .messages(&target.wxid, -1)?
            .into_iter()
            .find(|r| r.server_id as u64 == server)
        {
            let actual = crate::music::parse_native(&row.content)?;
            let verified = row.kind == 49
                && actual.is_some_and(|native| {
                    crate::music::MusicRequest::Card(card.clone()).matches_cached(&native)
                });
            report["database_confirmed_native_type3"] = verified.into();
            report["new_message_key"] = row.key().into();
            break;
        }
        if Instant::now() >= deadline {
            report["database_confirmed_native_type3"] = false.into();
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    report["send_seconds"] = begin.elapsed().as_secs_f64().into();
    report["local_id_origin"] = "bridge_native_outbox".into();
    std::thread::sleep(Duration::from_secs(hold.min(75)));
    report["recall"] = rpc(
        c,
        json!({"mode":"send","operation":"revoke","from":db.wxid,"to":target.wxid,"client_id":client,"time":sent_time,"local_id":local,"server_id":server.to_string(),"limit_seconds":120}),
    )?;
    std::thread::sleep(Duration::from_millis(100));
    report["lifecycle"] = rpc(c, json!({"mode":"stats"}))?;
    Ok(report)
}
