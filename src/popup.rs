//! The EXE embeds this Rust DLL and installs its hook only on the explicitly
//! bound WeChat GUI thread. The native actor retains protection between actions;
//! nested operations share it until their last reference is released.
use anyhow::{Result, bail};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{LibraryLoader::*, Threading::*},
        UI::WindowsAndMessaging::*,
    },
    core::{PCSTR, PCWSTR, w},
};
const BYTES: &[u8] = include_bytes!("../assets/offscreen-guard.dll");
pub(crate) fn load_embedded(data: &std::path::Path) -> Result<HMODULE> {
    let dir = data.join("runtime");
    std::fs::create_dir_all(&dir)?;
    let hash = hex::encode(Sha256::digest(BYTES));
    let path = dir.join(format!("offscreen-guard-{}.dll", &hash[..16]));
    if path.exists() {
        if Sha256::digest(std::fs::read(&path)?) != Sha256::digest(BYTES) {
            bail!("离屏组件校验失败");
        }
    } else {
        std::fs::write(&path, BYTES)?;
    }
    let path = crate::config::wide(path);
    Ok(unsafe { LoadLibraryW(PCWSTR(path.as_ptr()))? })
}
pub struct Guard {
    inner: Rc<Hooks>,
}
struct Hooks {
    hook: HHOOK,
    position: HHOOK,
    module: HMODULE,
    thread: u32,
    main: Option<HWND>,
}
thread_local! {static ACTIVE:RefCell<HashMap<isize,Weak<Hooks>>>=RefCell::new(HashMap::new());}
impl Guard {
    pub fn install(hwnd: HWND, data: &std::path::Path) -> Result<Self> {
        let tid = unsafe { GetWindowThreadProcessId(hwnd, None) };
        if let Some(inner) =
            ACTIVE.with(|a| a.borrow().get(&(hwnd.0 as isize)).and_then(Weak::upgrade))
        {
            if inner.thread == tid {
                return Ok(Self { inner });
            }
        }
        {
            let mut guard = Self::thread(tid, data)?;
            Rc::get_mut(&mut guard.inner).unwrap().main = Some(hwnd);
            unsafe {
                let mut out = 0;
                let status = SendMessageTimeoutW(
                    hwnd,
                    WM_APP + 0x5f0,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    500,
                    Some(&mut out),
                );
                if status.0 == 0
                    || GetPropW(hwnd, w!("WechatRedReplyRust.OriginalPopupProc")).is_invalid()
                {
                    bail!("主窗口离屏保护未接入；未执行原生菜单操作");
                }
            }
            ACTIVE.with(|a| {
                a.borrow_mut()
                    .insert(hwnd.0 as isize, Rc::downgrade(&guard.inner))
            });
            Ok(guard)
        }
    }
    fn thread(tid: u32, data: &std::path::Path) -> Result<Self> {
        unsafe {
            if GetSystemMetrics(SM_XVIRTUALSCREEN) < -25000
                || GetSystemMetrics(SM_YVIRTUALSCREEN) < -25000
            {
                bail!("显示器布局超出已验证离屏弹窗范围");
            }
            let module = load_embedded(data)?;
            let symbol = GetProcAddress(module, PCSTR(c"OffscreenGuard".as_ptr() as _))
                .ok_or_else(|| anyhow::anyhow!("离屏组件入口不存在"))?;
            let function: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT =
                std::mem::transmute(symbol);
            let hook = match SetWindowsHookExW(WH_CBT, Some(function), Some(module.into()), tid) {
                Ok(h) => h,
                Err(e) => {
                    let _ = FreeLibrary(module);
                    return Err(e.into());
                }
            };
            let symbol = GetProcAddress(module, PCSTR(c"PositionGuard".as_ptr() as _))
                .ok_or_else(|| anyhow::anyhow!("离屏位置保护入口不存在"))?;
            let function: unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT =
                std::mem::transmute(symbol);
            let position =
                match SetWindowsHookExW(WH_CALLWNDPROC, Some(function), Some(module.into()), tid) {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = UnhookWindowsHookEx(hook);
                        let _ = FreeLibrary(module);
                        return Err(e.into());
                    }
                };
            notify_existing(tid, WM_APP + 0x5ee);
            Ok(Self {
                inner: Rc::new(Hooks {
                    hook,
                    position,
                    module,
                    thread: tid,
                    main: None,
                }),
            })
        }
    }
}
impl Drop for Hooks {
    fn drop(&mut self) {
        unsafe {
            notify_existing(self.thread, WM_APP + 0x5ef);
            if let Some(h) = self.main {
                let mut out = 0;
                let _ = SendMessageTimeoutW(
                    h,
                    WM_APP + 0x5ef,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    500,
                    Some(&mut out),
                );
            }
            let _ = UnhookWindowsHookEx(self.hook);
            let _ = UnhookWindowsHookEx(self.position);
            let _ = FreeLibrary(self.module);
        }
    }
}
unsafe fn notify_existing(thread: u32, message: u32) {
    unsafe extern "system" fn callback(h: HWND, p: LPARAM) -> windows::core::BOOL {
        unsafe {
            let message = p.0 as u32;
            let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
            if style & WS_POPUP.0 != 0 && style & WS_CHILD.0 == 0 {
                let mut out = 0;
                let _ = SendMessageTimeoutW(
                    h,
                    message,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    300,
                    Some(&mut out),
                );
            }
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = EnumThreadWindows(thread, Some(callback), LPARAM(message as isize));
    }
}
pub fn probe(data: &std::path::Path) -> Result<serde_json::Value> {
    unsafe {
        let existing = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("existing popup verification"),
            WS_POPUP,
            50,
            50,
            300,
            200,
            None,
            None,
            None,
            None,
        )?;
        let _guard = Guard::thread(GetCurrentThreadId(), data)?;
        MoveWindow(existing, 100, 100, 300, 200, false)?;
        let mut old = RECT::default();
        GetWindowRect(existing, &mut old)?;
        let existing_protected = old.right < GetSystemMetrics(SM_XVIRTUALSCREEN)
            && old.bottom < GetSystemMetrics(SM_YVIRTUALSCREEN);
        let h = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("offscreen guard verification"),
            WS_POPUP,
            50,
            50,
            300,
            200,
            None,
            None,
            None,
            None,
        )?;
        let check = (|| -> Result<serde_json::Value> {
            let mut first = RECT::default();
            GetWindowRect(h, &mut first)?;
            let create = first.right < GetSystemMetrics(SM_XVIRTUALSCREEN)
                && first.bottom < GetSystemMetrics(SM_YVIRTUALSCREEN);
            MoveWindow(h, 100, 100, 300, 200, false)?;
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            let mut moved = RECT::default();
            GetWindowRect(h, &mut moved)?;
            let protected = moved.right < GetSystemMetrics(SM_XVIRTUALSCREEN)
                && moved.bottom < GetSystemMetrics(SM_YVIRTUALSCREEN);
            if !create || !protected || !existing_protected {
                bail!(
                    "离屏弹窗保护验证失败：create={create} [{} {} {} {}], move={protected} [{} {} {} {}]；未在微信启用原生操作",
                    first.left,
                    first.top,
                    first.right,
                    first.bottom,
                    moved.left,
                    moved.top,
                    moved.right,
                    moved.bottom
                );
            }
            Ok(
                serde_json::json!({"create_offscreen":create,"move_offscreen":protected,"existing_popup_protected":existing_protected}),
            )
        })();
        let _ = DestroyWindow(h);
        let _ = DestroyWindow(existing);
        let mut result = check?;
        drop(_guard);
        let nested = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("STATIC"),
            w!("nested guard verification"),
            WS_POPUP,
            20,
            20,
            200,
            100,
            None,
            None,
            None,
            None,
        )?;
        let outer = Guard::install(nested, data)?;
        let borrowed = Guard::install(nested, data)?;
        drop(outer);
        MoveWindow(nested, 100, 100, 200, 100, false)?;
        let mut r = RECT::default();
        GetWindowRect(nested, &mut r)?;
        let alive = r.right < 0
            && r.bottom < 0
            && !GetPropW(nested, w!("WechatRedReplyRust.OriginalPopupProc")).is_invalid();
        drop(borrowed);
        let released = GetPropW(nested, w!("WechatRedReplyRust.OriginalPopupProc")).is_invalid();
        let _ = DestroyWindow(nested);
        if !alive || !released {
            bail!("嵌套离屏保护引用寿命验证失败");
        }
        result["nested_guard_survives_owner_drop"] = alive.into();
        result["guard_releases_after_last_reference"] = released.into();
        Ok(result)
    }
}
