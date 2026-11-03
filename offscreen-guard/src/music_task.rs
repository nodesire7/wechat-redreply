//! CGI222 task with native BaseRequest, owned completion and native destruction.
use crate::music_cgi::{MusicCgiHook, reply};
use serde_json::{Value, json};
use std::{
    cell::UnsafeCell,
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
};
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::*, Memory::*},
    },
    core::PCWSTR,
};
type Serialize = unsafe extern "system" fn(usize, usize, usize, *mut u8, *mut u32) -> u8;
type Response = unsafe extern "system" fn(usize, usize) -> u8;
type Dtor = unsafe extern "system" fn(usize, u32) -> usize;
static BASE: OnceLock<usize> = OnceLock::new();
struct Entry {
    base: usize,
    from: String,
    to: String,
    client: String,
    mapping: String,
    appmsg: Vec<u8>,
    vtable: Box<[usize; 6]>,
    original: [usize; 6],
    done: AtomicU32,
    server: AtomicU64,
    revoke: bool,
}
static REGISTRY: OnceLock<Mutex<HashMap<usize, Arc<Entry>>>> = OnceLock::new();
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static DESTROYED: AtomicU64 = AtomicU64::new(0);
pub fn stats() -> Value {
    json!({"active_native_tasks":registry().lock().map(|r|r.len()).unwrap_or(usize::MAX),"submitted":SUBMITTED.load(Ordering::Relaxed),"destroyed":DESTROYED.load(Ordering::Relaxed)})
}
fn registry() -> &'static Mutex<HashMap<usize, Arc<Entry>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}
fn entry(task: usize) -> Option<Arc<Entry>> {
    registry().lock().ok()?.get(&task).cloned()
}
fn error(e: &Entry, message: impl Into<String>) {
    e.done.store(2, Ordering::Release);
    reply(
        &e.mapping,
        json!({"error":message.into(),"network_submitted":true}),
        3,
    )
}
unsafe fn word(ptr: usize, offset: usize) -> usize {
    unsafe { std::ptr::read_unaligned((ptr + offset) as *const usize) }
}
unsafe fn holder(base: usize, h: usize) -> Result<&'static [u8], String> {
    unsafe {
        let p = crate::profile::current()?;
        let size: unsafe extern "system" fn(usize) -> u32 =
            std::mem::transmute(base + crate::profile::rva(p, "holder_size")?);
        let data: unsafe extern "system" fn(usize, i32) -> usize =
            std::mem::transmute(base + crate::profile::rva(p, "holder_data")?);
        let len = size(h) as usize;
        let p = data(h, 0);
        if len == 0 || len > 1024 * 1024 || p == 0 {
            return Err("原生音乐buffer为空或超限".into());
        }
        Ok(std::slice::from_raw_parts(p as *const u8, len))
    }
}
unsafe extern "system" fn serialize(
    task: usize,
    h: usize,
    a3: usize,
    mode: *mut u8,
    a5: *mut u32,
) -> u8 {
    unsafe {
        let Some(e) = entry(task) else { return 0 };
        let result = std::panic::catch_unwind(|| -> Result<(), String> {
            let native: Serialize = std::mem::transmute(e.original[1]);
            if native(task, h, a3, mode, a5) == 0 {
                return Err("原生BaseRequest序列化失败".into());
            }
            let original = holder(e.base, h)?;
            crate::music_wire::native_base_only(original)?;
            let before = original.len();
            let p = crate::profile::current()?;
            let write: unsafe extern "system" fn(usize, *const u8, u32) -> u32 =
                std::mem::transmute(e.base + crate::profile::rva(p, "holder_write")?);
            write(h, e.appmsg.as_ptr(), e.appmsg.len() as u32);
            let after = holder(e.base, h)?;
            if after.len() != before + e.appmsg.len() || &after[before..] != e.appmsg {
                return Err("原生AppMsg追加未完整确认".into());
            }
            Ok(())
        });
        match result {
            Ok(Ok(())) => 1,
            Ok(Err(m)) => {
                error(&e, m);
                0
            }
            Err(_) => {
                error(&e, "原生音乐序列化异常");
                0
            }
        }
    }
}
unsafe extern "system" fn response(task: usize, h: usize) -> u8 {
    unsafe {
        let Some(e) = entry(task) else { return 0 };
        let result = std::panic::catch_unwind(|| -> Result<u64, String> {
            let ack = if e.revoke {
                crate::music_wire::server_ok(holder(e.base, h)?).map(|_| crate::music_wire::Ack {
                    server_id: e.server.load(Ordering::Relaxed),
                })
            } else {
                crate::music_wire::ack(holder(e.base, h)?, &e.from, &e.to, &e.client)
            };
            let native: Response = std::mem::transmute(e.original[2]);
            native(task, h);
            Ok(ack?.server_id)
        });
        match result {
            Ok(Ok(server)) => {
                e.server.store(server, Ordering::Relaxed);
                e.done.store(1, Ordering::Release);
                1
            }
            Ok(Err(m)) => {
                error(&e, m);
                0
            }
            Err(_) => {
                error(&e, "原生音乐响应处理异常");
                0
            }
        }
    }
}
unsafe extern "system" fn destroy(task: usize, flags: u32) -> usize {
    unsafe {
        let e = registry().lock().ok().and_then(|mut r| r.remove(&task));
        let original = e.as_ref().map(|e| e.original[0]).unwrap_or_else(|| {
            let base = BASE.get().copied().unwrap_or(0);
            crate::profile::current()
                .and_then(|p| crate::profile::rva(p, "task_dtor"))
                .map(|rva| base + rva)
                .unwrap_or(0)
        });
        let native: Dtor = std::mem::transmute(original);
        let result = native(task, flags);
        DESTROYED.fetch_add(1, Ordering::Relaxed);
        result
    }
}
unsafe extern "system" fn callback_copy(this: usize, dest: usize) -> usize {
    unsafe {
        let dest = if dest == 0 {
            let p = crate::profile::current().ok();
            let rva = p
                .and_then(|p| crate::profile::rva(p, "operator_new").ok())
                .unwrap_or(0);
            let alloc: unsafe extern "system" fn(usize) -> usize =
                std::mem::transmute(BASE.get().copied().unwrap() + rva);
            alloc(16)
        } else {
            dest
        };
        if dest != 0 {
            std::ptr::copy(this as *const u8, dest as *mut u8, 16);
        }
        dest
    }
}
unsafe extern "system" fn callback_call(
    this: usize,
    task: *const usize,
    ty: *const u32,
    code: *const u32,
) -> u8 {
    unsafe {
        let captured = word(this, 8);
        let Some(e) = entry(captured) else { return 1 };
        if task.is_null() || *task != captured || ty.is_null() || code.is_null() {
            error(&e, "音乐完成回调身份不一致");
            return 1;
        }
        if *ty != 0 || *code != 0 {
            error(
                &e,
                format!("微信音乐网络任务失败（类型{}，代码{}）", *ty, *code),
            );
            return 1;
        }
        match e.done.load(Ordering::Acquire) {
            1 => {
                let mut result = json!({"server_accepted":true,"server_id":e.server.load(Ordering::Relaxed).to_string(),"native_task_completed":true,"network_submitted":true});
                if e.revoke {
                    result["revoke_confirmed"] = true.into();
                }
                reply(&e.mapping, result, 2)
            }
            2 => (),
            _ => error(&e, "原生音乐任务已结束，但缺少可核验的响应"),
        }
        1
    }
}
#[repr(C)]
struct TypeInfo {
    vtable: usize,
    spare: UnsafeCell<usize>,
    name: [u8; 64],
}
unsafe impl Sync for TypeInfo {}
static TYPE: OnceLock<TypeInfo> = OnceLock::new();
unsafe extern "system" fn callback_type(_: usize) -> usize {
    TYPE.get().map(|t| t as *const _ as usize).unwrap_or(0)
}
unsafe extern "system" fn callback_delete(this: usize, free: u8) {
    unsafe {
        if free != 0 {
            let base = BASE.get().copied().unwrap();
            let rva = crate::profile::current()
                .and_then(|p| crate::profile::rva(p, "operator_delete"))
                .unwrap_or(0);
            let delete: unsafe extern "system" fn(usize) = std::mem::transmute(base + rva);
            delete(this)
        }
    }
}
unsafe extern "system" fn callback_target(this: usize) -> usize {
    this + 8
}
static CALLBACK: OnceLock<[usize; 6]> = OnceLock::new();
pub unsafe fn submit(base: usize, v: &Value) -> Result<(), String> {
    unsafe {
        let p = crate::profile::current()?;
        if registry().lock().map_err(|_| "音乐任务锁异常")?.len() >= 32 {
            return Err("原生音乐未结束任务已达上限，未提交".into());
        }
        let manager = v["manager"].as_u64().ok_or("缺少管理器")? as usize;
        let mut mbi = MEMORY_BASIC_INFORMATION::default();
        if VirtualQuery(
            Some(manager as *const _),
            &mut mbi,
            std::mem::size_of_val(&mbi),
        ) == 0
            || mbi.State != MEM_COMMIT
            || mbi.Protect.0 & PAGE_GUARD.0 != 0
            || word(manager, 0) != base + p.manager_anchor
            || (*((manager + 8) as *const u8) & 1) == 0
        {
            return Err("音乐管理器已失效，未提交".into());
        }
        let from = v["from"].as_str().ok_or("缺少发送方")?.to_owned();
        let to = v["to"].as_str().ok_or("缺少目标")?.to_owned();
        let client = v["client_id"].as_str().ok_or("缺少客户端ID")?.to_owned();
        let revoke = v["operation"] == "revoke";
        let time = u32::try_from(v["time"].as_u64().ok_or("缺少时间")?).map_err(|_| "时间溢出")?;
        let server = if revoke {
            let s = v["server_id"].as_str().ok_or("缺少服务器ID")?;
            let n = s.parse::<u64>().map_err(|_| "服务器ID无效")?;
            if n == 0 || n.to_string() != s {
                return Err("服务器ID无效".into());
            }
            n
        } else {
            0
        };
        let appmsg = if revoke {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "时钟无效")?
                .as_secs();
            let limit = v["limit_seconds"].as_u64().unwrap_or(120).clamp(1, 120);
            if now < time as u64 || now - time as u64 >= limit {
                return Err("音乐撤回已超过设置时限（最多120秒），未提交".into());
            }
            crate::music_wire::revoke_fields(
                &from,
                &to,
                &client,
                u32::try_from(v["local_id"].as_u64().ok_or("缺少本地回执ID")?)
                    .map_err(|_| "本地回执ID溢出")?,
                time,
                server,
            )?
        } else {
            crate::music_wire::appmsg_field(
                &from,
                &to,
                v["xml"].as_str().ok_or("缺少音乐XML")?,
                &client,
                time,
            )?
        };
        let mapping = v["mapping"].as_str().ok_or("缺少回执通道")?.to_owned();
        let mut pinned = HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            PCWSTR(MusicCgiHook as *const () as _),
            &mut pinned,
        )
        .map_err(|_| "音乐模块无法保持异步生命周期")?;
        let _ = BASE.set(base);
        TYPE.get_or_init(|| {
            let native: unsafe extern "system" fn(usize) -> usize =
                std::mem::transmute(base + crate::profile::rva(p, "callback_type").unwrap());
            let mut name = [0u8; 64];
            let s = b".?AVWechatRedReplyMusicCompletion@@";
            name[..s.len()].copy_from_slice(s);
            TypeInfo {
                vtable: word(native(0), 0),
                spare: UnsafeCell::new(0),
                name,
            }
        });
        let callback = CALLBACK.get_or_init(|| {
            [
                callback_copy as *const () as usize,
                callback_copy as *const () as usize,
                callback_call as *const () as usize,
                callback_type as *const () as usize,
                callback_delete as *const () as usize,
                callback_target as *const () as usize,
            ]
        });
        let mut original = [0usize; 6];
        std::ptr::copy_nonoverlapping(
            (base + p.task_vtable) as *const usize,
            original.as_mut_ptr(),
            6,
        );
        let mut vt = original;
        vt[0] = destroy as *const () as usize;
        vt[1] = serialize as *const () as usize;
        vt[2] = response as *const () as usize;
        let e = Arc::new(Entry {
            base,
            from,
            to,
            client,
            mapping,
            appmsg,
            vtable: Box::new(vt),
            original,
            done: AtomicU32::new(0),
            server: AtomicU64::new(server),
            revoke,
        });
        let alloc: unsafe extern "system" fn(usize) -> usize =
            std::mem::transmute(base + crate::profile::rva(p, "operator_new")?);
        let ctor: unsafe extern "system" fn(usize, usize) =
            std::mem::transmute(base + crate::profile::rva(p, "task_ctor")?);
        let task = alloc(0x178);
        if task == 0 {
            return Err("音乐任务分配失败".into());
        }
        let mut info = [0u64; 8];
        let cgi = if revoke { 594 } else { 222 };
        let endpoint = if revoke {
            c"/cgi-bin/micromsg-bin/revokemsg"
        } else {
            c"/cgi-bin/micromsg-bin/sendappmsg"
        };
        info[0] = cgi;
        info[1] = 1u64 << 32;
        info[2] = cgi;
        info[3] = endpoint.as_ptr() as usize as u64;
        info[5] = endpoint.to_bytes().len() as u64;
        info[6] = 47;
        info[7] = 1;
        ctor(task, info.as_ptr() as usize);
        *(task as *mut usize) = e.vtable.as_ptr() as usize;
        *((task + 0x60) as *mut usize) = callback.as_ptr() as usize;
        *((task + 0x68) as *mut usize) = task;
        *((task + 0x98) as *mut usize) = task + 0x60;
        registry()
            .lock()
            .map_err(|_| "音乐任务锁异常")?
            .insert(task, e.clone());
        reply(
            &e.mapping,
            json!({"network_submitted":true,"pending":true}),
            1,
        );
        let dispatch: unsafe extern "system" fn(usize, usize) =
            std::mem::transmute(base + crate::profile::rva(p, "manager_dispatch")?);
        SUBMITTED.fetch_add(1, Ordering::Relaxed);
        dispatch(manager, task);
        Ok(())
    }
}
