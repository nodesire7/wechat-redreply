//! Update our own settings process without synthesized user input. Refuse to
//! close a foreground editor or discard any settings that differ from disk.
use crate::{
    config::{self, Config},
    native,
};
use anyhow::{Result, bail};
use windows::Win32::UI::Controls::EM_GETMODIFY;
use windows::{
    Win32::{Foundation::*, System::Threading::*, UI::WindowsAndMessaging::*},
    core::w,
};
unsafe fn text(window: HWND, id: i32) -> Result<String> {
    unsafe {
        let control = GetDlgItem(Some(window), id)?;
        let mut buffer = vec![0u16; 32768];
        let mut written = 0;
        if SendMessageTimeoutW(
            control,
            WM_GETTEXT,
            WPARAM(buffer.len()),
            LPARAM(buffer.as_mut_ptr() as isize),
            SMTO_ABORTIFHUNG,
            3000,
            Some(&mut written),
        )
        .0 == 0
        {
            bail!(
                "设置窗口字段 {id} 未响应（Win32 {:?}），未关闭",
                GetLastError()
            );
        }
        Ok(String::from_utf16_lossy(
            &buffer[..written.min(buffer.len())],
        ))
    }
}
pub fn close_clean_settings(c: &Config) -> Result<bool> {
    let identity = config::contact_id(&c.path.canonicalize()?.to_string_lossy().to_lowercase());
    let executable = c
        .path
        .parent()
        .unwrap()
        .join("WechatRedReply.exe")
        .canonicalize()?;
    let state = config::load(&c.path.parent().unwrap().join("data/state.json")).unwrap_or_default();
    if state["running"] == true {
        bail!("请先停止监听以更新程序");
    }
    unsafe {
        let mut previous = HWND::default();
        while let Ok(window) =
            FindWindowExW(None, Some(previous), w!("WechatRedReplyRustSettings"), None)
        {
            if window.is_invalid() {
                break;
            }
            previous = window;
            if GetPropW(window, w!("WechatRedReplyConfig")).0 as usize != identity as usize {
                continue;
            }
            if GetForegroundWindow() == window {
                bail!("设置窗口当前正在前台使用，未关闭；请稍后更新");
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            let process = native::Process::open(pid)?;
            if std::path::PathBuf::from(process.image()?).canonicalize()? != executable {
                bail!("设置进程路径不匹配，未关闭");
            }
            let mut fields = vec![];
            for (id, key) in [(200, "whitelist"), (201, "group_whitelist")] {
                fields.push((
                    id,
                    c.raw[key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|v| v.as_str())
                        .collect::<Vec<_>>()
                        .join("\n"),
                ));
            }
            for (id, key) in [
                (500, "listen_host"),
                (501, "connect_host"),
                (502, "port"),
                (503, "access_token"),
                (504, "self_id"),
                (506, "tls_certfile"),
                (507, "tls_keyfile"),
            ] {
                let value = &c.r()[key];
                fields.push((
                    id,
                    value.as_str().map(str::to_owned).unwrap_or_else(|| {
                        if value.is_null() {
                            String::new()
                        } else {
                            value.to_string()
                        }
                    }),
                ));
            }
            fields.extend([
                (
                    505,
                    c.raw["redreply_management_url"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                ),
                (508, c.interval().to_string()),
                (509, crate::actions::recall_limit(&c.raw).to_string()),
                (
                    510,
                    c.raw["forward_staging_chat"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned(),
                ),
            ]);
            for (id, expected) in fields {
                if id == 503 {
                    // Password controls deliberately deny cross-process text
                    // reads. Inspect only their documented modified flag.
                    let control = GetDlgItem(Some(window), id)?;
                    let mut modified = 0;
                    if SendMessageTimeoutW(
                        control,
                        EM_GETMODIFY,
                        WPARAM(0),
                        LPARAM(0),
                        SMTO_ABORTIFHUNG,
                        1000,
                        Some(&mut modified),
                    )
                    .0 == 0
                        || modified != 0
                    {
                        bail!("令牌字段存在未保存修改或无法核对，未关闭设置窗口");
                    }
                    continue;
                }
                if text(window, id)?.replace("\r\n", "\n") != expected {
                    bail!("设置窗口有未保存内容（字段 {id}），未关闭，未丢弃修改");
                }
            }
            if text(window, 402)?.contains("待保存") {
                bail!("账号绑定尚未保存，未关闭");
            }
            if let Ok(control) = GetDlgItem(Some(window), 511) {
                let checked = SendMessageW(control, BM_GETCHECK, None, None).0 != 0;
                if checked
                    != c.raw["friend_requests_auto_accept"]
                        .as_bool()
                        .unwrap_or(false)
                {
                    bail!("好友申请设置尚未保存，未关闭");
                }
            }
            // The window is inactive and every editable value matches disk.
            // Terminating this GUI avoids creating a confirmation dialog or
            // touching the foreground. The service has already stopped.
            let handle = native::Handle(OpenProcess(
                PROCESS_TERMINATE | PROCESS_SYNCHRONIZE,
                false,
                pid,
            )?);
            crate::lifecycle::note(
                c,
                "gui_maintenance_close",
                &format!("维护命令将关闭已停止监听、配置已保存且非前台的设置进程 PID={pid}"),
            );
            TerminateProcess(handle.0, 0)?;
            let _ = WaitForSingleObject(handle.0, 2000);
            return Ok(true);
        }
    }
    Ok(false)
}
pub fn capture_settings(c: &Config, output: &std::path::Path) -> Result<()> {
    let identity = config::contact_id(&c.path.canonicalize()?.to_string_lossy().to_lowercase());
    unsafe {
        let mut previous = HWND::default();
        while let Ok(window) =
            FindWindowExW(None, Some(previous), w!("WechatRedReplyRustSettings"), None)
        {
            if window.is_invalid() {
                break;
            }
            previous = window;
            if GetPropW(window, w!("WechatRedReplyConfig")).0 as usize == identity as usize {
                let args = std::env::args().collect::<Vec<_>>();
                let option = |key: &str| {
                    args.iter()
                        .position(|s| s == key)
                        .and_then(|i| args.get(i + 1))
                        .and_then(|s| s.parse::<u32>().ok())
                };
                let page = option("--settings-page");
                let filter = option("--settings-filter");
                let minimum = args.iter().any(|s| s == "--settings-minimum");
                let width = option("--settings-width");
                let height = option("--settings-height");
                let tls = args.iter().any(|s| s == "--settings-toggle-wss");
                let alter = page.is_some()
                    || filter.is_some()
                    || minimum
                    || width.is_some()
                    || height.is_some()
                    || tls;
                if alter && GetForegroundWindow() == window {
                    bail!("设置窗口正在前台使用，未改变页面或大小");
                }
                let old_page =
                    (GetPropW(window, w!("WechatRedReplyPage")).0 as usize).clamp(100, 104);
                let raw_filter = GetPropW(window, w!("WechatRedReplyLogFilter")).0 as usize;
                let old_filter = if raw_filter == 0 { 311 } else { raw_filter };
                let mut old_rect = RECT::default();
                GetWindowRect(window, &mut old_rect)?;
                if let Some(page) = page {
                    if !(100..=104).contains(&page) {
                        bail!("Invalid settings page");
                    }
                    SendMessageW(
                        window,
                        WM_COMMAND,
                        Some(WPARAM(page as usize)),
                        Some(LPARAM(0)),
                    );
                }
                if let Some(filter) = filter {
                    if ![311, 312, 313, 314, 318].contains(&filter) {
                        bail!("Invalid settings filter");
                    }
                    SendMessageW(
                        window,
                        WM_COMMAND,
                        Some(WPARAM(filter as usize)),
                        Some(LPARAM(0)),
                    );
                }
                if tls {
                    SendMessageW(window, WM_COMMAND, Some(WPARAM(319)), Some(LPARAM(0)));
                }
                if minimum {
                    let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(window) as f64 / 96.;
                    SetWindowPos(
                        window,
                        None,
                        old_rect.left,
                        old_rect.top,
                        (1020. * scale) as i32,
                        (800. * scale) as i32,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )?;
                }
                if let (Some(width), Some(height)) = (width, height) {
                    if !(1020..=2000).contains(&width) || !(800..=1400).contains(&height) {
                        bail!("Capture dimensions out of supported range");
                    }
                    let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(window) as f64 / 96.;
                    SetWindowPos(
                        window,
                        None,
                        old_rect.left,
                        old_rect.top,
                        (width as f64 * scale) as i32,
                        (height as f64 * scale) as i32,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )?;
                }
                // Cross-thread positioning can still be pending when the
                // caller first reads GetWindowRect. Drain the settings thread
                // before allocating a capture bitmap, including at min size.
                let mut acknowledged = 0;
                if SendMessageTimeoutW(
                    window,
                    WM_NULL,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    2000,
                    Some(&mut acknowledged),
                )
                .0 == 0
                {
                    bail!("设置窗口未完成布局，未保存可能失真的截图");
                }
                let _ = windows::Win32::Graphics::Gdi::RedrawWindow(
                    Some(window),
                    None,
                    None,
                    windows::Win32::Graphics::Gdi::RDW_INVALIDATE
                        | windows::Win32::Graphics::Gdi::RDW_UPDATENOW
                        | windows::Win32::Graphics::Gdi::RDW_ALLCHILDREN
                        | windows::Win32::Graphics::Gdi::RDW_FRAME,
                );
                std::thread::sleep(std::time::Duration::from_millis(180));
                if args.iter().any(|s| s == "--settings-layout-report") {
                    let mut controls = Vec::new();
                    for id in [
                        403, 305, 306, 633, 634, 500, 501, 502, 504, 503, 505, 304, 319, 506, 507,
                    ] {
                        if let Ok(control) = GetDlgItem(Some(window), id) {
                            let mut r = RECT::default();
                            GetWindowRect(control, &mut r)?;
                            controls.push(serde_json::json!({"id":id,"visible":IsWindowVisible(control).as_bool(),"rect":[r.left,r.top,r.right,r.bottom]}));
                        }
                    }
                    let combo = GetDlgItem(Some(window), 403)?;
                    config::atomic(
                        &output.with_extension("layout.json"),
                        &serde_json::json!({"controls":controls,"selection_height":SendMessageW(combo, CB_GETITEMHEIGHT, Some(WPARAM(usize::MAX)),None).0,"dpi":windows::Win32::UI::HiDpi::GetDpiForWindow(window)}),
                    )?;
                }
                let result = native::capture_gdi(window).and_then(|image| {
                    if let Some(path) = args
                        .iter()
                        .position(|s| s == "--settings-physical-copy")
                        .and_then(|i| args.get(i + 1))
                    {
                        image.save(path)?;
                    }
                    if args.iter().any(|s| s == "--settings-logical-image") {
                        // Keep a comparison capture in device-independent
                        // pixels, while ordinary captures retain physical pixels.
                        let dpi = windows::Win32::UI::HiDpi::GetDpiForWindow(window).max(96);
                        let normalized = image::imageops::resize(
                            &image,
                            (image.width() * 96 / dpi).max(1),
                            (image.height() * 96 / dpi).max(1),
                            image::imageops::FilterType::Lanczos3,
                        );
                        normalized.save(output)?;
                    } else {
                        image.save(output)?;
                    }
                    Ok(())
                });
                if alter {
                    if tls {
                        SendMessageW(window, WM_COMMAND, Some(WPARAM(319)), Some(LPARAM(0)));
                    }
                    SendMessageW(window, WM_COMMAND, Some(WPARAM(old_page)), Some(LPARAM(0)));
                    if old_filter != 0 {
                        SendMessageW(
                            window,
                            WM_COMMAND,
                            Some(WPARAM(old_filter)),
                            Some(LPARAM(0)),
                        );
                    }
                    if minimum || width.is_some() || height.is_some() {
                        let _ = SetWindowPos(
                            window,
                            None,
                            old_rect.left,
                            old_rect.top,
                            old_rect.right - old_rect.left,
                            old_rect.bottom - old_rect.top,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                }
                return result;
            }
        }
    }
    bail!("没有找到此配置的设置窗口")
}
