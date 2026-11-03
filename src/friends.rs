//! Friend requests use the same guarded background native window lane.
use crate::service::Shared;
use crate::{config::Config, delivery::Automation, native};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use windows::Win32::{Foundation::HWND, UI::Accessibility::*};

pub(crate) fn tab(a: &Automation, h: HWND, name: &str) -> Result<()> {
    let root = a.root(h)?;
    let tabs = a
        .descendants(&root)?
        .into_iter()
        .filter(|n| unsafe {
            n.CurrentName().is_ok_and(|s| s.to_string() == name)
                && n.CurrentClassName()
                    .is_ok_and(|s| s.to_string().contains("XTabBarItem"))
        })
        .collect::<Vec<_>>();
    if tabs.len() != 1 {
        bail!("微信{name}入口不唯一，未操作");
    }
    a.click_native(&tabs[0], h)?;
    std::thread::sleep(Duration::from_millis(180));
    Ok(())
}
fn enter(a: &Automation, h: HWND) -> Result<()> {
    tab(a, h, "通讯录")?;
    let root = a.root(h)?;
    if a.descendants(&root)?.iter().any(|n| unsafe {
        n.CurrentClassName()
            .is_ok_and(|s| s.to_string().contains("ContactsCellNewFriendView"))
    }) {
        return Ok(());
    }
    let entry = a
        .find(&root, UIA_NamePropertyId, "新的朋友")
        .context("新的朋友入口不可用")?;
    a.click_native(&entry, h)?;
    std::thread::sleep(Duration::from_millis(400));
    Ok(())
}
fn nodes(a: &Automation, h: HWND) -> Result<Value> {
    let root = a.root(h)?;
    let mut out = vec![];
    for node in a.descendants(&root)? {
        unsafe {
            let role = node.CurrentControlType()?.0;
            out.push(json!({"name":node.CurrentName()?.to_string(),"class":node.CurrentClassName()?.to_string(),"aid":node.CurrentAutomationId()?.to_string(),"role":role,"enabled":node.CurrentIsEnabled()?.as_bool()}));
        }
    }
    Ok(json!(out))
}
fn permission_row(
    a: &Automation,
    root: &IUIAutomationElement,
    name: &str,
) -> Result<IUIAutomationElement> {
    let matches = a
        .descendants(root)?
        .into_iter()
        .filter(|n| unsafe {
            n.CurrentName().is_ok_and(|s| s.to_string() == name)
                && n.CurrentClassName()
                    .is_ok_and(|s| s.to_string().contains("ProfileFormPermissionItemUi"))
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        bail!("朋友权限项目{name}不唯一");
    }
    Ok(matches[0].clone())
}
fn selected(a: &Automation, row: &IUIAutomationElement) -> Result<Option<bool>> {
    let mut controls = vec![row.clone()];
    controls.extend(a.descendants(row)?);
    for node in controls {
        unsafe {
            if let Ok(p) =
                node.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
            {
                return Ok(Some(p.CurrentToggleState()? == ToggleState_On));
            }
            if let Ok(p) = node.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                UIA_SelectionItemPatternId,
            ) {
                return Ok(Some(p.CurrentIsSelected()?.as_bool()));
            }
            if node.CurrentControlType()? == UIA_CheckBoxControlTypeId {
                if let Ok(p) = node.GetCurrentPatternAs::<IUIAutomationLegacyIAccessiblePattern>(
                    UIA_LegacyIAccessiblePatternId,
                ) {
                    return Ok(Some(p.CurrentState()? & 0x10 != 0));
                }
            }
        }
    }
    Ok(None)
}
fn permission_state(a: &Automation, root: &IUIAutomationElement) -> Result<Value> {
    let only = permission_row(a, root, "仅聊天")?;
    let all = permission_row(a, root, "聊天、朋友圈、微信运动等")?;
    Ok(json!({"only_chat":selected(a,&only)?,"all_permissions":selected(a,&all)?}))
}
fn verify_dialog(a: &Automation, h: HWND) -> Result<(HWND, IUIAutomationElement)> {
    let root = a.root(h)?;
    let verify = a
        .descendants(&root)?
        .into_iter()
        .filter(|n| unsafe {
            n.CurrentControlType()
                .is_ok_and(|r| r == UIA_ButtonControlTypeId)
                && n.CurrentName().is_ok_and(|s| s.to_string() == "前往验证")
        })
        .collect::<Vec<_>>();
    if verify.len() != 1 {
        bail!("前往验证按钮不唯一");
    }
    a.click_native(&verify[0], h)?;
    std::thread::sleep(Duration::from_millis(250));
    let mut candidates = vec![];
    for ph in native::thread_windows(h) {
        let root = a.root(ph)?;
        if a.find(&root, UIA_NamePropertyId, "通过朋友验证").is_ok() {
            candidates.push((ph, root));
        }
    }
    if candidates.len() != 1 {
        bail!("通过朋友验证窗口不唯一，未同意");
    }
    Ok(candidates.remove(0))
}
pub fn probe(c: &Config, report: &Path) -> Result<()> {
    probe_mode(c, report, true)
}
pub fn list(c: &Config, report: &Path) -> Result<()> {
    probe_mode(c, report, false)
}
fn probe_mode(c: &Config, report: &Path, open_dialog: bool) -> Result<()> {
    let h = native::validate_window(&c.binding()?)?;
    let a = Automation::new()?;
    let before = native::input_snapshot();
    let old = a.title(h).ok();
    let _guard = crate::popup::Guard::install(h, &c.path.parent().unwrap().join("data"))?;
    let old_popups = native::thread_windows(h);
    let result = (|| -> Result<Value> {
        enter(&a, h)?;
        let dialog = if open_dialog {
            verify_dialog(&a, h).ok()
        } else {
            None
        };
        let mut permission = Value::Null;
        if let Some((ph, root)) = dialog {
            let before = permission_state(&a, &root)?;
            let row = permission_row(&a, &root, "仅聊天")?;
            a.click_native(&row, ph)?;
            std::thread::sleep(Duration::from_millis(120));
            permission = json!({"before":before,"after":permission_state(&a,&root)?});
        }
        let mut all = nodes(&a, h)?.as_array().unwrap().clone();
        for popup in native::thread_windows(h) {
            all.extend(nodes(&a, popup)?.as_array().unwrap().clone());
        }
        Ok(json!({"controls":all,"permission":permission}))
    })();
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
    let _ = tab(&a, h, "微信");
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
    let value = json!({"nodes":result.as_ref().ok(),"error":result.as_ref().err().map(ToString::to_string),"clipboard_unchanged":before.clipboard==after.clipboard,"cursor_unchanged":before.cursor==after.cursor,"foreground_unchanged":before.foreground==after.foreground,"focus_unchanged":before.focus==after.focus});
    crate::config::atomic(report, &value)?;
    result?;
    Ok(())
}
fn require_only_chat(state: &Value) -> Result<()> {
    if state["only_chat"] != true || state["all_permissions"] != false {
        bail!("无法确认仅聊天已开启且完整权限已关闭，未同意申请");
    }
    Ok(())
}
// This version shows a separate positive acknowledgement after the server has
// accepted a friend. WM_CLOSE does not reliably dismiss that Qt notice.
fn acknowledge_success(a: &Automation, h: HWND) -> Result<bool> {
    let mut roots = vec![h];
    roots.extend(native::thread_windows(h));
    for window in roots {
        let root = a.root(window)?;
        let labels = a
            .descendants(&root)?
            .into_iter()
            .filter(|n| unsafe {
                n.CurrentName().is_ok_and(|s| s.to_string() == "已通过验证")
            })
            .collect::<Vec<_>>();
        for label in labels {
            unsafe {
                let walker = a.automation.RawViewWalker()?;
                let mut parent = walker.GetParentElement(&label).ok();
                for _ in 0..10 {
                    let Some(container) = parent else { break };
                    let nodes = a.descendants(&container)?;
                    if nodes.len() > 40 {
                        break;
                    }
                    let buttons = nodes
                        .into_iter()
                        .filter(|n| {
                            n.CurrentControlType()
                                .is_ok_and(|r| r == UIA_ButtonControlTypeId)
                                && n.CurrentName().is_ok_and(|s| s.to_string() == "确定")
                        })
                        .collect::<Vec<_>>();
                    if buttons.len() == 1 {
                        a.click_native(&buttons[0], window)?;
                        std::thread::sleep(Duration::from_millis(150));
                        return Ok(true);
                    }
                    parent = walker.GetParentElement(&container).ok();
                }
            }
        }
    }
    Ok(false)
}
fn restore(a: &Automation, h: HWND, c: &Config, old: Option<String>, old_popups: &[HWND]) {
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
    let _ = tab(a, h, "微信");
    if let Some(old) = old {
        if let Some((name, group)) = c
            .scope()
            .into_iter()
            .find(|(n, g)| crate::delivery::title_matches(&old, n, *g))
        {
            let _ = a.select(h, &name, group);
        }
    }
}
pub fn poll(shared: &Shared, a: &Automation) -> Result<usize> {
    use std::sync::atomic::Ordering;
    let c = shared.config.read().unwrap().clone();
    if !c.raw["friend_requests_auto_accept"]
        .as_bool()
        .unwrap_or(false)
    {
        return Ok(0);
    }
    if crate::outbox::friend_pause_reason(shared)?.is_some() {
        return Ok(0);
    }
    let binding = c.binding()?;
    let h = native::validate_window(&binding)?;
    if !native::windows()
        .iter()
        .any(|w| w.binding.hwnd == binding.hwnd && w.offscreen)
    {
        bail!("好友申请仅在绑定微信完全离屏时处理");
    }
    let current = crate::database::account_root(&c, &native::Process::bound(&binding)?)?;
    let identity = hex::encode(Sha256::digest(
        current
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_lowercase()
            .as_bytes(),
    ))[..24]
        .to_owned();
    if identity != crate::outbox::account(shared)? {
        bail!("好友申请账号与绑定账号不符");
    }
    let old = a.title(h).ok();
    let old_popups = native::thread_windows(h);
    let _guard = crate::popup::Guard::install(h, &shared.data)?;
    let mut key = None;
    let mut invoked = false;
    let result = (|| -> Result<usize> {
        let _ = acknowledge_success(a, h)?;
        enter(a, h)?;
        let root = a.root(h)?;
        let rows = a
            .descendants(&root)?
            .into_iter()
            .filter(|n| unsafe {
                n.CurrentClassName()
                    .is_ok_and(|s| s.to_string().contains("ContactsCellNewFriendView"))
                    && n.CurrentName()
                        .is_ok_and(|s| s.to_string().contains("等待验证"))
            })
            .collect::<Vec<_>>();
        for row in rows {
            let name = unsafe { row.CurrentName()?.to_string() };
            let request = hex::encode(Sha256::digest(format!("{identity}:{name}")));
            if crate::outbox::friend_status(shared, &request)?
                .is_some_and(|(_, at)| crate::config::now() - at < 300.)
            {
                continue;
            }
            key = Some(request.clone());
            a.click_native(&row, h)?;
            std::thread::sleep(Duration::from_millis(120));
            let profile = a
                .descendants(&a.root(h)?)?
                .into_iter()
                .find_map(|n| unsafe {
                    if n.CurrentClassName()
                        .is_ok_and(|s| s.to_string().contains("ContactHeadView"))
                    {
                        n.CurrentName().ok().map(|s| s.to_string())
                    } else {
                        None
                    }
                });
            let (ph, dialog) = verify_dialog(a, h)?;
            let only = permission_row(a, &dialog, "仅聊天")?;
            if selected(a, &only)? != Some(true) {
                a.click_native(&only, ph)?;
                std::thread::sleep(Duration::from_millis(120));
            }
            let state = permission_state(a, &dialog)?;
            require_only_chat(&state)?;
            let buttons = a
                .descendants(&dialog)?
                .into_iter()
                .filter(|n| unsafe {
                    n.CurrentControlType()
                        .is_ok_and(|r| r == UIA_ButtonControlTypeId)
                        && n.CurrentName().is_ok_and(|s| s.to_string() == "确定")
                })
                .collect::<Vec<_>>();
            if buttons.len() != 1 {
                bail!("好友验证确定按钮不唯一，未同意");
            }
            if !unsafe { buttons[0].CurrentIsEnabled()?.as_bool() } {
                bail!("好友验证确定按钮未启用，未同意");
            }
            let revised = shared.config.read().unwrap().clone();
            if shared.stop.load(Ordering::SeqCst)
                || !revised.raw["friend_requests_auto_accept"]
                    .as_bool()
                    .unwrap_or(false)
                || revised.binding()?.pid != binding.pid
            {
                bail!("设置已改变或正在停止，未同意");
            }
            require_only_chat(&permission_state(a, &dialog)?)?;
            let had_positive = a
                .find(&a.root(h)?, UIA_NamePropertyId, "已通过验证")
                .is_ok();
            crate::outbox::friend_note(shared, &request, "approving", "")?;
            shared.event("runtime",&json!({"text":"好友通过前已核对仅聊天权限","event":"friend_permission_verified","request_key":request,"only_chat":true,"all_permissions":false}));
            invoked = true;
            a.click_native(&buttons[0], ph)?;
            let until = std::time::Instant::now() + Duration::from_secs(3);
            let mut accepted = false;
            while std::time::Instant::now() < until {
                if acknowledge_success(a, h)? {
                    accepted = true;
                    break;
                }
                let mut roots = vec![h];
                roots.extend(native::thread_windows(h));
                for popup in roots {
                    let root = a.root(popup)?;
                    for node in a.descendants(&root)? {
                        let text = unsafe { node.CurrentName()?.to_string() };
                        if text.contains("当前账号存在安全风险") && text.contains("安全验证")
                        {
                            crate::outbox::pause_friends(shared, &text)?;
                            bail!(
                                "微信要求在微信团队完成账号安全验证；好友自动同意已暂停，消息监听继续"
                            );
                        }
                    }
                }
                let root = a.root(h)?;
                let positive = a.root(ph).ok().is_some_and(|dialog| {
                    a.find(&dialog, UIA_NamePropertyId, "已通过验证").is_ok()
                });
                let dialog_closed = !unsafe {
                    windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(ph).as_bool()
                };
                let same_profile = profile.as_ref().is_some_and(|name| {
                    a.descendants(&root).is_ok_and(|nodes| {
                        nodes.iter().any(|n| unsafe {
                            n.CurrentClassName()
                                .is_ok_and(|s| s.to_string().contains("ContactHeadView"))
                                && n.CurrentName().is_ok_and(|s| s.to_string() == *name)
                        })
                    })
                });
                let newly_positive = !had_positive
                    && same_profile
                    && a.find(&root, UIA_NamePropertyId, "已通过验证").is_ok();
                let stem = name.trim_end_matches("等待验证");
                let listed = a.descendants(&root)?.into_iter().any(|n| unsafe {
                    let name = n.CurrentName().map(|s| s.to_string()).unwrap_or_default();
                    (n.CurrentControlType()
                        .is_ok_and(|r| r == UIA_ButtonControlTypeId)
                        && ["发消息", "发送消息"].contains(&name.as_str()))
                        || (n
                            .CurrentClassName()
                            .is_ok_and(|s| s.to_string().contains("ContactsCellNewFriendView"))
                            && name.starts_with(stem)
                            && (name.ends_with("已添加") || name.ends_with("已通过")))
                });
                let chat = profile
                    .as_ref()
                    .is_some_and(|name| a.title(h).is_ok_and(|title| title == *name))
                    && a.find(&root, UIA_AutomationIdPropertyId, "chat_input_field")
                        .is_ok();
                accepted = positive || newly_positive || (dialog_closed && (listed || chat));
                if accepted {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if !accepted {
                bail!("同意好友已尝试，但未确认完成；不重复提交");
            }
            crate::outbox::friend_note(shared, &request, "approved_only_chat", "")?;
            shared.event("runtime",&json!({"text":format!("已同意好友申请并核对仅聊天权限，记录={}",&request[..12]),"event":"friend_approved","request_key":request,"only_chat":true,"all_permissions":false}));
            return Ok(1);
        }
        Ok(0)
    })();
    if result.is_err() {
        let mut all = nodes(a, h)
            .unwrap_or(Value::Null)
            .as_array()
            .cloned()
            .unwrap_or_default();
        for popup in native::thread_windows(h) {
            if let Ok(v) = nodes(a, popup) {
                all.extend(v.as_array().cloned().unwrap_or_default());
            }
        }
        let _ = crate::config::atomic(
            &shared.data.join("friend-last-error.json"),
            &json!({"controls":all,"error":result.as_ref().err().map(ToString::to_string)}),
        );
    }
    restore(a, h, &c, old, &old_popups);
    if let Err(e) = &result {
        if let Some(key) = key {
            crate::outbox::friend_note(
                shared,
                &key,
                if invoked { "uncertain" } else { "failed" },
                &e.to_string(),
            )?;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_or_full_permissions_never_pass_the_accept_gate() {
        for v in [
            json!({}),
            json!({"only_chat":true,"all_permissions":true}),
            json!({"only_chat":null,"all_permissions":false}),
            json!({"only_chat":false,"all_permissions":true}),
        ] {
            assert!(require_only_chat(&v).is_err());
        }
        assert!(require_only_chat(&json!({"only_chat":true,"all_permissions":false})).is_ok());
    }
}
