//! Read-only diagnostics for another Windows installation. Deliberately omits
//! tokens, account paths, contact names, message content and screenshots.
use crate::{
    config::{self, Config},
    delivery::Automation,
    native,
};
use serde_json::{Value, json};
use windows::Win32::{
    Foundation::*,
    System::Diagnostics::ToolHelp::*,
    UI::{Accessibility::*, WindowsAndMessaging::*},
};
#[repr(C)]
struct Version {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
    sp_major: u16,
    sp_minor: u16,
    suite: u16,
    product: u8,
    reserved: u8,
}
fn os() -> Value {
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut Version) -> i32;
    }
    unsafe {
        let mut v: Version = std::mem::zeroed();
        v.size = std::mem::size_of::<Version>() as u32;
        if RtlGetVersion(&mut v) < 0 {
            return json!({"name":"Windows","version":"unknown"});
        }
        json!({"major":v.major,"minor":v.minor,"build":v.build,"server":v.product!=1,"remote_session":GetSystemMetrics(SM_REMOTESESSION)!=0})
    }
}
fn client_modules(pid: u32) -> Value {
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
        else {
            return Value::Null;
        };
        let snapshot = native::Handle(snapshot);
        let mut row: MODULEENTRY32W = std::mem::zeroed();
        row.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut found = vec![];
        if Module32FirstW(snapshot.0, &mut row).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &row.szModule[..row
                        .szModule
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(row.szModule.len())],
                );
                if [
                    "weixin.dll",
                    "wechatwin.dll",
                    "qwindows.dll",
                    "qt6core.dll",
                    "qt6gui.dll",
                    "qt6widgets.dll",
                    "qtaccessiblewidgets.dll",
                ]
                .contains(&name.to_lowercase().as_str())
                {
                    let path = String::from_utf16_lossy(
                        &row.szExePath[..row
                            .szExePath
                            .iter()
                            .position(|c| *c == 0)
                            .unwrap_or(row.szExePath.len())],
                    );
                    let parent = std::path::Path::new(&path)
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let hint = if parent.len() < 32
                        && parent.chars().all(|c| c.is_ascii_digit() || c == '.')
                    {
                        parent
                    } else {
                        "unknown".into()
                    };
                    found.push(
                        json!({"module":name,"version_hint":hint,"loaded_size":row.modBaseSize}),
                    );
                }
                if Module32NextW(snapshot.0, &mut row).is_err() {
                    break;
                }
            }
        }
        json!(found)
    }
}
pub fn collect(c: &Config) -> Value {
    let mut report = json!({"schema":1,"app_version":env!("CARGO_PKG_VERSION"),"time":config::now(),"windows":os(),"privacy":"No token, account name, path, chat content or screenshot is collected.","checks":{}});
    let binding = match c.binding() {
        Ok(b) => b,
        Err(_) => {
            report["checks"]["binding"] = json!({"valid":false,"reason":"not_configured"});
            return report;
        }
    };
    let h = match native::validate_window(&binding) {
        Ok(h) => h,
        Err(e) => {
            report["checks"]["binding"] = json!({"valid":false,"reason":format!("{e}")});
            return report;
        }
    };
    report["checks"]["binding"] =
        json!({"valid":true,"process_creation_matches":true,"window_process_matches":true});
    report["client_modules"] = client_modules(binding.pid);
    unsafe {
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let mut class = [0u16; 256];
        let n = GetClassNameW(h, &mut class);
        report["checks"]["window"] = json!({"class":String::from_utf16_lossy(&class[..n as usize]),"visible":IsWindowVisible(h).as_bool(),"minimized":IsIconic(h).as_bool(),"enabled":windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(h).as_bool(),"width":r.right-r.left,"height":r.bottom-r.top,"offscreen":r.right<GetSystemMetrics(SM_XVIRTUALSCREEN)&&r.bottom<GetSystemMetrics(SM_YVIRTUALSCREEN)});
    }
    let checked = (|| -> anyhow::Result<Value> {
        let legacy = std::env::args().any(|s| s == "--uia-legacy");
        let a = if legacy {
            Automation::legacy()?
        } else {
            Automation::new()?
        };
        let root = a.root(h)?;
        let nodes = a.descendants(&root)?;
        let mut search = false;
        let mut edits = 0;
        unsafe {
            for node in &nodes {
                if node.CurrentControlType()? == UIA_EditControlTypeId {
                    edits += 1;
                    if ["搜索", "Search"].contains(&node.CurrentName()?.to_string().as_str()) {
                        search = true;
                    }
                }
            }
        }
        Ok(
            json!({"nodes":nodes.len(),"edit_controls":edits,"has_current_chat_title":a.title(h).is_ok(),"has_chat_input":a.find(&root,UIA_AutomationIdPropertyId,"chat_input_field").is_ok(),"has_search_edit":search,"has_search_control":search||a.find(&root,UIA_NamePropertyId,"搜索").is_ok()||a.find(&root,UIA_NamePropertyId,"Search").is_ok()}),
        )
    })();
    report["checks"]["accessibility"] = checked.unwrap_or_else(|e| json!({"error":format!("{e}")}));
    report
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unbound_diagnostics_are_read_only_and_do_not_expose_configuration() {
        let d = tempfile::tempdir().unwrap();
        let c = Config::default(d.path().join("config.json"));
        let before = c.raw.clone();
        let report = collect(&c);
        let text = report.to_string();
        assert_eq!(report["checks"]["binding"]["valid"], false);
        assert!(!text.contains(c.token()));
        assert!(!text.contains("access_token"));
        assert_eq!(c.raw, before);
        assert!(!d.path().join("config.json").exists());
    }
}
