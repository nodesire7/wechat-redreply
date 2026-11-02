//! Guarded diagnostics for native music surfaces; never opens an external URL.
use crate::{config::Config, delivery::Automation, native};
use anyhow::{Context, Result, bail};
use serde_json::json;
pub fn probe(c: &Config, mode: &str, path: &std::path::Path) -> Result<()> {
    let h = native::validate_window(&c.binding()?)?;
    let a = Automation::new()?;
    let before = native::input_snapshot();
    let mut old_rect = windows::Win32::Foundation::RECT::default();
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowRect(h, &mut old_rect)?;
    }
    if mode != "restore_chat" {
        native::offscreen(h)?;
    }
    let old = a.title(h).ok();
    let _guard = crate::popup::Guard::install(h, &c.path.parent().unwrap().join("data"))?;
    a.cancel_actions(h, &c.path.parent().unwrap().join("data"));
    let old_popups = native::thread_windows(h);
    let result = (|| -> Result<_> {
        match mode {
            "restore_chat" => {
                // Repair only the explicitly bound, verified account window.
                // Position and resize without activating it or using user input.
                unsafe {
                    use windows::Win32::UI::WindowsAndMessaging::*;
                    SetWindowPos(
                        h,
                        None,
                        -30000,
                        -30000,
                        896,
                        648,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )?;
                }
                std::thread::sleep(std::time::Duration::from_millis(180));
                crate::friends::tab(&a, h, "微信")?;
                a.select(
                    h,
                    c.raw["native_test_target"]
                        .as_str()
                        .context("请设置 native_test_target")?,
                    false,
                )?;
                native::offscreen(h)?;
            }
            "discover" => crate::friends::tab(&a, h, "发现")?,
            "source" => a.select(
                h,
                c.raw["native_test_target"]
                    .as_str()
                    .context("请设置 native_test_target")?,
                false,
            )?,
            "card_menu" => {
                a.select(
                    h,
                    c.raw["native_test_target"]
                        .as_str()
                        .context("请设置 native_test_target")?,
                    false,
                )?;
                let row = a
                    .rows(h)?
                    .into_iter()
                    .filter(|r| unsafe {
                        r.CurrentName().is_ok_and(|s| {
                            s.to_string().contains("STAY (Clean)")
                                && s.to_string().contains("The Kid LAROI")
                        })
                    })
                    .last()
                    .ok_or_else(|| anyhow::anyhow!("样本音乐卡不可见"))?;
                let rect = unsafe { row.CurrentBoundingRectangle()? };
                let mut origin = windows::Win32::Foundation::POINT::default();
                unsafe {
                    windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut origin).ok()?;
                }
                let x = rect.left + 100 - origin.x;
                let y = (rect.top + rect.bottom) / 2 - origin.y;
                for (message, key) in [
                    (windows::Win32::UI::WindowsAndMessaging::WM_MOUSEMOVE, 0),
                    (windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONDOWN, 2),
                    (windows::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP, 0),
                ] {
                    unsafe {
                        windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            Some(h),
                            message,
                            windows::Win32::Foundation::WPARAM(key),
                            windows::Win32::Foundation::LPARAM(((y << 16) | (x & 0xffff)) as isize),
                        )?;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                std::thread::sleep(std::time::Duration::from_millis(180));
            }
            "settings" | "settings_general" => {
                let root = a.root(h)?;
                let more = a.find(
                    &root,
                    windows::Win32::UI::Accessibility::UIA_AutomationIdPropertyId,
                    "MainView.main_tabbar.tabbar_setting",
                )?;
                a.click_native(&more, h)?;
                std::thread::sleep(std::time::Duration::from_millis(150));
                let mut chosen = None;
                for ph in native::thread_windows(h) {
                    if let Ok(button) = a.find(
                        &a.root(ph)?,
                        windows::Win32::UI::Accessibility::UIA_NamePropertyId,
                        "设置",
                    ) {
                        chosen = Some((ph, button));
                        break;
                    }
                }
                let (ph, button) = chosen.ok_or_else(|| anyhow::anyhow!("微信设置入口未找到"))?;
                a.click_native(&button, ph)?;
                std::thread::sleep(std::time::Duration::from_millis(250));
                if mode == "settings_general" {
                    for ph in native::thread_windows(h) {
                        let root = a.root(ph)?;
                        if let Ok(general) = a.find(
                            &root,
                            windows::Win32::UI::Accessibility::UIA_NamePropertyId,
                            "通用",
                        ) {
                            a.click_native(&general, ph)?;
                            std::thread::sleep(std::time::Duration::from_millis(180));
                            break;
                        }
                    }
                }
            }
            "search" => {
                crate::friends::tab(&a, h, "微信")?;
                let root = a.root(h)?;
                let edit = a
                    .descendants(&root)?
                    .into_iter()
                    .find(|n| unsafe {
                        n.CurrentControlType().is_ok_and(|r| {
                            r == windows::Win32::UI::Accessibility::UIA_EditControlTypeId
                        }) && n.CurrentName().is_ok_and(|s| s.to_string() == "搜索")
                    })
                    .ok_or_else(|| anyhow::anyhow!("Search field absent"))?;
                let value: windows::Win32::UI::Accessibility::IUIAutomationValuePattern = unsafe {
                    edit.GetCurrentPatternAs(windows::Win32::UI::Accessibility::UIA_ValuePatternId)?
                };
                unsafe {
                    value.SetValue(&windows::core::BSTR::new())?;
                    a.click_native(&edit, h)?;
                    value.SetValue(&windows::core::BSTR::from("STAY"))?;
                }
                std::thread::sleep(std::time::Duration::from_millis(900));
            }
            _ => bail!("Unknown native music probe mode"),
        }
        let mut controls = vec![];
        let mut windows = vec![h];
        windows.extend(native::thread_windows(h));
        for window in windows {
            let root = a.root(window)?;
            for n in a.descendants(&root)? {
                unsafe {
                    let toggle=n.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationTogglePattern>(windows::Win32::UI::Accessibility::UIA_TogglePatternId).ok().and_then(|p|p.CurrentToggleState().ok()).map(|s|s.0);
                    let rect = n.CurrentBoundingRectangle()?;
                    controls.push(json!({"window":window.0 as usize,"main":window==h,"class":n.CurrentClassName()?.to_string(),"name":n.CurrentName()?.to_string(),"role":n.CurrentControlType()?.0,"aid":n.CurrentAutomationId()?.to_string(),"toggle":toggle,"rect":[rect.left,rect.top,rect.right,rect.bottom]}));
                }
            }
        }
        Ok(controls)
    })();
    if mode == "search" {
        if let Ok(root) = a.root(h) {
            if let Ok(edit) = a.find(
                &root,
                windows::Win32::UI::Accessibility::UIA_NamePropertyId,
                "搜索",
            ) {
                unsafe {
                    if let Ok(value)=edit.GetCurrentPatternAs::<windows::Win32::UI::Accessibility::IUIAutomationValuePattern>(windows::Win32::UI::Accessibility::UIA_ValuePatternId){let _=value.SetValue(&windows::core::BSTR::new());}
                }
            }
        }
    }
    for popup in native::thread_windows(h) {
        if !old_popups.contains(&popup) {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(popup),
                    windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            }
        }
    }
    if mode != "restore_chat" && native::validate_window(&c.binding()?).is_ok() {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::*;
            SetWindowPos(
                h,
                None,
                old_rect.left,
                old_rect.top,
                old_rect.right - old_rect.left,
                old_rect.bottom - old_rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )?;
        }
    }
    let _ = crate::friends::tab(&a, h, "微信");
    if let Some(old) = old {
        if let Some((name, group)) = c
            .scope()
            .into_iter()
            .find(|(n, g)| crate::delivery::title_matches(&old, n, *g))
        {
            let _ = a.select(h, &name, group);
        }
    }
    let after = native::input_snapshot();
    crate::config::atomic(
        path,
        &json!({"controls":result.as_ref().ok(),"error":result.as_ref().err().map(ToString::to_string),"clipboard_unchanged":before.clipboard==after.clipboard,"cursor_unchanged":before.cursor==after.cursor,"foreground_unchanged":before.foreground==after.foreground,"focus_unchanged":before.focus==after.focus}),
    )?;
    result?;
    Ok(())
}
