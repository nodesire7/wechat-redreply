//! Thread-scoped CBT hook: place Qt popup windows outside the desktop BEFORE
//! their first display and every subsequent attempted move. No client offsets.
mod music_cgi;
mod music_task;
#[path = "../../src/music_wire.rs"]
mod music_wire;
mod profile;
mod voice_task;
#[path = "../../src/voice_wire.rs"]
mod voice_wire;
use windows::{
    Win32::{Foundation::*, System::LibraryLoader::*, UI::WindowsAndMessaging::*},
    core::w,
};
const OUTSIDE: i32 = -30000;
const PROPERTY: windows::core::PCWSTR = w!("WechatRedReplyRust.OriginalPopupProc");
const ARM: u32 = WM_APP + 0x5ee;
const DISARM: u32 = WM_APP + 0x5ef;
unsafe fn arm(h: HWND, force: bool) {
    unsafe {
        let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        if (!force && style & WS_POPUP.0 == 0) || style & WS_CHILD.0 != 0 {
            return;
        }
        let mut class = [0u16; 128];
        let n = GetClassNameW(h, &mut class);
        let name = String::from_utf16_lossy(&class[..n as usize]);
        if !name.starts_with("Qt") && !name.eq_ignore_ascii_case("STATIC") && name != "#32770" {
            return;
        }
        if GetPropW(h, PROPERTY).is_invalid() {
            let original = GetWindowLongPtrW(h, GWLP_WNDPROC);
            if original != 0 && SetPropW(h, PROPERTY, Some(HANDLE(original as *mut _))).is_ok() {
                let mut module = HMODULE::default();
                let _ = GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
                    windows::core::PCWSTR(popup_proc as *const () as _),
                    &mut module,
                );
                SetWindowLongPtrW(h, GWLP_WNDPROC, popup_proc as *const () as isize);
            }
        }
    }
}
unsafe extern "system" fn popup_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        let original = GetPropW(h, PROPERTY);
        if msg == DISARM {
            SetWindowLongPtrW(h, GWLP_WNDPROC, original.0 as isize);
            let _ = RemovePropW(h, PROPERTY);
            return LRESULT(0);
        }
        let proc: WNDPROC = std::mem::transmute(original.0);
        let result = CallWindowProcW(proc, h, msg, w, l);
        if msg == WM_WINDOWPOSCHANGING && l.0 != 0 {
            let pos = &mut *(l.0 as *mut WINDOWPOS);
            pos.x = OUTSIDE;
            pos.y = OUTSIDE;
            pos.flags &= !SWP_NOMOVE;
        }
        if msg == WM_NCDESTROY {
            let _ = RemovePropW(h, PROPERTY);
        }
        result
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "system" fn PositionGuard(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code >= 0 && l.0 != 0 {
            let message = &*(l.0 as *const CWPSTRUCT);
            if message.message == ARM || message.message == ARM + 2 {
                arm(message.hwnd, message.message == ARM + 2);
                let _ = SetWindowPos(
                    message.hwnd,
                    None,
                    OUTSIDE,
                    OUTSIDE,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            if message.message == WM_WINDOWPOSCHANGING && message.lParam.0 != 0 {
                let style = GetWindowLongPtrW(message.hwnd, GWL_STYLE) as u32;
                if style & WS_POPUP.0 != 0 && style & WS_CHILD.0 == 0 {
                    let pos = &mut *(message.lParam.0 as *mut WINDOWPOS);
                    pos.x = OUTSIDE;
                    pos.y = OUTSIDE;
                    pos.flags &= !SWP_NOMOVE;
                }
            }
        }
        CallNextHookEx(None, code, w, l)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "system" fn OffscreenGuard(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if code == HCBT_ACTIVATE as i32 || code == HCBT_SETFOCUS as i32 {
            // Only this explicitly bound GUI thread is hooked. Background
            // operations must never take the user's active window or focus.
            return LRESULT(1);
        }
        if code == HCBT_CREATEWND as i32 && l.0 != 0 {
            let data = &mut *(l.0 as *mut CBT_CREATEWNDW);
            if !data.lpcs.is_null() {
                let cs = &mut *data.lpcs;
                if cs.style as u32 & WS_CHILD.0 == 0 {
                    let h = HWND(w.0 as *mut _);
                    arm(h, true);
                    if !GetPropW(h, PROPERTY).is_invalid() {
                        cs.x = OUTSIDE;
                        cs.y = OUTSIDE;
                    }
                }
            }
        } else if code == HCBT_MOVESIZE as i32 && l.0 != 0 {
            let h = HWND(w.0 as *mut _);
            let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
            if style & WS_POPUP.0 != 0 && style & WS_CHILD.0 == 0 {
                let r = &mut *(l.0 as *mut RECT);
                let width = r.right - r.left;
                let height = r.bottom - r.top;
                r.left = OUTSIDE;
                r.top = OUTSIDE;
                r.right = OUTSIDE + width;
                r.bottom = OUTSIDE + height;
            }
        }
        CallNextHookEx(None, code, w, l)
    }
}
