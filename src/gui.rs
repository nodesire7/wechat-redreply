//! Native Windows settings, sharing one EXE with the background listener.
#[path = "gui_theme.rs"]
mod theme;
use crate::{
    config::{self, Config},
    native,
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::*,
            HiDpi::*,
            Input::KeyboardAndMouse::EnableWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::*,
        },
    },
    core::{PCWSTR, w},
};
const PRIVATE: i32 = 200;
const GROUP: i32 = 201;
const STATUS: i32 = 400;
const FONT_SIZES: [f64; 4] = [20., 36., 30., 20.];
const CHROME_TOP: i32 = 31;
const TITLEBAR_HEIGHT: i32 = 40;
const NOTICE: i32 = 401;
enum Work {
    Notice(String),
    Contacts(Vec<(String, bool)>),
}
struct App {
    config: Config,
    controls: HashMap<i32, HWND>,
    pages: HashMap<i32, Vec<i32>>,
    page: i32,
    root: HWND,
    font: HFONT,
    heading: HFONT,
    section: HFONT,
    small: HFONT,
    paint: theme::Paint,
    tls_expanded: bool,
    windows: Vec<native::Window>,
    dirty: bool,
    loading: bool,
    scale: f64,
    work: Option<mpsc::Receiver<std::result::Result<Work, String>>>,
    contacts: Vec<(String, bool)>,
    log_filter: i32,
    log_paused: bool,
    log_text: String,
}
impl App {
    unsafe fn add(
        &mut self,
        id: i32,
        class: PCWSTR,
        text: &str,
        style: WINDOW_STYLE,
        page: Option<i32>,
    ) -> Result<()> {
        unsafe {
            let s = config::wide(text);
            let h = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                PCWSTR(s.as_ptr()),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                100,
                24,
                Some(self.root),
                Some(HMENU(id as usize as _)),
                None,
                None,
            )
            .with_context(|| format!("创建原生控件 {id}"))?;
            SendMessageW(
                h,
                WM_SETFONT,
                Some(WPARAM(self.font.0 as usize)),
                Some(LPARAM(1)),
            );
            self.controls.insert(id, h);
            if class == w!("BUTTON") && id != 511 {
                let _ = SetWindowSubclass(h, Some(theme::button_proc), 1, 0);
            }
            if class == w!("EDIT") {
                SendMessageW(
                    h,
                    EM_SETMARGINS,
                    Some(WPARAM(EC_LEFTMARGIN as usize | EC_RIGHTMARGIN as usize)),
                    Some(LPARAM(1 | (1 << 16))),
                );
            }
            if let Some(p) = page {
                self.pages.entry(p).or_default().push(id);
            }
            Ok(())
        }
    }
    unsafe fn label(&mut self, id: i32, text: &str, page: Option<i32>) -> Result<()> {
        unsafe { self.add(id, w!("STATIC"), text, WINDOW_STYLE(0), page) }
    }
    unsafe fn button(&mut self, id: i32, text: &str, page: Option<i32>) -> Result<()> {
        unsafe {
            self.add(
                id,
                w!("BUTTON"),
                text,
                WS_TABSTOP | WINDOW_STYLE(BS_OWNERDRAW as u32),
                page,
            )
        }
    }
    unsafe fn edit(&mut self, id: i32, multi: bool, page: Option<i32>) -> Result<()> {
        unsafe {
            self.add(
                id,
                w!("EDIT"),
                "",
                WS_TABSTOP
                    | if multi {
                        WS_VSCROLL
                            | WINDOW_STYLE((ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32)
                    } else {
                        WINDOW_STYLE(ES_AUTOHSCROLL as u32)
                    },
                page,
            )
        }
    }
    unsafe fn put(&self, id: i32, s: &str) {
        unsafe {
            let s = config::wide(s);
            let _ = SetWindowTextW(self.controls[&id], PCWSTR(s.as_ptr()));
        }
    }
    unsafe fn get(&self, id: i32) -> String {
        unsafe {
            let h = self.controls[&id];
            let mut text = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
            let n = GetWindowTextW(h, &mut text);
            String::from_utf16_lossy(&text[..n as usize])
        }
    }
    unsafe fn init(&mut self) -> Result<()> {
        unsafe {
            self.label(600, "账号与接入", None)?;
            self.label(630, "绑定微信，并连接你的红色问答", None)?;
            self.label(633, "微信 未绑定", Some(101))?;
            self.label(634, "RedReply 未连接", Some(101))?;
            self.label(632, "RedReply 连接", Some(101))?;
            self.label(635, "仅绑定所选账号", Some(101))?;
            self.label(636, "选择已登录的微信窗口", Some(101))?;
            SendMessageW(
                self.controls[&600],
                WM_SETFONT,
                Some(WPARAM(self.heading.0 as usize)),
                Some(LPARAM(1)),
            );
            self.label(STATUS, "正在读取运行状态…", None)?;
            self.button(300, "启动监听", None)?;
            self.button(301, "停止监听", None)?;
            for (id, text) in [
                (100, "接管范围"),
                (101, "账号与接入"),
                (102, "高级设置"),
                (103, "日志与对话"),
                (104, "发送队列"),
            ] {
                self.button(id, text, None)?;
            }
            for (id, text) in [(650, "最小化"), (651, "最大化"), (652, "关闭")] {
                self.button(id, text, None)?;
            }
            self.label(NOTICE, "关闭设置窗口不会停止监听。", None)?;
            self.button(302, "保存并应用", None)?;
            self.button(303, "重新载入", None)?;
            self.label(610, "私聊白名单", Some(100))?;
            self.label(611, "群聊白名单", Some(100))?;
            self.label(
                612,
                "每行一个完整备注名或群名；空列表表示不接管该类会话。",
                Some(100),
            )?;
            self.edit(PRIVATE, true, Some(100))?;
            self.edit(GROUP, true, Some(100))?;
            self.add(
                405,
                w!("COMBOBOX"),
                "",
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
                Some(100),
            )?;
            self.button(308, "读取联系人", Some(100))?;
            self.button(309, "加入白名单", Some(100))?;
            self.button(310, "复制选中 ID", Some(100))?;
            self.label(613, "", Some(100))?;
            self.label(
                614,
                "名称必须完全一致；群成员按真实标识区分，不继承私聊主人权限。",
                Some(100),
            )?;
            for (id, label) in [
                (500, "监听 IP"),
                (501, "对外连接地址"),
                (502, "端口"),
                (503, "接入令牌"),
                (504, "机器人 ID"),
                (505, "RedReply 管理地址"),
                (506, "WSS 证书（可选）"),
                (507, "WSS 私钥（可选）"),
            ] {
                self.label(id + 200, label, Some(101))?;
                self.edit(id, false, Some(101))?;
            }
            self.button(319, "WSS 证书与私钥（可选）", Some(101))?;
            SendMessageW(
                self.controls[&503],
                EM_SETPASSWORDCHAR,
                Some(WPARAM('●' as usize)),
                None,
            );
            self.button(307, "显示令牌", Some(101))?;
            self.button(304, "复制 OneBot 地址", Some(101))?;
            self.label(620, "微信账号", Some(101))?;
            self.add(
                403,
                w!("COMBOBOX"),
                "",
                WS_TABSTOP
                    | WS_VSCROLL
                    | WINDOW_STYLE((CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32),
                Some(101),
            )?;
            self.button(305, "刷新窗口", Some(101))?;
            self.button(306, "绑定账号", Some(101))?;
            self.label(402, "未绑定", Some(101))?;
            self.label(621, "接收方式：数据库增量监听（Rust 原生）", Some(102))?;
            self.label(622, "数据库查询间隔（秒）：", Some(102))?;
            self.edit(508, false, Some(102))?;
            self.label(625, "合并转发中转自聊：", Some(102))?;
            self.edit(510, false, Some(102))?;
            let cue = config::wide("自动识别当前账号，留空即可");
            SendMessageW(
                self.controls[&510],
                EM_SETCUEBANNER,
                Some(WPARAM(0)),
                Some(LPARAM(cue.as_ptr() as isize)),
            );
            self.label(626, "撤回上限（秒，最多120）：", Some(102))?;
            self.edit(509, false, Some(102))?;
            self.add(
                511,
                w!("BUTTON"),
                "自动同意好友申请，权限仅聊天",
                WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                Some(102),
            )?;
            self.button(512, "安全验证后重试好友申请", Some(102))?;
            self.label(623,"离屏模式已启用：窗口放在所有显示器之外，不最小化。\n自动收发不访问剪贴板、不移动鼠标、不抢前台。",Some(102))?;
            self.label(624,"配置、日志、缓存位于 EXE 旁；迁移后需要重新绑定微信。\n支持原生合并转发和两分钟内撤回；引用和艾特转换为正文，语音、文件暂不支持。",Some(102))?;
            self.edit(404, true, Some(103))?;
            for (id, label) in [
                (311, "全部"),
                (312, "对话记录"),
                (313, "协议明细"),
                (314, "运行状态"),
                (318, "发送队列"),
                (315, "暂停刷新"),
                (317, "复制当前日志"),
            ] {
                self.button(id, label, Some(103))?;
            }
            self.label(
                627,
                "收发原文、成员、目标、回包和耗时；图片只记录摘要，令牌自动隐藏。",
                Some(103),
            )?;
            self.label(628, "持久化队列：正在读取状态…", Some(103))?;
            SendMessageW(self.controls[&404], EM_SETREADONLY, Some(WPARAM(1)), None);
            SendMessageW(
                self.controls[&404],
                EM_SETLIMITTEXT,
                Some(WPARAM(2 * 1024 * 1024)),
                None,
            );
            self.fill();
            self.refresh();
            for id in [620, 632] {
                SendMessageW(
                    self.controls[&id],
                    WM_SETFONT,
                    Some(WPARAM(self.section.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            for id in [
                STATUS, NOTICE, 630, 633, 634, 635, 636, 402, 612, 613, 614, 623, 624, 627, 628,
            ] {
                SendMessageW(
                    self.controls[&id],
                    WM_SETFONT,
                    Some(WPARAM(self.small.0 as usize)),
                    Some(LPARAM(1)),
                );
            }
            // Dialog navigation follows the same order as the visible pages.
            // Hidden controls are skipped by IsDialogMessage.
            for id in [
                101, 100, 104, 103, 102, 403, 305, 306, 500, 501, 502, 504, 503, 307, 505, 304,
                319, 506, 507, 405, 308, 309, 310, 200, 201, 508, 510, 509, 511, 512, 311, 312,
                313, 314, 315, 317, 404, 300, 301, 303, 302,
            ] {
                SetWindowPos(
                    self.controls[&id],
                    Some(HWND_BOTTOM),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )?;
            }
            self.show(if std::env::args().any(|s| s == "--log-page") {
                103
            } else {
                101
            });
            SetTimer(Some(self.root), 1, 1000, None);
            self.status();
            if std::env::args().any(|s| s == "--start-service") {
                self.command(300, BN_CLICKED as u16);
            }
            Ok(())
        }
    }
    unsafe fn fill(&mut self) {
        unsafe {
            self.loading = true;
            for (id, key) in [(200, "whitelist"), (201, "group_whitelist")] {
                self.put(
                    id,
                    &self.config.raw[key]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("\r\n"),
                );
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
                let v = &self.config.r()[key];
                self.put(
                    id,
                    &v.as_str().map(str::to_owned).unwrap_or_else(|| {
                        if v.is_null() {
                            String::new()
                        } else {
                            v.to_string()
                        }
                    }),
                );
            }
            self.put(
                505,
                self.config.raw["redreply_management_url"]
                    .as_str()
                    .unwrap_or(""),
            );
            self.put(508, &self.config.interval().to_string());
            SendMessageW(
                self.controls[&511],
                BM_SETCHECK,
                Some(WPARAM(
                    if self.config.raw["friend_requests_auto_accept"]
                        .as_bool()
                        .unwrap_or(false)
                    {
                        BST_CHECKED.0 as usize
                    } else {
                        BST_UNCHECKED.0 as usize
                    },
                )),
                None,
            );
            self.put(
                509,
                &crate::actions::recall_limit(&self.config.raw).to_string(),
            );
            self.put(
                510,
                self.config.raw["forward_staging_chat"]
                    .as_str()
                    .unwrap_or(""),
            );
            self.put(
                402,
                &self
                    .config
                    .binding()
                    .map(|b| {
                        if native::validate_window(&b).is_ok() {
                            format!("已绑定 · PID {}", b.pid)
                        } else {
                            "需要重新绑定".into()
                        }
                    })
                    .unwrap_or_else(|_| "未绑定".to_owned()),
            );
            self.put(
                613,
                &self
                    .config
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
                    .collect::<Vec<_>>()
                    .join("　"),
            );
            self.dirty = false;
            self.loading = false;
        }
    }
    unsafe fn collect(&self) -> Result<Config> {
        unsafe {
            let mut c = self.config.clone();
            for (id, key) in [(200, "whitelist"), (201, "group_whitelist")] {
                let mut names = vec![];
                for n in self.get(id).lines() {
                    let n = n.trim().to_owned();
                    if !n.is_empty() && !names.contains(&n) {
                        names.push(n);
                    }
                }
                c.raw[key] = json!(names);
            }
            for (id, key) in [
                (500, "listen_host"),
                (501, "connect_host"),
                (503, "access_token"),
                (506, "tls_certfile"),
                (507, "tls_keyfile"),
            ] {
                c.raw["redreply"][key] = self.get(id).trim().into();
            }
            c.raw["redreply"]["port"] = self
                .get(502)
                .trim()
                .parse::<u64>()
                .context("端口应为整数")?
                .into();
            c.raw["redreply"]["self_id"] = self
                .get(504)
                .trim()
                .parse::<u64>()
                .context("机器人 ID 应为整数")?
                .into();
            c.raw["redreply_management_url"] = self.get(505).trim().into();
            let interval = self
                .get(508)
                .trim()
                .parse::<f64>()
                .context("查询间隔应为数字")?;
            if !(0.05..=5.).contains(&interval) {
                bail!("查询间隔须在 0.05–5 秒之间");
            }
            c.raw["database_poll_interval_seconds"] = json!(interval);
            let limit = self
                .get(509)
                .trim()
                .parse::<f64>()
                .context("撤回上限应为秒数")?;
            if !limit.is_finite() {
                bail!("撤回上限必须是有效数字");
            }
            c.raw["recall_limit_seconds"] = json!(limit.clamp(1., 120.));
            c.raw["forward_staging_chat"] = self.get(510).trim().into();
            c.raw["friend_requests_auto_accept"] = json!(
                SendMessageW(self.controls[&511], BM_GETCHECK, None, None).0
                    == BST_CHECKED.0 as isize
            );
            c.raw["receive_backend"] = "database".into();
            c.raw["offscreen_mode"] = true.into();
            c.raw["require_fully_hidden"] = true.into();
            c.validate()?;
            Ok(c)
        }
    }
    unsafe fn show(&mut self, page: i32) {
        unsafe {
            self.page = page;
            let _ = SetPropW(
                self.root,
                w!("WechatRedReplyPage"),
                Some(HANDLE(page as usize as *mut _)),
            );
            for (p, ids) in &self.pages {
                for id in ids {
                    let _ = ShowWindow(
                        self.controls[id],
                        if *p == page { SW_SHOW } else { SW_HIDE },
                    );
                }
            }
            if page == 104 {
                for id in [404, 628] {
                    let _ = ShowWindow(self.controls[&id], SW_SHOW);
                }
                self.log_filter = 318;
            } else if page == 103 && self.log_filter == 318 {
                self.log_filter = 311;
            }
            self.put(
                600,
                match page {
                    100 => "接管范围",
                    101 => "账号与接入",
                    102 => "高级设置",
                    104 => "发送队列",
                    _ => "日志与对话",
                },
            );
            self.put(
                630,
                match page {
                    100 => "选择允许红色问答接管的私聊和群聊",
                    101 => "绑定微信，并连接你的红色问答",
                    102 => "自动识别优先，需要时再调整",
                    104 => "查看已入队回复的实际发送和撤回结果",
                    _ => "保留消息原文，核对每一次收发",
                },
            );
            let _ = InvalidateRect(Some(self.root), None, false);
            for id in 100..=104 {
                let _ = InvalidateRect(Some(self.controls[&id]), None, false);
            }
            self.layout();
        }
    }
    unsafe fn place(&self, id: i32, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            if let Some(c) = self.controls.get(&id) {
                let s = self.scale;
                let y = if matches!(id, 650..=652) {
                    y
                } else {
                    y + CHROME_TOP
                };
                let _ = MoveWindow(
                    *c,
                    (x as f64 * s) as _,
                    (y as f64 * s) as _,
                    (w as f64 * s) as _,
                    (h as f64 * s) as _,
                    true,
                );
            }
        }
    }
    fn side_width(&self) -> i32 {
        unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.root, &mut r);
            if r.right as f64 / self.scale < 1100. {
                192
            } else {
                ((r.right as f64 / self.scale / 6.).round() as i32).min(264)
            }
        }
    }
    fn account_rect(&self) -> RECT {
        unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.root, &mut r);
            let v = self.vertical();
            theme::rect(
                self.side_width() + 28,
                (167. * v) as i32,
                (r.right as f64 / self.scale) as i32 - self.side_width() - 56,
                ((if self.tls_expanded { 96. } else { 124. }) * v) as i32,
            )
        }
    }
    fn vertical(&self) -> f64 {
        unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.root, &mut r);
            let content = (r.bottom as f64 / self.scale - CHROME_TOP as f64).max(600.);
            ((content - 140.) / 674.).clamp(0.90, 1.35)
        }
    }
    unsafe fn field(&self, id: i32, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            self.place(id, x + 9, y + 4, w - 18, h - 8);
        }
    }
    unsafe fn layout(&self) {
        unsafe {
            let mut r = RECT::default();
            let _ = GetClientRect(self.root, &mut r);
            let w = (r.right as f64 / self.scale) as i32;
            let h = ((r.bottom as f64 / self.scale) as i32 - CHROME_TOP).max(600);
            let side = self.side_width();
            let left = side + 28;
            let width = w - left - 28;
            let v = self.vertical();
            let y = |n: i32| (n as f64 * v) as i32;
            let bottom = h - 152;
            for (i, id) in [101, 100, 104, 103, 102].iter().enumerate() {
                self.place(*id, 12, 52 + i as i32 * 62, side - 24, 54);
            }
            for (i, id) in [650, 651, 652].iter().enumerate() {
                self.place(*id, w - 44 - 44 * (3 - i as i32), 0, 44, TITLEBAR_HEIGHT);
            }
            self.place(STATUS, 22, h - 54, side - 38, 38);
            self.place(600, left, y(36), width, 48);
            self.place(630, left, y(86), width, 28);
            self.place(NOTICE, left, h - 128, width, 24);
            self.place(300, left, h - 95, 184, 52);
            self.place(301, left + 196, h - 95, 132, 52);
            self.place(303, w - 402, h - 95, 176, 52);
            self.place(302, w - 208, h - 95, 180, 52);
            self.place(633, left + 29, y(134), 180, 26);
            self.place(634, left + 234, y(134), 260, 26);
            let card = self.account_rect();
            let cy = card.top;
            let offset = (width as f64 * 0.098) as i32;
            let selection = (width as f64 * 0.386) as i32;
            self.place(
                620,
                left + offset,
                cy + y(if self.tls_expanded { 8 } else { 18 }),
                selection - offset - 20,
                36,
            );
            self.place(
                402,
                left + offset,
                cy + y(if self.tls_expanded { 44 } else { 54 }),
                selection - offset - 20,
                26,
            );
            self.place(
                635,
                left + offset,
                cy + y(if self.tls_expanded { 66 } else { 84 }),
                selection - offset - 20,
                26,
            );
            let select = left + selection;
            let compact = width < 1000;
            let refresh_w = if compact { 120 } else { 164 };
            let bind_w = if compact { 120 } else { 155 };
            let gap = 10;
            let reserve = gap + refresh_w + gap + bind_w + gap + 18;
            let select_width = (width - selection - reserve).max(150);
            self.place(
                636,
                select,
                cy + y(if self.tls_expanded { 0 } else { 12 }),
                width - selection - 16,
                26,
            );
            let operation_y = cy + y(if self.tls_expanded { 26 } else { 36 });
            self.place(403, select, operation_y, select_width, 230);
            SendMessageW(
                self.controls[&403],
                CB_SETITEMHEIGHT,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM((y(42) as f64 * self.scale) as isize - 6)),
            );
            SendMessageW(
                self.controls[&403],
                CB_SETDROPPEDWIDTH,
                Some(WPARAM((width as f64 * self.scale) as usize)),
                None,
            );
            let refresh_x = select + select_width + 24;
            let bind_x = refresh_x + refresh_w + 14;
            self.place(305, refresh_x, operation_y, refresh_w, y(44));
            self.place(306, bind_x, operation_y, bind_w, y(44));
            let top = if self.tls_expanded { 280 } else { 312 };
            let field_h = if self.tls_expanded { 36 } else { 44 };
            let row = |expanded: i32, collapsed: i32| {
                if self.tls_expanded {
                    expanded
                } else {
                    collapsed
                }
            };
            let third = (width - 36) / 3;
            let half = (width - 18) / 2;
            self.place(632, left, y(top), width, 34);
            for (id, x, fw) in [
                (500, left, third),
                (501, left + third + 18, third),
                (502, left + 2 * (third + 18), width - 2 * (third + 18)),
            ] {
                self.place(id + 200, x, y(top + row(40, 44)), fw, 26);
                self.field(id, x, y(top + row(66, 66)), fw, y(field_h));
            }
            self.place(704, left, y(top + row(118, 120)), half, 26);
            self.field(504, left, y(top + row(144, 152)), half, y(field_h));
            self.place(703, left + half + 18, y(top + row(118, 120)), half, 26);
            self.field(
                503,
                left + half + 18,
                y(top + row(144, 152)),
                half - 46,
                y(field_h),
            );
            self.place(307, w - 68, y(top + row(144, 152)), 40, y(field_h));
            self.place(705, left, y(top + row(192, 222)), width, 26);
            self.field(505, left, y(top + row(218, 244)), width - 250, y(field_h));
            self.place(304, w - 266, y(top + row(218, 244)), 238, y(field_h));
            self.place(319, left, y(top + row(270, 294)), width, y(38));
            for id in [506, 507, 706, 707] {
                let _ = ShowWindow(
                    self.controls[&id],
                    if self.page == 101 && self.tls_expanded {
                        SW_SHOW
                    } else {
                        SW_HIDE
                    },
                );
            }
            for (id, x) in [(506, left), (507, left + half + 18)] {
                self.place(id + 200, x, y(top + 318), half, 26);
                self.field(id, x, y(top + 348), half, y(34));
            }
            // Whitelist: choose once, add/save once; manual editors remain available.
            self.place(612, left, 126, width, 24);
            self.place(405, left, 170, (width - 396).max(100), 220);
            self.place(308, w - 412, 168, 118, 38);
            self.place(309, w - 284, 168, 122, 38);
            self.place(310, w - 152, 168, 124, 38);
            self.place(610, left, 230, half, 28);
            self.place(611, left + half + 18, 230, half, 28);
            self.field(200, left, 268, half, (bottom - 302).max(140));
            self.field(201, left + half + 18, 268, half, (bottom - 302).max(140));
            self.place(613, left, bottom - 22, width, 38);
            self.place(614, left, 214, 1, 1);
            let _ = ShowWindow(self.controls[&614], SW_HIDE);
            self.place(621, left, 140, width, 28);
            self.place(622, left, 198, 270, 26);
            self.field(508, left + 280, 192, 140, 36);
            self.place(625, left, 256, 270, 26);
            self.field(510, left + 280, 250, (width - 280).min(390), 36);
            self.place(626, left, 314, 270, 26);
            self.field(509, left + 280, 308, 140, 36);
            self.place(511, left, 372, width, 28);
            self.place(512, left, 420, 286, 38);
            self.place(623, left, 486, width, 56);
            self.place(624, left, 552, width, 58);
            for (i, id) in [311, 312, 313, 314].iter().enumerate() {
                self.place(*id, left + i as i32 * 100, 132, 94, 36);
            }
            let _ = ShowWindow(self.controls[&318], SW_HIDE);
            self.place(315, w - 312, 132, 108, 36);
            self.place(317, w - 194, 132, 166, 36);
            self.place(627, left, 186, width, 25);
            self.place(
                628,
                left,
                if self.page == 104 { 132 } else { 220 },
                width,
                30,
            );
            self.field(
                404,
                left,
                if self.page == 104 { 182 } else { 262 },
                width,
                (bottom - if self.page == 104 { 190 } else { 270 }).max(120),
            );
            let _ = InvalidateRect(Some(self.root), None, false);
        }
    }
    unsafe fn refresh(&mut self) {
        unsafe {
            self.windows = native::windows();
            let h = self.controls[&403];
            SendMessageW(h, CB_RESETCONTENT, None, None);
            let bound = self.config.binding().ok();
            for (i, w) in self.windows.iter().enumerate() {
                let text = config::wide(format!(
                    "{}　PID {}　{}",
                    w.title,
                    w.binding.pid,
                    if w.offscreen {
                        "已离屏"
                    } else {
                        "当前可见"
                    }
                ));
                SendMessageW(h, CB_ADDSTRING, None, Some(LPARAM(text.as_ptr() as isize)));
                if bound
                    .as_ref()
                    .is_some_and(|b| b.hwnd == w.binding.hwnd && b.pid == w.binding.pid)
                {
                    SendMessageW(h, CB_SETCURSEL, Some(WPARAM(i)), None);
                }
            }
            if self.windows.len() == 1 {
                SendMessageW(h, CB_SETCURSEL, Some(WPARAM(0)), None);
            }
        }
    }
    fn begin(&mut self, job: impl FnOnce() -> Result<Work> + Send + 'static) -> Result<()> {
        if self.work.is_some() {
            bail!("上一步操作尚未完成，请稍候");
        }
        let (tx, rx) = mpsc::channel();
        self.work = Some(rx);
        std::thread::spawn(move || {
            let result = job().map_err(|e| format!("{e:#}"));
            let _ = tx.send(result);
        });
        Ok(())
    }
    unsafe fn status(&mut self) {
        unsafe {
            if let Some(result) = self.work.as_ref().and_then(|rx| rx.try_recv().ok()) {
                self.work = None;
                match result {
                    Ok(Work::Notice(text)) => self.put(NOTICE, &text),
                    Ok(Work::Contacts(items)) => {
                        self.contacts = items;
                        let combo = self.controls[&405];
                        SendMessageW(combo, CB_RESETCONTENT, None, None);
                        for (name, group) in &self.contacts {
                            let text = config::wide(format!(
                                "{}　{name}",
                                if *group { "群聊" } else { "私聊" }
                            ));
                            SendMessageW(
                                combo,
                                CB_ADDSTRING,
                                None,
                                Some(LPARAM(text.as_ptr() as isize)),
                            );
                        }
                        if !self.contacts.is_empty() {
                            SendMessageW(combo, CB_SETCURSEL, Some(WPARAM(0)), None);
                        }
                        self.put(
                            NOTICE,
                            &format!(
                                "已读取 {} 个联系人；选择后可加入白名单或复制 ID。",
                                self.contacts.len()
                            ),
                        );
                    }
                    Err(error) => self.put(NOTICE, &error),
                }
            }
            let state = state(&self.config);
            let running = state["running"] == true;
            let text = if running {
                format!(
                    "运行中 · {}",
                    if state["connected"] == true {
                        "已连接"
                    } else {
                        "等待 RedReply"
                    }
                )
            } else {
                "监听已停止".to_owned()
            };
            self.put(STATUS, &text);
            let stopped_queue = if !running {
                crate::outbox::gui_summary(&self.config.path.parent().unwrap().join("data"))
                    .unwrap_or_default()
            } else {
                Value::Null
            };
            let q = if running {
                &state["outbox"]
            } else {
                &stopped_queue
            };
            let n = |key: &str| q[key].as_u64().unwrap_or(0);
            self.put(
                628,
                &format!(
                    "发送队列：待发 {} · 准备 {} · 发送 {} · 成功 {} · 失败 {} · 待核对 {}",
                    n("queued"),
                    n("preparing"),
                    n("sending"),
                    n("sent"),
                    n("failed"),
                    n("uncertain")
                ),
            );
            if q["friend_paused_reason"].as_str().is_some() {
                self.put(512, "安全验证后重试好友申请");
                self.put(
                    624,
                    "好友自动同意已暂停：微信要求账号安全验证。\n请先在微信团队完成验证，再点击上方重试按钮。",
                );
            } else {
                self.put(512, "重新检查好友申请");
                self.put(624,"好友通过时核对“仅聊天”，不自动加入回复白名单。\n配置位于 EXE 旁，迁移后重新绑定；引用/艾特转正文，语音和文件暂不支持。");
            }
            let notice = self.get(NOTICE);
            if state["ready"] == true
                && (notice.starts_with("监听正在启动")
                    || notice == "微信数据库已就绪，等待 RedReply 连接。")
            {
                self.put(
                    NOTICE,
                    if state["connected"] == true {
                        "监听已运行，配置已应用。关闭本窗口不会停止监听。"
                    } else {
                        "微信数据库已就绪，等待 RedReply 连接。"
                    },
                );
            }
            let bound = self
                .config
                .binding()
                .is_ok_and(|b| native::validate_window(&b).is_ok());
            let _ = EnableWindow(
                self.controls[&300],
                !running && bound && self.work.is_none(),
            );
            let _ = EnableWindow(self.controls[&301], running && self.work.is_none());
            let _ = EnableWindow(self.controls[&302], self.work.is_none());
            let _ = EnableWindow(self.controls[&308], self.work.is_none());
            self.put(
                633,
                if bound {
                    "微信 已绑定"
                } else {
                    "微信 未绑定"
                },
            );
            self.put(
                634,
                if running && state["connected"] == true {
                    "RedReply 已连接"
                } else {
                    "RedReply 未连接"
                },
            );
            self.put(
                300,
                if self.dirty {
                    "保存并启动"
                } else {
                    "启动监听"
                },
            );
            if !bound && self.get(NOTICE) == "关闭设置窗口不会停止监听。" {
                self.put(NOTICE, "绑定微信账号后即可启动监听。");
            }
            let _ = ShowWindow(self.controls[&301], if running { SW_SHOW } else { SW_HIDE });
            let _ = EnableWindow(
                self.controls[&306],
                self.work.is_none() && !self.windows.is_empty(),
            );
            let _ = EnableWindow(self.controls[&305], self.work.is_none());
            if let Some(e) = state["error"].as_str().filter(|e| !e.is_empty()) {
                self.put(NOTICE, e);
            }
            if matches!(self.page, 103 | 104) && !self.log_paused {
                let text = if self.log_filter == 318 {
                    crate::outbox::gui_text(
                        &self.config.path.parent().unwrap().join("data"),
                        self.config.token(),
                    )
                } else {
                    crate::telemetry::read(
                        &self.config.path.parent().unwrap().join("data"),
                        self.log_filter,
                    )
                };
                let text = if text.is_empty() {
                    "尚无此类记录。启动监听后会载入白名单会话的近期文字记录，并实时显示新消息及调用结果。".into()
                } else {
                    text
                };
                if self.log_text != text {
                    let edit = self.controls[&404];
                    let first = SendMessageW(edit, EM_GETFIRSTVISIBLELINE, None, None).0;
                    let count = SendMessageW(edit, EM_GETLINECOUNT, None, None).0;
                    let mut rect = RECT::default();
                    let _ = GetClientRect(edit, &mut rect);
                    let bottom =
                        first + ((rect.bottom - rect.top) as f64 / (16. * self.scale)) as isize + 2
                            >= count;
                    self.put(404, &text);
                    if bottom {
                        let length = text.encode_utf16().count();
                        SendMessageW(
                            edit,
                            EM_SETSEL,
                            Some(WPARAM(length)),
                            Some(LPARAM(length as isize)),
                        );
                        SendMessageW(edit, EM_SCROLLCARET, None, None);
                        // An inactive read-only editor has no native caret.
                        // Scroll its viewport directly without taking focus.
                        let lines = SendMessageW(edit, EM_GETLINECOUNT, None, None).0;
                        SendMessageW(edit, EM_LINESCROLL, None, Some(LPARAM(lines)));
                    } else {
                        SendMessageW(edit, EM_LINESCROLL, None, Some(LPARAM(first)));
                    }
                    self.log_text = text;
                }
            }
        }
    }
    unsafe fn command(&mut self, id: i32, n: u16) {
        unsafe {
            if n == EN_CHANGE as u16 && !self.loading && matches!(id, 200 | 201 | 500..=510) {
                self.dirty = true;
                return;
            }
            if n != BN_CLICKED as u16 {
                return;
            }
            let result = (|| -> Result<String> {
                match id {
                    650 => {
                        let _ = ShowWindow(self.root, SW_MINIMIZE);
                        return Ok(String::new());
                    }
                    651 => {
                        let _ = ShowWindow(
                            self.root,
                            if IsZoomed(self.root).as_bool() {
                                SW_RESTORE
                            } else {
                                SW_MAXIMIZE
                            },
                        );
                        return Ok(String::new());
                    }
                    652 => {
                        let _ = PostMessageW(Some(self.root), WM_CLOSE, WPARAM(0), LPARAM(0));
                        return Ok(String::new());
                    }
                    100..=104 => {
                        self.show(id);
                        return Ok(String::new());
                    }
                    300 => {
                        let c = self.collect()?;
                        native::validate_window(&c.binding()?)
                            .context("请先在本页绑定一个已登录的微信账号")?;
                        config::atomic(&c.path, &c.raw)?;
                        self.config = c.clone();
                        self.fill();
                        self.begin(move || {
                            start(&c)?;
                            Ok(Work::Notice(
                                "监听正在启动，微信数据库就绪后自动接收。".into(),
                            ))
                        })?;
                    }
                    301 => {
                        let c = self.config.clone();
                        self.begin(move || {
                            stop_and_wait(&c)?;
                            Ok(Work::Notice("监听已停止。".into()))
                        })?;
                    }
                    302 => {
                        let c = self.collect()?;
                        let old = Config::read(self.config.path.clone())?;
                        let restart = c.r() != old.r()
                            || c.raw["wechat_binding"] != old.raw["wechat_binding"];
                        let running = state(&c)["running"] == true;
                        config::atomic(&c.path, &c.raw)?;
                        self.config = c;
                        self.fill();
                        if running && restart {
                            let c = self.config.clone();
                            self.begin(move || {
                                stop_and_wait(&c)?;
                                start(&c)?;
                                Ok(Work::Notice("设置已保存，监听已按新参数重新启动。".into()))
                            })?;
                            return Ok("设置已保存，正在重启监听…".into());
                        }
                        return Ok("设置已保存，白名单和查询间隔自动应用".into());
                    }
                    303 => {
                        if self.dirty
                            && MessageBoxW(
                                Some(self.root),
                                w!("放弃未保存修改并重新载入？"),
                                w!("重新载入"),
                                MB_YESNO | MB_ICONQUESTION,
                            ) != IDYES
                        {
                            return Ok(String::new());
                        }
                        self.config = Config::read(self.config.path.clone())?;
                        self.fill();
                    }
                    304 => {
                        let c = self.collect()?;
                        let restart = c.r() != self.config.r()
                            || c.raw["wechat_binding"] != self.config.raw["wechat_binding"];
                        let running = state(&self.config)["running"] == true;
                        config::atomic(&c.path, &c.raw)?;
                        self.config = c.clone();
                        self.fill();
                        crate::gui_clipboard::copy_text(&c.url())?;
                        if running && restart {
                            self.begin(move || {
                                stop_and_wait(&c)?;
                                start(&c)?;
                                Ok(Work::Notice(
                                    "接入地址已复制，监听已按新参数重新启动。".into(),
                                ))
                            })?;
                        }
                        return Ok("完整接入地址已复制，包含令牌，请勿公开".into());
                    }
                    305 => self.refresh(),
                    306 => {
                        let i = SendMessageW(self.controls[&403], CB_GETCURSEL, None, None).0;
                        if i < 0 {
                            bail!("请明确选择一个微信窗口");
                        }
                        let b = self.windows[i as usize].binding.clone();
                        let mut c = self.collect()?;
                        c.raw["wechat_binding"] = serde_json::to_value(&b)?;
                        native::offscreen(native::validate_window(&b)?)?;
                        let running = state(&self.config)["running"] == true;
                        config::atomic(&c.path, &c.raw)?;
                        self.config = c.clone();
                        self.fill();
                        if running {
                            self.begin(move || {
                                stop_and_wait(&c)?;
                                start(&c)?;
                                Ok(Work::Notice("账号已绑定并保存，监听已重新启动。".into()))
                            })?;
                        }
                        return Ok("账号已绑定并保存；自聊会自动识别，无需再次保存。".into());
                    }
                    319 => {
                        self.tls_expanded = !self.tls_expanded;
                        self.layout();
                        return Ok(String::new());
                    }
                    307 => {
                        let h = self.controls[&503];
                        let old = SendMessageW(h, EM_GETPASSWORDCHAR, None, None).0;
                        SendMessageW(
                            h,
                            EM_SETPASSWORDCHAR,
                            Some(WPARAM(if old == 0 { '●' as usize } else { 0 })),
                            None,
                        );
                        let _ = InvalidateRect(Some(h), None, true);
                    }
                    311..=314 | 318 => {
                        self.log_filter = id;
                        let _ = SetPropW(
                            self.root,
                            w!("WechatRedReplyLogFilter"),
                            Some(HANDLE(id as usize as *mut _)),
                        );
                        self.log_text.clear();
                        self.status();
                        return Ok(String::new());
                    }
                    315 => {
                        self.log_paused = !self.log_paused;
                        self.put(
                            315,
                            if self.log_paused {
                                "继续刷新"
                            } else {
                                "暂停刷新"
                            },
                        );
                        return Ok(String::new());
                    }
                    317 => {
                        crate::gui_clipboard::copy_text(&self.get(404))?;
                        return Ok("当前日志已复制。".into());
                    }
                    511 => {
                        self.dirty = true;
                        return Ok("好友申请设置待保存；通过时强制核对仅聊天权限".into());
                    }
                    512 => {
                        if !self.config.raw["friend_requests_auto_accept"]
                            .as_bool()
                            .unwrap_or(false)
                        {
                            bail!("请先启用并保存自动同意好友申请");
                        }
                        if self.dirty {
                            bail!("请先保存好友申请设置");
                        }
                        let c = self.config.clone();
                        self.begin(move || {
                            crate::outbox::resume_friends(&c.path.parent().unwrap().join("data"))?;
                            Ok(Work::Notice(
                                "已重新检查好友申请；如果微信仍要求验证，将再次暂停。".into(),
                            ))
                        })?;
                        return Ok("正在恢复好友申请检查…".into());
                    }
                    308 => {
                        let c = self.config.clone();
                        self.begin(move || {
                            let cache = c.path.parent().unwrap().join("data/contacts.json");
                            if let Ok(v) = config::load(&cache) {
                                if v["binding"] == c.raw["wechat_binding"] {
                                    return Ok(Work::Contacts(serde_json::from_value(
                                        v["items"].clone(),
                                    )?));
                                }
                            }
                            let temp = c.path.parent().unwrap().join("data/contact-picker");
                            let mut db = crate::database::Database::new(&c, &temp)?;
                            Ok(Work::Contacts(db.contact_choices()?))
                        })?;
                        return Ok("正在读取绑定账号的联系人…".into());
                    }
                    309 | 310 => {
                        let index = SendMessageW(self.controls[&405], CB_GETCURSEL, None, None).0;
                        let (name, group) = self
                            .contacts
                            .get(index as usize)
                            .context("请先读取并选择联系人")?;
                        if id == 310 {
                            crate::gui_clipboard::copy_text(
                                &if *group {
                                    config::group_id(name)
                                } else {
                                    config::contact_id(name)
                                }
                                .to_string(),
                            )?;
                            return Ok("选中联系人的 ID 已复制。".into());
                        }
                        let field = if *group { GROUP } else { PRIVATE };
                        let mut names = self
                            .get(field)
                            .lines()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .collect::<Vec<_>>();
                        if !names.contains(name) {
                            names.push(name.clone());
                            self.put(field, &names.join("\r\n"));
                            self.dirty = true;
                        }
                        let c = self.collect()?;
                        config::atomic(&c.path, &c.raw)?;
                        self.config = c;
                        self.fill();
                        return Ok("已加入白名单并保存。".into());
                    }
                    _ => {}
                }
                Ok("操作已完成".into())
            })();
            match result {
                Ok(s) if !s.is_empty() => self.put(NOTICE, &s),
                Err(e) => self.put(NOTICE, &format!("{e:#}")),
                _ => {}
            }
            self.status();
        }
    }
}
pub(crate) fn state(c: &Config) -> Value {
    let mut state =
        config::load(&c.path.parent().unwrap().join("data/state.json")).unwrap_or(json!({}));
    if config::now() - state["updated_at"].as_f64().unwrap_or(0.) > 10. {
        state["running"] = false.into();
    }
    if state["running"] == true {
        let pid = state["pid"].as_u64().unwrap_or(0) as u32;
        if !native::Process::open(pid).is_ok_and(|p| {
            p.image()
                .is_ok_and(|image| PathBuf::from(image) == std::env::current_exe().unwrap())
                && p.creation()
                    .is_ok_and(|at| (at - state["created_at"].as_f64().unwrap_or(0.)).abs() < 0.01)
        }) {
            state["running"] = false.into();
        }
    }
    state
}
pub(crate) fn start(c: &Config) -> Result<()> {
    use std::os::windows::process::CommandExt;
    if state(c)["running"] == true {
        return Ok(());
    }
    c.binding()?;
    let host = if c.host() == "0.0.0.0" {
        "127.0.0.1"
    } else {
        c.host()
    };
    let addr = std::net::SocketAddr::new(host.parse()?, c.port());
    if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
        bail!("端口已被占用，请先停止旧监听；未启动第二个发送端");
    }
    Command::new(std::env::current_exe()?)
        .args(["--service", "--config"])
        .arg(&c.path)
        .args(["--gui-pid", &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()?;
    Ok(())
}
fn request_stop(c: &Config) -> Result<()> {
    std::fs::create_dir_all(c.path.parent().unwrap().join("data"))?;
    std::fs::write(c.path.parent().unwrap().join("data/stop.request"), b"stop")?;
    Ok(())
}
pub(crate) fn stop_and_wait(c: &Config) -> Result<()> {
    request_stop(c)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(35);
    while state(c)["running"] == true {
        if std::time::Instant::now() >= deadline {
            bail!("监听尚未完成停止，请稍后重试");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}
pub fn run(config: Config) -> Result<()> {
    crate::lifecycle::note(&config, "gui_start", "设置窗口入口启动");
    unsafe {
        let no_activate = std::env::args().any(|s| s == "--no-activate");
        let identity = config::contact_id(
            &config
                .path
                .canonicalize()
                .unwrap_or_else(|_| config.path.clone())
                .to_string_lossy()
                .to_lowercase(),
        );
        let mutex = config::wide(format!("Local\\WechatRedReplyRustGui-{identity}"));
        let handle =
            windows::Win32::System::Threading::CreateMutexW(None, false, PCWSTR(mutex.as_ptr()))?;
        let _instance = native::Handle(handle);
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let mut previous = HWND::default();
            while let Ok(existing) =
                FindWindowExW(None, Some(previous), w!("WechatRedReplyRustSettings"), None)
            {
                if existing.is_invalid() {
                    break;
                }
                previous = existing;
                if GetPropW(existing, w!("WechatRedReplyConfig")).0 as usize == identity as usize {
                    if !no_activate {
                        if IsIconic(existing).as_bool() {
                            let _ = ShowWindow(existing, SW_RESTORE);
                        }
                        let _ = SetForegroundWindow(existing);
                    }
                    return Ok(());
                }
            }
            bail!("设置窗口已经打开，请在任务栏查看");
        }
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as _,
            dwICC: ICC_WIN95_CLASSES,
        })
        .ok()
        .context("初始化 Windows 原生控件")?;
        let module = GetModuleHandleW(None)?;
        let class = w!("WechatRedReplyRustSettings");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: module.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let scale = GetDpiForSystem() as f64 / 96.;
        let face = config::wide("Microsoft YaHei UI");
        let font = CreateFontW(
            (-FONT_SIZES[0] * scale) as _,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            DEFAULT_QUALITY,
            DEFAULT_PITCH.0 as u32,
            PCWSTR(face.as_ptr()),
        );
        let mut lf = LOGFONTW::default();
        GetObjectW(
            font.into(),
            std::mem::size_of::<LOGFONTW>() as i32,
            Some(&mut lf as *mut _ as _),
        );
        lf.lfHeight = (-FONT_SIZES[1] * scale) as i32;
        lf.lfWeight = 600;
        let heading = CreateFontIndirectW(&lf);
        lf.lfHeight = (-FONT_SIZES[2] * scale) as i32;
        let section = CreateFontIndirectW(&lf);
        lf.lfHeight = (-FONT_SIZES[3] * scale) as i32;
        lf.lfWeight = 400;
        let small = CreateFontIndirectW(&lf);
        let mut app = Box::new(App {
            config,
            controls: HashMap::new(),
            pages: HashMap::new(),
            page: 101,
            root: HWND::default(),
            font,
            heading,
            section,
            small,
            paint: theme::Paint::new(),
            tls_expanded: false,
            windows: vec![],
            dirty: false,
            loading: true,
            scale,
            work: None,
            contacts: vec![],
            log_filter: 311,
            log_paused: false,
            log_text: String::new(),
        });
        let (width, height) =
            if cfg!(debug_assertions) && std::env::args().any(|s| s == "--gui-minimum") {
                (1020., 800.)
            } else {
                (1280., 900.)
            };
        let h = CreateWindowExW(
            if no_activate {
                WS_EX_NOACTIVATE
            } else {
                WINDOW_EX_STYLE(0)
            },
            class,
            w!("微信 · 红色问答"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            (width * scale) as _,
            (height * scale) as _,
            None,
            None,
            Some(module.into()),
            Some(&mut *app as *mut _ as _),
        )?;
        app.root = h;
        let caption = theme::SIDE.0;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            h,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(35),
            &caption as *const _ as _,
            4,
        );
        let text = theme::INK.0;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            h,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(36),
            &text as *const _ as _,
            4,
        );
        let border = theme::SIDE.0;
        let _ = windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
            h,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(34),
            &border as *const _ as _,
            4,
        );
        SetPropW(
            h,
            w!("WechatRedReplyConfig"),
            Some(HANDLE(identity as usize as _)),
        )?;
        app.init().context("初始化设置界面")?;
        if no_activate {
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
            SetWindowLongPtrW(
                h,
                GWL_EXSTYLE,
                GetWindowLongPtrW(h, GWL_EXSTYLE) & !(WS_EX_NOACTIVATE.0 as isize),
            );
            SetWindowPos(
                h,
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )?;
        } else {
            let _ = ShowWindow(h, SW_SHOW);
        }
        let _ = UpdateWindow(h);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(h, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let _ = DeleteObject(app.font.into());
        let _ = DeleteObject(app.heading.into());
        let _ = DeleteObject(app.section.into());
        let _ = DeleteObject(app.small.into());
        Ok(())
    }
}
unsafe extern "system" fn proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_NCCREATE {
            let create = &*(l.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(h, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut App;
        if !p.is_null() {
            let app = &mut *p;
            match msg {
                WM_ERASEBKGND => return LRESULT(1),
                WM_PAINT => {
                    let mut ps = PAINTSTRUCT::default();
                    let dc = BeginPaint(h, &mut ps);
                    theme::paint(app, dc);
                    let _ = EndPaint(h, &ps);
                    return LRESULT(0);
                }
                WM_NCCALCSIZE => {
                    if w.0 != 0 {
                        return LRESULT(0);
                    }
                }
                WM_NCHITTEST => {
                    let mut pt = POINT {
                        x: (l.0 as i32 & 0xffff) as i16 as i32,
                        y: ((l.0 as i32 >> 16) & 0xffff) as i16 as i32,
                    };
                    let mut r = RECT::default();
                    let _ = GetClientRect(h, &mut r);
                    let _ = ScreenToClient(h, &mut pt);
                    let edge = (8. * app.scale) as i32;
                    if !IsZoomed(h).as_bool() {
                        let left = pt.x < edge;
                        let right = pt.x >= r.right - edge;
                        let top = pt.y < edge;
                        let bottom = pt.y >= r.bottom - edge;
                        let hit = match (left, right, top, bottom) {
                            (true, _, true, _) => HTTOPLEFT,
                            (_, true, true, _) => HTTOPRIGHT,
                            (true, _, _, true) => HTBOTTOMLEFT,
                            (_, true, _, true) => HTBOTTOMRIGHT,
                            (true, _, _, _) => HTLEFT,
                            (_, true, _, _) => HTRIGHT,
                            (_, _, true, _) => HTTOP,
                            (_, _, _, true) => HTBOTTOM,
                            _ => HTCLIENT,
                        };
                        if hit != HTCLIENT {
                            return LRESULT(hit as isize);
                        }
                    }
                    if pt.y < (TITLEBAR_HEIGHT as f64 * app.scale) as i32 {
                        return LRESULT(HTCAPTION as isize);
                    }
                    return LRESULT(HTCLIENT as isize);
                }
                WM_MEASUREITEM => {
                    if l.0 != 0 {
                        let item = &mut *(l.0 as *mut MEASUREITEMSTRUCT);
                        if item.CtlID == 403 {
                            item.itemHeight = (28. * app.scale) as u32;
                            return LRESULT(1);
                        }
                    }
                }
                WM_PRINTCLIENT => {
                    theme::paint(app, HDC(w.0 as *mut _));
                    return LRESULT(0);
                }
                WM_DRAWITEM => {
                    if l.0 != 0 {
                        theme::draw_button(app, &*(l.0 as *const DRAWITEMSTRUCT));
                        return LRESULT(1);
                    }
                }
                WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                    let dc = HDC(w.0 as *mut _);
                    let control = HWND(l.0 as *mut _);
                    let id = GetDlgCtrlID(control);
                    let side = matches!(id, STATUS | 631);
                    SetBkMode(dc, TRANSPARENT);
                    SetTextColor(
                        dc,
                        if matches!(
                            id,
                            630 | 633
                                | 634
                                | 635
                                | 636
                                | NOTICE
                                | 612
                                | 613
                                | 623
                                | 624
                                | 627
                                | 628
                                | 402
                                | STATUS
                        ) {
                            theme::MUTED
                        } else {
                            theme::INK
                        },
                    );
                    SetBkColor(dc, if side { theme::SIDE } else { theme::WHITE });
                    return LRESULT(if side {
                        app.paint.sidebar.0 as isize
                    } else {
                        app.paint.white.0 as isize
                    });
                }
                WM_DPICHANGED => {
                    let revised = (w.0 & 65535) as f64 / 96.;
                    for kind in 0..4 {
                        let old = match kind {
                            0 => app.font,
                            1 => app.heading,
                            2 => app.section,
                            _ => app.small,
                        };
                        let mut lf = LOGFONTW::default();
                        GetObjectW(
                            old.into(),
                            std::mem::size_of::<LOGFONTW>() as i32,
                            Some(&mut lf as *mut _ as _),
                        );
                        lf.lfHeight = (-FONT_SIZES[kind as usize] * revised) as i32;
                        let new = CreateFontIndirectW(&lf);
                        for (&id, &control) in &app.controls {
                            let control_kind = if id == 600 {
                                1
                            } else if matches!(id, 620 | 632) {
                                2
                            } else if matches!(
                                id,
                                STATUS
                                    | NOTICE
                                    | 630
                                    | 633
                                    | 634
                                    | 635
                                    | 636
                                    | 402
                                    | 612
                                    | 613
                                    | 614
                                    | 623
                                    | 624
                                    | 627
                                    | 628
                            ) {
                                3
                            } else {
                                0
                            };
                            if control_kind == kind {
                                SendMessageW(
                                    control,
                                    WM_SETFONT,
                                    Some(WPARAM(new.0 as usize)),
                                    Some(LPARAM(1)),
                                );
                            }
                        }
                        match kind {
                            0 => app.font = new,
                            1 => app.heading = new,
                            2 => app.section = new,
                            _ => app.small = new,
                        }
                        let _ = DeleteObject(old.into());
                    }
                    app.scale = revised;
                    let r = &*(l.0 as *const RECT);
                    let _ = SetWindowPos(
                        h,
                        None,
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    app.layout();
                    return LRESULT(0);
                }
                WM_SIZE => {
                    if !app.controls.is_empty() {
                        app.layout();
                    }
                    return LRESULT(0);
                }
                WM_GETMINMAXINFO => {
                    let m = &mut *(l.0 as *mut MINMAXINFO);
                    m.ptMinTrackSize = POINT {
                        x: (1020. * app.scale) as _,
                        y: (800. * app.scale) as _,
                    };
                    return LRESULT(0);
                }
                WM_COMMAND => {
                    app.command((w.0 & 65535) as i32, (w.0 >> 16) as u16);
                    return LRESULT(0);
                }
                WM_TIMER => {
                    app.status();
                    return LRESULT(0);
                }
                WM_CLOSE => {
                    crate::lifecycle::note(
                        &app.config,
                        "gui_close_request",
                        "设置窗口收到 WM_CLOSE 关闭请求（消息不提供发起者身份）",
                    );
                    if app.dirty
                        && MessageBoxW(
                            Some(h),
                            w!("关闭并放弃未保存修改？监听将继续运行。"),
                            w!("关闭设置"),
                            MB_YESNO | MB_ICONQUESTION,
                        ) != IDYES
                    {
                        return LRESULT(0);
                    }
                    let _ = DestroyWindow(h);
                    return LRESULT(0);
                }
                WM_QUERYENDSESSION => {
                    crate::lifecycle::note(
                        &app.config,
                        "gui_session_query",
                        "Windows 请求结束会话",
                    );
                    return LRESULT(1);
                }
                WM_ENDSESSION => {
                    crate::lifecycle::note(
                        &app.config,
                        "gui_session_end",
                        if w.0 != 0 {
                            "Windows 正在结束会话"
                        } else {
                            "Windows 已取消结束会话"
                        },
                    );
                }
                WM_DESTROY => {
                    crate::lifecycle::note(
                        &app.config,
                        "gui_destroy",
                        "设置窗口已销毁；监听按设计继续运行",
                    );
                    PostQuitMessage(0);
                    return LRESULT(0);
                }
                _ => {}
            }
        }
        DefWindowProcW(h, msg, w, l)
    }
}
