//! Version-checked CGI127 task. One RPC owns one chunk; the caller owns range progress.
//! The native voice serializer creates BaseRequest10; session bytes stay in-process.
use crate::music_cgi::{MusicCgiHook, reply};
use base64::{Engine as _, engine::general_purpose::STANDARD};
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
static NATIVE_DTOR: OnceLock<usize> = OnceLock::new();
struct Entry {
    base: usize,
    mapping: String,
    fields: Vec<u8>,
    vtable: Box<[usize; 6]>,
    original: [usize; 6],
    done: AtomicU32,
    response: Mutex<Option<Vec<u8>>>,
}
static REGISTRY: OnceLock<Mutex<HashMap<usize, Arc<Entry>>>> = OnceLock::new();
static SUBMITTED: AtomicU64 = AtomicU64::new(0);
static DESTROYED: AtomicU64 = AtomicU64::new(0);
pub fn stats() -> Value {
    json!({"active_native_voice_tasks":registry().lock().map(|r|r.len()).unwrap_or(usize::MAX),"submitted":SUBMITTED.load(Ordering::Relaxed),"destroyed":DESTROYED.load(Ordering::Relaxed)})
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
        let ptr = data(h, 0);
        if len == 0 || len > 64 * 1024 || ptr == 0 {
            return Err("原生语音buffer为空或超限".into());
        }
        Ok(std::slice::from_raw_parts(ptr as *const u8, len))
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
                return Err("原生语音BaseRequest序列化失败".into());
            }
            let original = holder(e.base, h)?;
            crate::voice_wire::native_base_only(original)?;
            let before = original.len();
            let p = crate::profile::current()?;
            let write: unsafe extern "system" fn(usize, *const u8, u32) -> u32 =
                std::mem::transmute(e.base + crate::profile::rva(p, "holder_write")?);
            write(h, e.fields.as_ptr(), e.fields.len() as u32);
            let after = holder(e.base, h)?;
            if after.len() != before + e.fields.len() || &after[before..] != e.fields {
                return Err("原生语音负载追加未完整确认".into());
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
                error(&e, "原生语音序列化异常");
                0
            }
        }
    }
}
unsafe extern "system" fn response(task: usize, h: usize) -> u8 {
    unsafe {
        let Some(e) = entry(task) else { return 0 };
        let result = std::panic::catch_unwind(|| -> Result<(), String> {
            let raw = holder(e.base, h)?.to_vec();
            // The existing receipt mapping has an 8 KiB envelope. Native voice
            // replies are metadata only; never overrun it with arbitrary data.
            if raw.len() > 4096 {
                return Err("原生语音响应超出回执通道限制".into());
            }
            let native: Response = std::mem::transmute(e.original[2]);
            if native(task, h) == 0 {
                return Err("原生语音响应处理失败".into());
            }
            *e.response.lock().map_err(|_| "语音响应锁异常")? = Some(raw);
            e.done.store(1, Ordering::Release);
            Ok(())
        });
        match result {
            Ok(Ok(())) => 1,
            Ok(Err(m)) => {
                error(&e, m);
                0
            }
            Err(_) => {
                error(&e, "原生语音响应处理异常");
                0
            }
        }
    }
}
unsafe extern "system" fn destroy(task: usize, flags: u32) -> usize {
    unsafe {
        let e = registry().lock().ok().and_then(|mut r| r.remove(&task));
        let original = e
            .as_ref()
            .map(|e| e.original[0])
            .unwrap_or_else(|| NATIVE_DTOR.get().copied().unwrap_or(0));
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
            error(&e, "语音完成回调身份不一致");
            return 1;
        }
        if *ty != 0 || *code != 0 {
            error(
                &e,
                format!("微信语音网络任务失败（类型{}，代码{}）", *ty, *code),
            );
            return 1;
        }
        match e.done.load(Ordering::Acquire) {
            1 => match e.response.lock().ok().and_then(|mut r| r.take()) {
                Some(raw) => reply(
                    &e.mapping,
                    json!({"response_b64":STANDARD.encode(raw),"native_task_completed":true,"network_submitted":true}),
                    2,
                ),
                None => error(&e, "语音完成时缺少可核验的响应"),
            },
            2 => (),
            _ => error(&e, "原生语音任务已结束，但缺少可核验的响应"),
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
unsafe fn checked_task_layout(
    base: usize,
    voice: &crate::profile::VoiceProfile,
) -> Result<[usize; 6], String> {
    unsafe {
        for (_, (rva, bytes)) in crate::profile::voice_functions(voice) {
            if std::slice::from_raw_parts((base + rva) as *const u8, 32) != bytes {
                return Err("语音通道代码版本不符，未调用".into());
            }
        }
        let expected = [
            "task_dtor",
            "serialize",
            "response",
            "get_request",
            "get_response",
        ];
        let mut original = [0usize; 6];
        std::ptr::copy_nonoverlapping(
            (base + voice.task_vtable) as *const usize,
            original.as_mut_ptr(),
            6,
        );
        for (i, name) in expected.iter().enumerate() {
            if original[i] != base + crate::profile::voice_rva(voice, name)? {
                return Err("语音任务虚表版本不符，未调用".into());
            }
        }
        if original[5] != 1 || voice.task_size != 0x198 {
            return Err("语音任务布局不符，未调用".into());
        }
        Ok(original)
    }
}
fn task_info() -> [u64; 8] {
    let endpoint = c"/cgi-bin/micromsg-bin/uploadvoice";
    let mut info = [0u64; 8];
    info[0] = crate::voice_wire::CGI_ID as u64;
    info[1] = 1u64 << 32;
    info[2] = crate::voice_wire::CGI_ID as u64;
    info[3] = endpoint.as_ptr() as usize as u64;
    info[5] = endpoint.to_bytes().len() as u64;
    info[6] = 47;
    info[7] = 1;
    info
}
unsafe fn fresh_task_matches(
    base: usize,
    voice: &crate::profile::VoiceProfile,
    task: usize,
) -> bool {
    unsafe {
        word(task, 0) == base + voice.task_vtable
            && std::ptr::read_unaligned((task + 12) as *const u32) == crate::voice_wire::CGI_ID
            && std::ptr::read_unaligned((task + 16) as *const u32) == 1
            && word(task, 0x98) == 0
            // UploadVoiceRequest's native constructor clears both its cached
            // size and has-bits at request+0x90, and BaseRequest+0x38 is null.
            && word(task, 0xa8 + 0x90) == 0
            && word(task, 0xa8 + 0x38) == 0
            && std::ptr::read_unaligned((task + 0x140 + 0x54) as *const u32) == 0
    }
}
/// Construct and release the actual voice task without serializing or dispatching.
pub unsafe fn construct_probe(base: usize) -> Result<Value, String> {
    unsafe {
        let p = crate::profile::current()?;
        let voice = crate::profile::voice(p)?;
        let original = checked_task_layout(base, voice)?;
        let alloc: unsafe extern "system" fn(usize) -> usize =
            std::mem::transmute(base + crate::profile::rva(p, "operator_new")?);
        let ctor: unsafe extern "system" fn(usize, usize) =
            std::mem::transmute(base + crate::profile::voice_rva(voice, "task_ctor")?);
        let dtor: Dtor = std::mem::transmute(original[0]);
        let task = alloc(voice.task_size);
        if task == 0 {
            return Err("原生语音探测任务分配失败".into());
        }
        let info = task_info();
        ctor(task, info.as_ptr() as usize);
        let valid = fresh_task_matches(base, voice, task);
        dtor(task, 1);
        if !valid {
            return Err("原生语音构造后的布局或默认请求不符，已释放，未提交".into());
        }
        Ok(json!({
            "constructed_and_released":true,
            "native_voice_task":true,
            "network_submitted":false,
            "ui_used":false,
        }))
    }
}
pub unsafe fn submit_chunk(base: usize, v: &Value) -> Result<(), String> {
    unsafe {
        let p = crate::profile::current()?;
        let voice = crate::profile::voice(p)?;
        let original = checked_task_layout(base, voice)?;
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
            return Err("语音管理器已失效，未提交".into());
        }
        let from = v["from"].as_str().ok_or("缺少语音发送方")?;
        let to = v["to"].as_str().ok_or("缺少语音目标")?;
        let client = v["client_id"].as_str().ok_or("缺少语音客户端ID")?;
        let encoded = v["fields_b64"].as_str().ok_or("缺少语音块负载")?;
        if encoded.len() > 64 * 1024 {
            return Err("语音块负载超限，未提交".into());
        }
        let fields = STANDARD.decode(encoded).map_err(|_| "语音块负载编码无效")?;
        crate::voice_wire::validate_request_fields(&fields, from, to, client)?;
        let mapping = v["mapping"].as_str().ok_or("缺少语音回执通道")?.to_owned();
        if !mapping.starts_with("Local\\WechatRedReplyMusic-") || mapping.len() > 100 {
            return Err("语音回执通道无效，未提交".into());
        }
        let mut pinned = HMODULE::default();
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            PCWSTR(MusicCgiHook as *const () as _),
            &mut pinned,
        )
        .map_err(|_| "语音模块无法保持异步生命周期")?;
        let _ = BASE.set(base);
        let _ = NATIVE_DTOR.set(original[0]);
        TYPE.get_or_init(|| {
            let native: unsafe extern "system" fn(usize) -> usize =
                std::mem::transmute(base + crate::profile::rva(p, "callback_type").unwrap());
            let mut name = [0u8; 64];
            let s = b".?AVWechatRedReplyVoiceCompletion@@";
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
        let mut vt = original;
        vt[0] = destroy as *const () as usize;
        vt[1] = serialize as *const () as usize;
        vt[2] = response as *const () as usize;
        let e = Arc::new(Entry {
            base,
            mapping,
            fields,
            vtable: Box::new(vt),
            original,
            done: AtomicU32::new(0),
            response: Mutex::new(None),
        });
        // Reserve ownership before allocation; never release a submitted task
        // because the external IPC caller times out or cancels its wait.
        let mut owned = registry().lock().map_err(|_| "语音任务锁异常")?;
        if owned.len() >= 32 {
            return Err("原生语音未结束任务已达上限，未提交".into());
        }
        let alloc: unsafe extern "system" fn(usize) -> usize =
            std::mem::transmute(base + crate::profile::rva(p, "operator_new")?);
        let ctor: unsafe extern "system" fn(usize, usize) =
            std::mem::transmute(base + crate::profile::voice_rva(voice, "task_ctor")?);
        let native_dtor: Dtor = std::mem::transmute(original[0]);
        let task = alloc(voice.task_size);
        if task == 0 {
            return Err("语音任务分配失败".into());
        }
        let info = task_info();
        ctor(task, info.as_ptr() as usize);
        if !fresh_task_matches(base, voice, task) {
            native_dtor(task, 1);
            return Err("原生语音构造后的布局不符，已释放，未提交".into());
        }
        *(task as *mut usize) = e.vtable.as_ptr() as usize;
        *((task + 0x60) as *mut usize) = callback.as_ptr() as usize;
        *((task + 0x68) as *mut usize) = task;
        *((task + 0x98) as *mut usize) = task + 0x60;
        owned.insert(task, e.clone());
        drop(owned);
        let dispatch: unsafe extern "system" fn(usize, usize) =
            std::mem::transmute(base + crate::profile::rva(p, "manager_dispatch")?);
        reply(
            &e.mapping,
            json!({"network_submitted":true,"pending":true}),
            1,
        );
        SUBMITTED.fetch_add(1, Ordering::Relaxed);
        dispatch(manager, task);
        Ok(())
    }
}
