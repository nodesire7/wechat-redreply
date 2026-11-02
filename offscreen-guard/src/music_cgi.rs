//! Explicit music RPC only, on the already bound GUI thread. No UI/input APIs.
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, Ordering};
use windows::{
    Win32::{
        Foundation::*,
        System::{DataExchange::COPYDATASTRUCT, LibraryLoader::*, Memory::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
    core::{PCWSTR, w},
};
const MAGIC: usize = 0x574d555349433432;
const PROFILE: &str = include_str!("../../assets/music-cgi-4.2.0.9.json");
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn reply(name: &str, value: Value, status: u32) {
    unsafe {
        if !name.starts_with("Local\\WechatRedReplyMusic-") || name.len() > 100 {
            return;
        }
        let Ok(data) = serde_json::to_vec(&value) else {
            return;
        };
        if data.len() > 8000 {
            return;
        }
        let Ok(h) = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, PCWSTR(wide(name).as_ptr()))
        else {
            return;
        };
        let view = MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, 8192);
        if !view.Value.is_null() {
            std::ptr::copy_nonoverlapping(
                data.as_ptr(),
                (view.Value as *mut u8).add(8),
                data.len(),
            );
            (&*((view.Value as *const u8).add(4) as *const AtomicU32))
                .store(data.len() as u32, Ordering::Relaxed);
            (&*(view.Value as *const AtomicU32)).store(status, Ordering::Release);
            let _ = UnmapViewOfFile(view);
        }
        let _ = CloseHandle(h);
    }
}
unsafe fn word(ptr: usize, offset: usize) -> usize {
    unsafe { std::ptr::read_unaligned((ptr + offset) as *const usize) }
}
unsafe fn check(v: &Value) -> Result<usize, String> {
    unsafe {
        if v["pid"].as_u64() != Some(GetCurrentProcessId() as u64) {
            return Err("音乐通道进程身份不匹配".into());
        }
        let base = GetModuleHandleW(w!("Weixin.dll"))
            .map_err(|_| "Weixin.dll不存在")?
            .0 as usize;
        if v["base"].as_u64() != Some(base as u64) {
            return Err("音乐通道模块地址不匹配".into());
        }
        let p: Value = serde_json::from_str(PROFILE).map_err(|_| "音乐版本资料无效")?;
        for f in p["functions"]
            .as_object()
            .ok_or("音乐版本资料无效")?
            .values()
        {
            let rva =
                usize::from_str_radix(f["rva"].as_str().ok_or("rva")?.trim_start_matches("0x"), 16)
                    .map_err(|_| "rva")?;
            let hex = f["first_32_bytes"].as_str().ok_or("bytes")?;
            let bytes = (0..32)
                .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
                .collect::<Vec<_>>();
            if std::slice::from_raw_parts((base + rva) as *const u8, 32) != bytes {
                return Err("音乐通道代码版本不符，未调用".into());
            }
        }
        Ok(base)
    }
}
unsafe fn construct(base: usize) -> Result<Value, String> {
    unsafe {
        type New = unsafe extern "system" fn(usize) -> usize;
        type Ctor = unsafe extern "system" fn(usize, usize);
        type Dtor = unsafe extern "system" fn(usize, u32) -> usize;
        let new: New = std::mem::transmute(base + 0x76970bc);
        let ctor: Ctor = std::mem::transmute(base + 0x3ae92f0);
        let dtor: Dtor = std::mem::transmute(base + 0x3ae94c0);
        let task = new(0x178);
        if task == 0 {
            return Err("原生任务分配失败".into());
        }
        let mut info = [0u64; 8];
        info[0] = 222;
        info[1] = 1u64 << 32;
        info[2] = 222;
        info[3] = c"/cgi-bin/micromsg-bin/sendappmsg".as_ptr() as usize as u64;
        info[5] = 32;
        info[6] = 47;
        info[7] = 1;
        ctor(task, info.as_ptr() as usize);
        let vt = word(task, 0);
        let ok = vt == base + 0x9806818
            && std::ptr::read_unaligned((task + 12) as *const u32) == 222
            && std::ptr::read_unaligned((task + 16) as *const u32) == 1
            && word(task, 0x98) == 0;
        dtor(task, 1);
        if !ok {
            return Err("原生任务构造后的布局不符，已释放，未提交".into());
        }
        Ok(json!({"constructed_and_released":true,"network_submitted":false,"ui_used":false}))
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "system" fn MusicCgiHook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && l.0 != 0 {
            let m = &*(l.0 as *const CWPSTRUCT);
            if m.message == WM_COPYDATA && m.lParam.0 != 0 {
                let data = &*(m.lParam.0 as *const COPYDATASTRUCT);
                if data.dwData == MAGIC
                    && data.cbData > 0
                    && data.cbData < 1024 * 1024
                    && !data.lpData.is_null()
                {
                    let bytes =
                        std::slice::from_raw_parts(data.lpData as *const u8, data.cbData as usize);
                    if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
                        if let Some(name) = v["mapping"].as_str() {
                            let result =
                                std::panic::catch_unwind(|| -> Result<Option<Value>, String> {
                                    let base = check(&v)?;
                                    match v["mode"].as_str() {
                                        Some("construct") => construct(base).map(Some),
                                        Some("send") => {
                                            crate::music_task::submit(base, &v).map(|_| None)
                                        }
                                        Some("stats") => Ok(Some(crate::music_task::stats())),
                                        _ => Err("音乐通道操作未启用".into()),
                                    }
                                });
                            match result {
                                Ok(Ok(Some(value))) => reply(name, value, 2),
                                Ok(Ok(None)) => (),
                                Ok(Err(error)) => reply(name, json!({"error":error}), 3),
                                Err(_) => {
                                    reply(name, json!({"error":"音乐通道内部异常，未确认"}), 3)
                                }
                            }
                        }
                    }
                }
            }
        }
        CallNextHookEx(None, code, w, l)
    }
}
