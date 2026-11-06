//! WebView2 shell. The visual surface is local HTML/CSS; all privileged work
//! stays in Rust behind a loopback JSON bridge protected by a per-launch token.
use crate::{
    config::{self, Config},
    native,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::WindowExtWindows,
    window::{ResizeDirection, WindowBuilder},
};
use wry::{WebContext, WebViewBuilder, WebViewBuilderExtWindows};

const HTML: &str = include_str!("../assets/gui/index.html");
const CSS: &str = include_str!("../assets/gui/app.css");
const JS: &str = include_str!("../assets/gui/app.js");
const NEXT: &str = include_str!("../assets/gui/next.html");
const AVATAR: &[u8] = include_bytes!("../assets/github-avatar.png");

#[derive(Debug)]
enum GuiEvent {
    Drag,
    Resize(ResizeDirection),
    Minimize,
    Maximize,
    Close,
}

struct Backend {
    config: Mutex<Config>,
    token: String,
    proxy: Mutex<Option<EventLoopProxy<GuiEvent>>>,
}

pub fn run(config: Config) -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let no_activate = args.iter().any(|s| s == "--no-activate");
    let log_page = args.iter().any(|s| s == "--log-page");
    if args.iter().any(|s| s == "--start-service") {
        let _ = crate::gui::start(&config);
    }
    let token: String = (0..32)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("绑定本地界面端口")?;
    let port = listener.local_addr()?.port();
    let software = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows::Win32::UI::WindowsAndMessaging::SM_REMOTESESSION,
        ) != 0
    };
    // An RDP session may reset its GPU/compositor without terminating our
    // message loop. Use software rendering for this GUI only, never OS-wide
    // environment variables or flags on the user's WeChat process.
    let mut context = WebContext::new(
        software.then(|| config.path.parent().unwrap().join("data/webview-software")),
    );
    crate::lifecycle::note(
        &config,
        "gui_renderer",
        if software {
            "GUI 使用远程桌面软件渲染，避免依赖会话 GPU；仅影响本程序"
        } else {
            "GUI 使用默认渲染后端"
        },
    );
    let backend = Arc::new(Backend {
        config: Mutex::new(config),
        token,
        proxy: Mutex::new(None),
    });
    {
        let backend = backend.clone();
        thread::Builder::new()
            .name("gui-web-bridge".into())
            .spawn(move || serve(backend, listener))
            .context("启动界面桥接线程")?;
    }

    let event_loop = EventLoopBuilder::<GuiEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    *backend.proxy.lock().unwrap() = Some(proxy);
    let window = WindowBuilder::new()
        .with_title("微信 · 红色问答")
        .with_decorations(false)
        .with_resizable(true)
        .with_inner_size(LogicalSize::new(1160., 780.))
        .with_min_inner_size(LogicalSize::new(960., 680.))
        .with_window_icon(window_icon()?)
        .build(&event_loop)
        .context("创建 WebView2 窗口")?;
    let url = format!(
        "http://127.0.0.1:{port}/?token={}{}",
        backend.token,
        if log_page { "&page=logs" } else { "" }
    );
    #[cfg(debug_assertions)]
    {
        let path = backend.config.lock().unwrap().path.clone();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir.join("data"));
            let _ = std::fs::write(dir.join("data/gui-web-url.txt"), &url);
        }
    }
    let mut builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(&url)
        .with_background_color((230, 242, 251, 255));
    if software {
        builder = builder.with_additional_browser_args("--disable-gpu");
    }
    let webview = builder.build(&window).context("创建 WebView2 控件")?;
    if no_activate {
        unsafe {
            let hwnd = windows::Win32::Foundation::HWND(window.hwnd() as _);
            let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE,
            );
            let _ = windows::Win32::UI::WindowsAndMessaging::SetWindowPos(
                hwnd,
                Some(windows::Win32::UI::WindowsAndMessaging::HWND_BOTTOM),
                0,
                0,
                0,
                0,
                windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                    | windows::Win32::UI::WindowsAndMessaging::SWP_NOACTIVATE,
            );
        }
    } else {
        window.set_visible(true);
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(GuiEvent::Drag) => {
                let _ = window.drag_window();
            }
            Event::UserEvent(GuiEvent::Resize(direction)) => {
                let _ = window.drag_resize_window(direction);
            }
            Event::UserEvent(GuiEvent::Minimize) => window.set_minimized(true),
            Event::UserEvent(GuiEvent::Maximize) => {
                window.set_maximized(!window.is_maximized());
            }
            Event::UserEvent(GuiEvent::Close) => {
                window.set_visible(false);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
        let _ = &webview;
        let _ = &context;
    });
}

fn window_icon() -> Result<Option<tao::window::Icon>> {
    let image = image::load_from_memory(AVATAR)?.into_rgba8();
    let (width, height) = image.dimensions();
    Ok(Some(tao::window::Icon::from_rgba(
        image.into_raw(),
        width,
        height,
    )?))
}

fn serve(backend: Arc<Backend>, listener: TcpListener) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let backend = backend.clone();
        let _ = thread::Builder::new()
            .name("gui-web-request".into())
            .spawn(move || {
                let _ = handle(&backend, stream);
            });
    }
}

fn read_request(
    stream: &mut TcpStream,
) -> Result<(String, String, Vec<(String, String)>, Vec<u8>)> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    let head_end;
    loop {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            bail!("连接提前关闭");
        }
        data.extend_from_slice(&buffer[..n]);
        if data.len() > 2 * 1024 * 1024 {
            bail!("请求过大");
        }
        if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            head_end = pos + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request = lines.next().context("缺少请求行")?;
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("/").to_owned();
    let mut headers = vec![];
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(n, _)| n == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < head_end + length {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buffer[..n]);
    }
    let body = data[head_end..(head_end + length).min(data.len())].to_vec();
    Ok((method, target, headers, body))
}

fn response_cookie(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
    cookie: &str,
) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nSet-Cookie: {cookie}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(())
}
fn response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) -> Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(())
}

fn json_response(stream: &mut TcpStream, value: &Value) -> Result<()> {
    response(
        stream,
        "200 OK",
        "application/json; charset=utf-8",
        value.to_string().as_bytes(),
    )
}

fn err_response(stream: &mut TcpStream, status: &str, error: impl std::fmt::Display) -> Result<()> {
    response(
        stream,
        status,
        "application/json; charset=utf-8",
        json!({"error": error.to_string()}).to_string().as_bytes(),
    )
}

fn handle(backend: &Backend, mut stream: TcpStream) -> Result<()> {
    let (method, target, headers, body) = read_request(&mut stream)?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let param = |name: &str| {
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == name).then_some(v.to_owned())
        })
    };
    let supplied = param("token")
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .iter()
                .find(|(n, _)| n == "x-token")
                .map(|(_, v)| v.clone())
                .filter(|s| !s.is_empty())
        })
        .or_else(|| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("cookie"))
                .and_then(|(_, v)| {
                    v.split(';').find_map(|part| {
                        part.trim()
                            .strip_prefix("gui_token=")
                            .map(|token| token.to_owned())
                    })
                })
        });
    if matches!(path, "/" | "/next" | "/classic")
        && supplied.as_deref() != Some(backend.token.as_str())
    {
        return err_response(
            &mut stream,
            "403 Forbidden",
            "会话未授权；请从程序窗口打开设置",
        );
    }
    match (method.as_str(), path) {
        ("GET", "/next") => {
            return response_cookie(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                NEXT.as_bytes(),
                &format!(
                    "gui_token={}; Path=/; HttpOnly; SameSite=Strict",
                    backend.token
                ),
            );
        }
        ("GET", "/classic") => {
            return response(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                HTML.as_bytes(),
            );
        }
        ("GET", "/") => {
            return response_cookie(
                &mut stream,
                "200 OK",
                "text/html; charset=utf-8",
                NEXT.as_bytes(),
                &format!(
                    "gui_token={}; Path=/; HttpOnly; SameSite=Strict",
                    backend.token
                ),
            );
        }
        ("GET", "/app.css") => {
            return response(
                &mut stream,
                "200 OK",
                "text/css; charset=utf-8",
                CSS.as_bytes(),
            );
        }
        ("GET", "/app.js") => {
            return response(
                &mut stream,
                "200 OK",
                "application/javascript; charset=utf-8",
                JS.as_bytes(),
            );
        }
        ("GET", "/avatar.png") => {
            return response(&mut stream, "200 OK", "image/png", AVATAR);
        }
        _ => {}
    }
    if supplied.as_deref() != Some(backend.token.as_str()) {
        return err_response(&mut stream, "403 Forbidden", "token 无效");
    }
    let value = if body.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice::<Value>(&body) {
            Ok(value) => value,
            Err(_) => return err_response(&mut stream, "400 Bad Request", "请求 JSON 无效"),
        }
    };
    let result = match (method.as_str(), path) {
        ("GET", "/api/state") => state_json(backend),
        ("GET", "/api/capabilities") => Ok(crate::compat::capabilities()),
        ("GET", "/api/status") => status_json(backend),
        ("GET", "/api/jobs") => jobs_json(backend),
        ("POST", "/api/recall") => recall_job(backend, &value),
        ("GET", "/api/windows") => windows_json(backend),
        ("POST", "/api/save") => save(backend, &value),
        ("POST", "/api/bind") => bind(backend, &value),
        ("POST", "/api/contacts") => contacts(backend),
        ("POST", "/api/start") => start(backend),
        ("POST", "/api/stop") => stop(backend),
        ("POST", "/api/copy") => copy(&value),
        ("POST", "/api/contact-id") => contact_id(backend, &value),
        ("POST", "/api/retry-friends") => retry_friends(backend),
        ("POST", "/api/window") => window_action(backend, &value),
        ("GET", "/api/logs") => logs(backend, param("filter").and_then(|s| s.parse().ok())),
        _ => Err(anyhow::anyhow!("未知接口")),
    };
    match result {
        Ok(value) => json_response(&mut stream, &value),
        Err(error) => err_response(&mut stream, "400 Bad Request", format!("{error:#}")),
    }
}

fn binding_json(c: &Config) -> Value {
    match c.binding() {
        Ok(b) => match native::validate_window(&b) {
            Ok(_) => {
                json!({"valid":true,"label":format!("已绑定 · PID {}",b.pid),"pid":b.pid,"hwnd":b.hwnd,"created_at":b.created_at})
            }
            Err(e) => json!({"valid":false,"label":"需要重新绑定","reason":format!("{e}")}),
        },
        Err(_) => json!({"valid":false,"label":"未绑定"}),
    }
}

fn config_json(c: &Config) -> Value {
    let ids = c
        .scope()
        .iter()
        .map(|(n, g)| {
            format!(
                "{n}：{}",
                if *g {
                    config::group_id(n)
                } else {
                    config::contact_id(n)
                }
            )
        })
        .collect::<Vec<_>>();
    json!({
        "listen_host": c.r()["listen_host"],
        "connect_host": c.r()["connect_host"],
        "port": c.r()["port"],
        "self_id": c.r()["self_id"],
        "access_token": c.token(),
        "management_url": c.raw["redreply_management_url"].as_str().unwrap_or(""),
        "tls_certfile": c.r()["tls_certfile"],
        "tls_keyfile": c.r()["tls_keyfile"],
        "whitelist": c.raw["whitelist"],
        "group_whitelist": c.raw["group_whitelist"],
        "interval": c.interval(),
        "recall_limit": crate::actions::recall_limit(&c.raw),
        "forward_staging_chat": c.raw["forward_staging_chat"].as_str().unwrap_or(""),
        "friend_accept": c.raw["friend_requests_auto_accept"].as_bool().unwrap_or(false),
        "music_link_mode": c.raw["music_link_mode"].as_str().unwrap_or("audio"),
        "scope_ids": ids,
        "url": c.url()
    })
}

fn state_json(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    let status = crate::gui::state(&c);
    Ok(json!({
        "config": config_json(&c),
        "binding": binding_json(&c),
        "status": status
    }))
}

fn jobs_json(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    let data = c.path.parent().unwrap().join("data");
    let jobs = crate::outbox::gui_jobs(&data, 200, crate::actions::recall_limit(&c.raw))?;
    Ok(json!({"jobs": jobs}))
}

fn recall_job(backend: &Backend, value: &Value) -> Result<Value> {
    let id = value["message_id"].as_u64().context("缺少 message_id")?;
    let c = backend.config.lock().unwrap().clone();
    let data = c.path.parent().unwrap().join("data");
    crate::outbox::gui_request_recall(&data, id, crate::actions::recall_limit(&c.raw))
}

fn status_json(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    let status = crate::gui::state(&c);
    let (outbox, outbox_error) =
        match crate::outbox::gui_summary(&c.path.parent().unwrap().join("data")) {
            Ok(value) => (value, Value::Null),
            Err(_) => (
                Value::Null,
                json!("发送统计暂不可用，请检查数据文件或稍后刷新"),
            ),
        };
    Ok(json!({
        "status": status,
        "binding": binding_json(&c),
        "outbox": outbox,
        "outbox_error": outbox_error,
        "version": env!("CARGO_PKG_VERSION")
    }))
}

fn windows_json(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    let windows = native::windows()
        .into_iter()
        .map(|w| {
            json!({
                "title": w.title,
                "pid": w.binding.pid,
                "hwnd": w.binding.hwnd,
                "created_at": w.binding.created_at,
                "offscreen": w.offscreen
            })
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"windows":windows,"bound":c.binding().ok().map(|b| json!({"pid":b.pid,"hwnd":b.hwnd,"created_at":b.created_at}))}),
    )
}

fn save(backend: &Backend, value: &Value) -> Result<Value> {
    let mut c = backend.config.lock().unwrap().clone();
    let old = c.clone();
    let rr = c.raw["redreply"]
        .as_object_mut()
        .context("配置缺少 redreply")?;
    if let Some(v) = value.get("listen_host").and_then(Value::as_str) {
        rr.insert("listen_host".into(), json!(v.trim()));
    }
    if let Some(v) = value.get("connect_host").and_then(Value::as_str) {
        rr.insert("connect_host".into(), json!(v.trim()));
    }
    if let Some(v) = value.get("port").and_then(Value::as_u64) {
        rr.insert("port".into(), json!(v));
    }
    if let Some(v) = value.get("self_id").and_then(Value::as_u64) {
        rr.insert("self_id".into(), json!(v));
    }
    if let Some(v) = value.get("access_token").and_then(Value::as_str) {
        rr.insert("access_token".into(), json!(v.trim()));
    }
    if let Some(v) = value.get("tls_certfile").and_then(Value::as_str) {
        rr.insert("tls_certfile".into(), json!(v.trim()));
    }
    if let Some(v) = value.get("tls_keyfile").and_then(Value::as_str) {
        rr.insert("tls_keyfile".into(), json!(v.trim()));
    }
    if let Some(v) = value.get("management_url").and_then(Value::as_str) {
        c.raw["redreply_management_url"] = json!(v.trim());
    }
    if let Some(v) = value.get("whitelist").and_then(Value::as_array) {
        c.raw["whitelist"] = json!(
            v.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
    }
    if let Some(v) = value.get("group_whitelist").and_then(Value::as_array) {
        c.raw["group_whitelist"] = json!(
            v.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        );
    }
    if let Some(v) = value.get("interval").and_then(Value::as_f64) {
        if !(0.05..=5.).contains(&v) {
            bail!("查询间隔须在 0.05–5 秒之间");
        }
        c.raw["database_poll_interval_seconds"] = json!(v);
    }
    if let Some(v) = value.get("recall_limit").and_then(Value::as_f64) {
        c.raw["recall_limit_seconds"] = json!(v.clamp(1., 120.));
    }
    if let Some(v) = value.get("forward_staging_chat").and_then(Value::as_str) {
        c.raw["forward_staging_chat"] = json!(v.trim());
    }
    if let Some(v) = value.get("friend_accept").and_then(Value::as_bool) {
        c.raw["friend_requests_auto_accept"] = json!(v);
    }
    if let Some(v) = value.get("music_link_mode").and_then(Value::as_str) {
        if !matches!(v, "audio" | "source") {
            bail!("音乐卡跳转方式无效");
        }
        c.raw["music_link_mode"] = json!(v);
    }
    c.raw["receive_backend"] = "database".into();
    c.raw["offscreen_mode"] = true.into();
    c.raw["require_fully_hidden"] = true.into();
    c.validate()?;
    config::atomic(&c.path, &c.raw)?;
    *backend.config.lock().unwrap() = c.clone();
    let restarting = old.r() != c.r() && crate::gui::state(&old)["running"] == true;
    if restarting {
        crate::gui::stop_and_wait(&c)
            .and_then(|_| crate::gui::start(&c))
            .context("配置已保存，但监听重启失败；请修正后重新启动")?;
    }
    Ok(json!({
        "config": config_json(&c),
        "binding": binding_json(&c),
        "restart_requested": restarting,
        "notice": if restarting {"设置已保存，正在重启监听以应用连接参数"} else {"设置已保存，白名单和查询间隔自动应用"}
    }))
}

fn bind(backend: &Backend, value: &Value) -> Result<Value> {
    let hwnd = value["hwnd"].as_i64().context("缺少窗口句柄")? as isize;
    let pid = value["pid"].as_u64().context("缺少窗口进程")? as u32;
    let window = native::windows()
        .into_iter()
        .find(|w| w.binding.hwnd == hwnd && w.binding.pid == pid)
        .context("所选微信窗口已变化，请刷新")?;
    native::validate_window(&window.binding)?;
    native::offscreen(native::validate_window(&window.binding)?)?;
    let mut c = backend.config.lock().unwrap().clone();
    let running = crate::gui::state(&c)["running"] == true;
    c.raw["wechat_binding"] = serde_json::to_value(&window.binding)?;
    config::atomic(&c.path, &c.raw)?;
    *backend.config.lock().unwrap() = c.clone();
    if running {
        crate::gui::stop_and_wait(&c)
            .and_then(|_| crate::gui::start(&c))
            .context("新账号已保存，但监听未能重启；请手动启动")?;
    }
    Ok(json!({
        "config": config_json(&c),
        "binding": binding_json(&c),
        "notice": "账号已绑定并保存；自聊会自动识别，无需再次保存。"
    }))
}

fn contacts(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    c.binding()?;
    let temp = c.path.parent().unwrap().join("data/contact-picker");
    let mut db = crate::database::Database::new(&c, &temp)?;
    let contacts = db
        .contact_choices()?
        .into_iter()
        .map(|(name, group)| json!({"name":name,"group":group}))
        .collect::<Vec<_>>();
    Ok(json!({"contacts":contacts}))
}

fn start(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    native::validate_window(&c.binding()?)?;
    crate::gui::start(&c)?;
    Ok(json!({"notice":"监听正在启动，微信数据库就绪后自动接收。"}))
}

fn stop(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    thread::Builder::new()
        .name("gui-web-stop".into())
        .spawn(move || {
            let _ = crate::gui::stop_and_wait(&c);
        })?;
    Ok(json!({"notice":"正在停止监听…"}))
}

fn copy(value: &Value) -> Result<Value> {
    let text = value["text"].as_str().context("缺少复制内容")?;
    crate::gui_clipboard::copy_text(text)?;
    Ok(json!({"ok":true}))
}

fn contact_id(backend: &Backend, value: &Value) -> Result<Value> {
    let name = value["name"].as_str().context("缺少联系人")?;
    let group = value["group"].as_bool().unwrap_or(false);
    let _ = backend;
    Ok(json!({"id":if group { config::group_id(name) } else { config::contact_id(name) }}))
}

fn retry_friends(backend: &Backend) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    crate::outbox::resume_friends(&c.path.parent().unwrap().join("data"))?;
    Ok(json!({"notice":"已重新检查好友申请；如果微信仍要求验证，将再次暂停。"}))
}

fn window_action(backend: &Backend, value: &Value) -> Result<Value> {
    let action = value["action"].as_str().unwrap_or("");
    let event = match action {
        "drag" => GuiEvent::Drag,
        "minimize" => GuiEvent::Minimize,
        "maximize" => GuiEvent::Maximize,
        "close" => GuiEvent::Close,
        "resize" => {
            let direction = match value["direction"].as_str().unwrap_or("") {
                "east" => ResizeDirection::East,
                "west" => ResizeDirection::West,
                "north" => ResizeDirection::North,
                "south" => ResizeDirection::South,
                "north-east" => ResizeDirection::NorthEast,
                "north-west" => ResizeDirection::NorthWest,
                "south-east" => ResizeDirection::SouthEast,
                "south-west" => ResizeDirection::SouthWest,
                _ => bail!("缩放方向无效"),
            };
            GuiEvent::Resize(direction)
        }
        _ => bail!("窗口动作无效"),
    };
    let proxy = backend
        .proxy
        .lock()
        .unwrap()
        .clone()
        .context("窗口尚未就绪")?;
    proxy.send_event(event).context("窗口已关闭")?;
    Ok(json!({"ok":true}))
}

fn logs(backend: &Backend, filter: Option<i32>) -> Result<Value> {
    let c = backend.config.lock().unwrap().clone();
    let filter = filter.unwrap_or(311);
    let data = c.path.parent().unwrap().join("data");
    let text = if filter == 318 {
        crate::outbox::gui_text(&data, c.token())
    } else {
        crate::telemetry::read(&data, filter)
    };
    Ok(json!({"text":text}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn exchange(backend: Arc<Backend>, request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let worker = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handle(&backend, stream).unwrap();
        });
        client.write_all(request.as_bytes()).unwrap();
        let mut result = String::new();
        client.read_to_string(&mut result).unwrap();
        worker.join().unwrap();
        result
    }
    fn backend(path: &std::path::Path) -> Arc<Backend> {
        let c = Config::default(path.join("config.json"));
        config::atomic(&c.path, &c.raw).unwrap();
        Arc::new(Backend {
            config: Mutex::new(c),
            token: "test-only-session".into(),
            proxy: Mutex::new(None),
        })
    }
    #[test]
    fn real_gui_http_cookie_refresh_and_errors_match_actual_results() {
        let d = tempfile::tempdir().unwrap();
        let backend = backend(d.path());
        let denied = exchange(
            backend.clone(),
            "GET /api/state HTTP/1.1\r\nHost: localhost\r\n\r\n",
        );
        assert!(denied.starts_with("HTTP/1.1 403"));
        let denied = exchange(backend.clone(), "GET / HTTP/1.1\r\nHost: localhost\r\n\r\n");
        assert!(denied.starts_with("HTTP/1.1 403"));
        let allowed = exchange(
            backend.clone(),
            "GET /api/state?token= HTTP/1.1\r\nHost: localhost\r\nX-Token: \r\nCookie: gui_token=test-only-session\r\n\r\n",
        );
        assert!(allowed.starts_with("HTTP/1.1 200"));
        let value: Value = serde_json::from_str(allowed.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(value["binding"]["valid"], false);
        let before = std::fs::read(d.path().join("config.json")).unwrap();
        let body = r#"{"port":0}"#;
        let invalid = exchange(
            backend.clone(),
            &format!(
                "POST /api/save HTTP/1.1\r\nHost: localhost\r\nCookie: gui_token=test-only-session\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            ),
        );
        assert!(invalid.starts_with("HTTP/1.1 400"));
        assert_eq!(before, std::fs::read(d.path().join("config.json")).unwrap());
        let body =
            r#"{"whitelist":["好友"],"group_whitelist":["同名群"],"music_link_mode":"audio"}"#;
        let ok = exchange(
            backend.clone(),
            &format!(
                "POST /api/save HTTP/1.1\r\nHost: localhost\r\nCookie: gui_token=test-only-session\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            ),
        );
        assert!(ok.starts_with("HTTP/1.1 200"));
        let saved = config::load(&d.path().join("config.json")).unwrap();
        assert_eq!(saved["whitelist"][0], "好友");
        assert_eq!(saved["group_whitelist"][0], "同名群");
        assert_eq!(saved["music_link_mode"], "audio");
    }
    #[test]
    fn unreadable_queue_statistics_never_become_zero_success_counts() {
        let d = tempfile::tempdir().unwrap();
        let backend = backend(d.path());
        assert_eq!(status_json(&backend).unwrap()["outbox"]["sent"], 0);
        std::fs::create_dir_all(d.path().join("data")).unwrap();
        std::fs::write(
            d.path().join("data/outbox.sqlite3"),
            b"invalid sqlite fixture",
        )
        .unwrap();
        let value = status_json(&backend).unwrap();
        assert!(value["outbox"].is_null());
        assert!(value["outbox_error"].as_str().unwrap().contains("不可用"));
    }
    #[test]
    fn live_listener_with_destroyed_wechat_window_is_not_reported_ready() {
        let d = tempfile::tempdir().unwrap();
        let backend = backend(d.path());
        let own = native::Process::open(std::process::id()).unwrap();
        config::atomic(
            &d.path().join("data/state.json"),
            &json!({
                "pid": std::process::id(), "created_at": own.creation().unwrap(),
                "updated_at": config::now(), "running": true, "connected": true,
                "ready": true, "error": ""
            }),
        )
        .unwrap();
        let status = status_json(&backend).unwrap();
        assert_eq!(status["status"]["running"], true);
        assert_eq!(status["binding"]["valid"], false);
        assert_eq!(
            status["status"]["ready"], false,
            "a live listener and readable old databases do not prove an active WeChat login"
        );
        assert!(!status["status"]["error"].as_str().unwrap().is_empty());
    }
}
