use crate::{bridge::Job, config, native, service::Shared};
use anyhow::{Context, Result, bail};
use base64::Engine;
use image::ImageReader;
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{Cursor, Read},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, Variant::VARIANT},
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::BSTR,
};
#[derive(Clone, Debug)]
pub enum Part {
    Text(String),
    Image(Vec<u8>),
    Music(crate::music::OutgoingMusic),
    Voice(crate::voice::Note),
}
pub fn network(part: &Part) -> bool {
    matches!(part, Part::Music(_) | Part::Voice(_))
}
fn deliver_network(
    shared: &Shared,
    job: &Job,
    part: &Part,
    index: usize,
) -> Result<crate::actions::Tracked> {
    match part {
        Part::Music(music) => crate::music_backend::deliver(shared, job, music, index),
        Part::Voice(note) => crate::voice::deliver(shared, job, note, index),
        _ => bail!("不是原生网络消息"),
    }
}
pub fn confirmation(parts: &[Part]) -> &'static str {
    let n = parts.iter().filter(|p| network(p)).count();
    if n == 0 {
        "database"
    } else if n == parts.len() {
        "native_server"
    } else {
        "mixed"
    }
}
pub struct CachedImage {
    url: String,
    raw: Vec<u8>,
    expires: Instant,
}
fn unescape(s: &str) -> String {
    s.replace("&#44;", ",")
        .replace("&#91;", "[")
        .replace("&#93;", "]")
        .replace("&amp;", "&")
}
pub fn prepare(message: &Value, auto_escape: bool, shared: &Shared) -> Result<Vec<Part>> {
    let segments = if let Some(s) = message.as_str() {
        if auto_escape {
            vec![serde_json::json!({"type":"text","data":{"text":s}})]
        } else {
            parse_cq(s)
        }
    } else {
        message
            .as_array()
            .context("message must be text or segment array")?
            .clone()
    };
    // RedReply sometimes serializes private messages as one text segment that
    // still contains CQ codes (private encoder flattening). Expand those so a
    // private image is not sent as literal "[CQ:image,...]" text.
    let segments = if auto_escape {
        segments
    } else {
        let mut expanded = Vec::with_capacity(segments.len());
        for segment in segments {
            let text = (segment["type"] == "text")
                .then(|| segment["data"]["text"].as_str())
                .flatten();
            if let Some(text) = text.filter(|text| text.contains("[CQ:")) {
                expanded.extend(parse_cq(text));
            } else {
                expanded.push(segment);
            }
        }
        expanded
    };
    let mut out = vec![];
    let (mut chars, mut images) = (0, 0);
    let deadline = Instant::now() + Duration::from_secs(8);
    let c = shared.config.read().unwrap().clone();
    let mut contacts: HashMap<_, _> = c
        .scope()
        .iter()
        .map(|(n, g)| {
            (
                if *g {
                    config::group_id(n)
                } else {
                    config::contact_id(n)
                }
                .to_string(),
                n.clone(),
            )
        })
        .collect();
    for members in shared.members.lock().unwrap().values() {
        for (id, member) in members {
            if let Some(label) = member["card"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| member["nickname"].as_str())
            {
                contacts
                    .entry(id.to_string())
                    .or_insert_with(|| label.to_owned());
            }
        }
    }
    for segment in segments {
        let d = &segment["data"];
        match segment["type"].as_str().unwrap_or("") {
            "reply" => {}
            "at" => {
                if d["qq"].is_null() || d["qq"] == "" {
                    bail!("Invalid mention segment");
                }
                let id = d["qq"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| d["qq"].to_string());
                let label = if id == "all" {
                    "所有人"
                } else {
                    contacts.get(&id).map_or(id.as_str(), String::as_str)
                };
                append(&mut out, &format!("@{label} "), &mut chars)?;
            }
            "text" => append(
                &mut out,
                d["text"].as_str().context("Invalid text segment")?,
                &mut chars,
            )?,
            "image" => {
                images += 1;
                if images > 4 {
                    bail!("一次最多发送 4 张图片");
                }
                let f = d["file"].as_str().unwrap_or("");
                let f = if f.starts_with("base64://")
                    || f.starts_with("https://")
                    || f.starts_with("http://")
                {
                    f
                } else {
                    d["url"].as_str().unwrap_or(f)
                };
                let bytes = load_image(f, deadline, shared)?;
                validate_image(&bytes)?;
                out.push(Part::Image(bytes));
            }
            "record" => {
                for note in crate::voice::prepare(d)? {
                    out.push(Part::Voice(note));
                }
                let duration: u32 = out
                    .iter()
                    .filter_map(|part| {
                        if let Part::Voice(note) = part {
                            Some(note.duration_ms)
                        } else {
                            None
                        }
                    })
                    .sum();
                if duration > 600_000 {
                    bail!("一次输出的原生语音总时长超过10分钟");
                }
            }
            "music" | "json" | "xml" => {
                let request =
                    crate::music::parse_segment(&segment)?.context("该卡片不是受支持的音乐卡片")?;
                if out.iter().filter(|p| matches!(p, Part::Music(_))).count() >= 4 {
                    bail!("一次最多发送4张音乐卡片");
                }
                out.push(Part::Music(crate::music::outgoing(&request, shared)?));
            }
            _ => bail!(
                "Unsupported message segment: {}",
                segment["type"].as_str().unwrap_or("invalid")
            ),
        }
    }
    if out.is_empty()
        || !out.iter().any(|p| match p {
            Part::Image(_) => true,
            Part::Music(_) => true,
            Part::Voice(_) => true,
            Part::Text(t) => !t.trim().is_empty(),
        })
    {
        bail!("Reply is empty");
    }
    Ok(out)
}
fn append(out: &mut Vec<Part>, text: &str, count: &mut usize) -> Result<()> {
    *count += text.chars().count();
    if *count > 4000 {
        bail!("Reply text exceeds 4000 characters");
    }
    if text.is_empty() {
        return Ok(());
    }
    if let Some(Part::Text(s)) = out.last_mut() {
        s.push_str(text);
    } else {
        out.push(Part::Text(text.to_owned()));
    }
    Ok(())
}
pub(crate) fn parse_cq(text: &str) -> Vec<Value> {
    let re = regex::Regex::new(r"\[CQ:([^,\]]+)([^\]]*)\]").unwrap();
    let mut out = vec![];
    let mut cursor = 0;
    for m in re.captures_iter(text) {
        let whole = m.get(0).unwrap();
        if whole.start() > cursor {
            out.push(serde_json::json!({"type":"text","data":{"text":unescape(&text[cursor..whole.start()])}}));
        }
        let mut data = serde_json::Map::new();
        for pair in m[2].trim_start_matches(',').split(',') {
            if let Some((k, v)) = pair.split_once('=') {
                data.insert(k.to_owned(), unescape(v).into());
            }
        }
        out.push(serde_json::json!({"type":&m[1],"data":data}));
        cursor = whole.end();
    }
    if cursor < text.len() {
        out.push(serde_json::json!({"type":"text","data":{"text":unescape(&text[cursor..])}}));
    }
    out
}
fn load_image(file: &str, deadline: Instant, shared: &Shared) -> Result<Vec<u8>> {
    if let Some(data) = file.strip_prefix("base64://") {
        if data.len() > 12 * 1024 * 1024 {
            bail!("Image data too large");
        }
        let raw = base64::engine::general_purpose::STANDARD
            .decode(data)
            .context("Invalid base64 image")?;
        if raw.len() > 8 * 1024 * 1024 {
            bail!("Image exceeds 8 MiB");
        }
        return Ok(raw);
    }
    let url = url::Url::parse(file)
        .map_err(|_| anyhow::anyhow!("Image must be base64 or HTTP(S) URL"))?;
    if !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("Image URL must be HTTP(S) without credentials");
    }
    {
        let mut cache = shared.images.lock().unwrap();
        cache.retain(|c| c.expires > Instant::now());
        if let Some(c) = cache.iter().find(|c| c.url == file) {
            return Ok(c.raw.clone());
        }
    }
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::limited(3))
        .connect_timeout(Duration::from_secs(3))
        .timeout(deadline.saturating_duration_since(Instant::now()))
        .build()?;
    let response = client
        .get(file)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|_| anyhow::anyhow!("Image HTTP download failed"))?;
    let policy = response
        .headers()
        .get("cache-control")
        .and_then(|x| x.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut raw = vec![];
    response
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut raw)
        .map_err(|_| anyhow::anyhow!("Image HTTP read failed"))?;
    if raw.len() > 8 * 1024 * 1024 {
        bail!("Image exceeds 8 MiB");
    }
    if !["no-store", "no-cache", "private"]
        .iter()
        .any(|v| policy.contains(v))
    {
        let max = policy
            .split(',')
            .find_map(|v| {
                v.trim()
                    .strip_prefix("max-age=")
                    .and_then(|n| n.trim_matches('"').parse::<u64>().ok())
            })
            .unwrap_or(0)
            .min(600);
        if max > 0 {
            let mut cache = shared.images.lock().unwrap();
            while cache.len() >= 64
                || cache.iter().map(|v| v.raw.len()).sum::<usize>() + raw.len() > 32 * 1024 * 1024
            {
                if cache.pop_front().is_none() {
                    break;
                }
            }
            cache.push_back(CachedImage {
                url: file.to_owned(),
                raw: raw.clone(),
                expires: Instant::now() + Duration::from_secs(max),
            });
        }
    }
    Ok(raw)
}
pub fn validate_image(raw: &[u8]) -> Result<()> {
    let reader = ImageReader::new(Cursor::new(raw)).with_guessed_format()?;
    let (w, h) = reader.into_dimensions()?;
    if w == 0 || h == 0 || w as u64 * h as u64 > 8_000_000 {
        bail!("Image exceeds 8 million pixels");
    }
    Ok(())
}

pub struct Automation {
    pub(crate) automation: IUIAutomation,
}
impl Automation {
    fn accessible_children(&self, a: &IAccessible) -> Vec<IAccessible> {
        unsafe {
            use windows::core::Interface;
            let count = a.accChildCount().unwrap_or(0).clamp(0, 2000);
            let mut variants: Vec<_> = (0..count).map(|_| VARIANT::default()).collect();
            let mut got = 0;
            let mut result = vec![];
            if AccessibleChildren(a, 0, &mut variants, &mut got).is_ok() {
                for v in variants.iter().take(got as usize) {
                    let raw = &v.Anonymous.Anonymous;
                    let dispatch = if raw.vt.0 == 9 {
                        (&*raw.Anonymous.pdispVal).clone()
                    } else {
                        a.get_accChild(v).ok()
                    };
                    if let Some(d) = dispatch {
                        if let Ok(child) = d.cast::<IAccessible>() {
                            result.push(child);
                        }
                    }
                }
            }
            result
        }
    }
    pub(crate) fn root(&self, h: HWND) -> Result<IUIAutomationElement> {
        unsafe {
            use windows::core::Interface;
            let mut best = self.automation.ElementFromHandle(h)?;
            let mut score = best
                .FindAll(
                    TreeScope_Descendants,
                    &self.automation.CreateTrueCondition()?,
                )
                .and_then(|nodes| nodes.Length())
                .unwrap_or(0) as usize;
            let mut raw = std::ptr::null_mut();
            if score > 5 {
                return Ok(best);
            }
            if AccessibleObjectFromWindow(h, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut raw)
                .is_ok()
            {
                let accessible = IAccessible::from_raw(raw);
                let mut queue = std::collections::VecDeque::from([(accessible, 0)]);
                while let Some((accessible, depth)) = queue.pop_front() {
                    if let Ok(element) = self.automation.ElementFromIAccessible(&accessible, 0) {
                        let count = self.descendants(&element)?.len();
                        if count > score {
                            best = element;
                            score = count;
                        }
                    }
                    if depth < 3 {
                        for child in self.accessible_children(&accessible) {
                            queue.push_back((child, depth + 1));
                        }
                    }
                }
            }
            Ok(best)
        }
    }
    fn msaa_wake(&self, h: HWND) -> Result<i32> {
        unsafe {
            use windows::core::Interface;
            let mut raw = std::ptr::null_mut();
            AccessibleObjectFromWindow(h, OBJID_CLIENT.0 as u32, &IAccessible::IID, &mut raw)?;
            let accessible = IAccessible::from_raw(raw);
            let mut queue = std::collections::VecDeque::from([(accessible, 0)]);
            let mut count = 0;
            while let Some((a, depth)) = queue.pop_front() {
                count += 1;
                if depth < 50 && count < 2000 {
                    for child in self.accessible_children(&a) {
                        queue.push_back((child, depth + 1));
                    }
                }
            }
            Ok(count)
        }
    }
    pub fn inspect(&self, c: &crate::config::Config) -> Result<serde_json::Value> {
        unsafe {
            let h = native::validate_window(&c.binding()?)?;
            native::offscreen(h)?;
            let msaa = self.msaa_wake(h).unwrap_or(-1);
            let root = self.root(h)?;
            let nodes = self.descendants(&root)?;
            let mut controls = vec![];
            for node in &nodes {
                let role = node.CurrentControlType()?;
                if role == UIA_ButtonControlTypeId || role == UIA_EditControlTypeId {
                    controls.push(serde_json::json!({"class":node.CurrentClassName()?.to_string(),"name":node.CurrentName()?.to_string(),"aid":node.CurrentAutomationId()?.to_string()}));
                }
            }
            let mut rows = vec![];
            for row in self.rows(h)? {
                rows.push(serde_json::json!({"class":row.CurrentClassName()?.to_string(),"name_length":row.CurrentName()?.to_string().len(),"name_image":row.CurrentName()?.to_string().contains("图片"),"prefix":row.CurrentName()?.to_string().chars().take(20).collect::<String>(),"runtime_id":self.runtime_id(&row)?,"children":self.descendants(&row)?.iter().map(|child| serde_json::json!({"class":child.CurrentClassName().map(|s|s.to_string()).unwrap_or_default(),"role":child.CurrentControlType().map(|r|r.0).unwrap_or(0)})).collect::<Vec<_>>()}));
            }
            Ok(
                serde_json::json!({"msaa_children":msaa,"root":root.CurrentClassName()?.to_string(),"raw_nodes":nodes.len(),"title_available":self.title(h).is_ok(),"enabled":windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(h).as_bool(),"foreground":GetForegroundWindow().0 as isize,"popups":native::thread_windows(h).len(),"rows":rows,"controls":controls}),
            )
        }
    }
    pub fn new8() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            Ok(Self {
                automation: CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)?,
            })
        }
    }
    pub(crate) fn descendants(
        &self,
        root: &IUIAutomationElement,
    ) -> Result<Vec<IUIAutomationElement>> {
        unsafe {
            let walker = self.automation.RawViewWalker()?;
            let mut queue = std::collections::VecDeque::from([(root.clone(), 0)]);
            let mut result = vec![];
            while let Some((parent, depth)) = queue.pop_front() {
                if depth >= 50 || result.len() > 4000 {
                    continue;
                }
                let mut child = walker.GetFirstChildElement(&parent).ok();
                while let Some(node) = child {
                    child = walker.GetNextSiblingElement(&node).ok();
                    queue.push_back((node.clone(), depth + 1));
                    result.push(node);
                }
            }
            Ok(result)
        }
    }
    pub fn menu_probe(
        &self,
        c: &crate::config::Config,
        data: &std::path::Path,
    ) -> Result<serde_json::Value> {
        unsafe {
            crate::popup::probe(data)?;
            let h = native::validate_window(&c.binding()?)?;
            native::offscreen(h)?;
            let _guard = crate::popup::Guard::install(h, data)?;
            let root = self.root(h)?;
            let list = self.find(&root, UIA_AutomationIdPropertyId, "chat_message_list")?;
            let all = list.FindAll(
                TreeScope_Descendants,
                &self.automation.CreateTrueCondition()?,
            )?;
            let list_rect = list.CurrentBoundingRectangle()?;
            let mut selected = None;
            for i in 0..all.Length()? {
                let row = all.GetElement(i)?;
                if row
                    .CurrentAutomationId()?
                    .to_string()
                    .ends_with("chat_bubble_item_view")
                {
                    let r = row.CurrentBoundingRectangle()?;
                    if r.right > r.left && r.bottom > list_rect.top && r.top < list_rect.bottom {
                        selected = Some((row, r));
                    }
                }
            }
            let (_row, rect) = selected.context("当前会话没有可检查的消息气泡")?;
            let mut origin = POINT::default();
            windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut origin).ok()?;
            let y =
                (rect.top.max(list_rect.top) + rect.bottom.min(list_rect.bottom)) / 2 - origin.y;
            let picture = native::capture(h)?;
            picture.save(data.join("native-menu-probe.png"))?;
            let mut window_rect = RECT::default();
            GetWindowRect(h, &mut window_rect)?;
            let mut count = 0u64;
            let mut sum = 0u64;
            for py in (rect.top.max(list_rect.top) - window_rect.top).max(0)
                ..(rect.bottom.min(list_rect.bottom) - window_rect.top).min(picture.height() as i32)
            {
                for px in (rect.left - window_rect.left).max(0)
                    ..(rect.right - window_rect.left).min(picture.width() as i32)
                {
                    let p = picture.get_pixel(px as _, py as _);
                    if p[1] as i32 > p[0] as i32 + 18 && p[1] as i32 > p[2] as i32 + 18 && p[1] > 45
                    {
                        count += 1;
                        sum += px as u64;
                    }
                }
            }
            let x = if count > 20 {
                (sum / count) as i32 + window_rect.left - origin.x
            } else {
                rect.left + 100 - origin.x
            };
            for (msg, key) in [(WM_MOUSEMOVE, 0), (WM_RBUTTONDOWN, 2), (WM_RBUTTONUP, 0)] {
                PostMessageW(
                    Some(h),
                    msg,
                    WPARAM(key),
                    LPARAM(((y << 16) | (x & 0xffff)) as isize),
                )?;
                std::thread::sleep(Duration::from_millis(40));
            }
            PostMessageW(
                Some(h),
                WM_CONTEXTMENU,
                WPARAM(h.0 as usize),
                LPARAM((((origin.y + y) & 0xffff) << 16 | ((origin.x + x) & 0xffff)) as isize),
            )?;
            std::thread::sleep(Duration::from_millis(350));
            let mut elements = self.descendants(&root)?;
            let popups = native::thread_windows(h);
            for popup in &popups {
                if let Ok(root) = self.root(*popup) {
                    elements.push(root.clone());
                    elements.extend(self.descendants(&root)?);
                }
            }
            let mut actions = vec![];
            for element in elements {
                let class = element.CurrentClassName()?.to_string();
                let name = element.CurrentName()?.to_string();
                if class.contains("Menu")
                    || [
                        "撤回",
                        "多选",
                        "转发",
                        "转发...",
                        "合并转发",
                        "复制",
                        "引用",
                        "删除",
                    ]
                    .contains(&name.as_str())
                {
                    let rect = element.CurrentBoundingRectangle()?;
                    actions.push(serde_json::json!({"name":name,"class":class,"offscreen":rect.right<GetSystemMetrics(SM_XVIRTUALSCREEN)&&rect.bottom<GetSystemMetrics(SM_YVIRTUALSCREEN)}));
                }
            }
            // Escape without an open menu hides WeChat's main window. Do not
            // synthesize it as a generic cancellation action.
            for popup in popups {
                let _ = PostMessageW(Some(popup), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            Ok(
                serde_json::json!({"native_menu_actions":actions,"target_kept_offscreen":true,"client_point":[x,y],"row_class":_row.CurrentClassName()?.to_string(),"row_bounds":[rect.left-origin.x,rect.top-origin.y,rect.right-origin.x,rect.bottom-origin.y],"list_bounds":[list_rect.left-origin.x,list_rect.top-origin.y,list_rect.right-origin.x,list_rect.bottom-origin.y]}),
            )
        }
    }
    pub fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            Ok(Self {
                automation: CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
                    .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))?,
            })
        }
    }
    pub fn legacy() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            Ok(Self {
                automation: CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?,
            })
        }
    }
    pub(crate) fn find(
        &self,
        root: &IUIAutomationElement,
        property: UIA_PROPERTY_ID,
        value: &str,
    ) -> Result<IUIAutomationElement> {
        unsafe {
            let condition = self
                .automation
                .CreatePropertyCondition(property, &VARIANT::from(value))?;
            if let Ok(element) = root.FindFirst(TreeScope_Descendants, &condition) {
                return Ok(element);
            }
            for element in self.descendants(root)? {
                let actual = if property == UIA_AutomationIdPropertyId {
                    element.CurrentAutomationId()?.to_string()
                } else {
                    element.CurrentName()?.to_string()
                };
                if actual == value {
                    return Ok(element);
                }
            }
            bail!("微信控件不可用：{value}")
        }
    }
    pub fn title(&self, h: HWND) -> Result<String> {
        unsafe {
            let root = self.root(h)?;
            Ok(self.find(&root,UIA_AutomationIdPropertyId,"content_view.top_content_view.title_h_view.left_v_view.left_content_v_view.left_ui_.big_title_line_h_view.current_chat_name_label")?.CurrentName()?.to_string())
        }
    }
    pub(crate) fn select(&self, h: HWND, name: &str, group: bool) -> Result<()> {
        if self.title(h).is_err() {
            let _ = self.msaa_wake(h);
            let mut recovered = false;
            for label in ["微信", "聊天", "Chats", "WeChat"] {
                if crate::friends::tab(self, h, label).is_ok() {
                    recovered = true;
                    break;
                }
            }
            anyhow::ensure!(
                recovered,
                "当前不在聊天页，未找到唯一的微信聊天入口；未激活其他窗口"
            );
        }
        if title_matches(&self.title(h).unwrap_or_default(), name, group) {
            return Ok(());
        }
        unsafe {
            let root = self.root(h)?;
            let row = match self.find(
                &root,
                UIA_AutomationIdPropertyId,
                &format!("session_item_{name}"),
            ) {
                Ok(row) => row,
                Err(_) => return self.open_search(h, name, group),
            };
            let rect = row.CurrentBoundingRectangle()?;
            let mut origin = POINT::default();
            windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut origin).ok()?;
            let x = (rect.left + rect.right) / 2 - origin.x;
            let y = (rect.top + rect.bottom) / 2 - origin.y;
            if !(0..2000).contains(&x) || !(0..2000).contains(&y) {
                bail!("会话选择坐标无效");
            }
            for (msg, key) in [(WM_MOUSEMOVE, 0), (WM_LBUTTONDOWN, 1), (WM_LBUTTONUP, 0)] {
                PostMessageW(Some(h), msg, WPARAM(key), LPARAM(((y << 16) | x) as isize))?;
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        let deadline = Instant::now() + Duration::from_millis(1200);
        while Instant::now() < deadline {
            if title_matches(&self.title(h).unwrap_or_default(), name, group) {
                std::thread::sleep(Duration::from_millis(250));
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("未能选中精确会话；未发送")
    }
    pub fn send(&self, shared: &Shared, job: &Job) -> Result<()> {
        let c = shared.config.read().unwrap().clone();
        let binding = c.binding()?;
        if let crate::actions::Operation::Recall(id) = &job.operation {
            return self.recall(shared, *id);
        }
        native::validate_chat_window(&binding)?;
        let _activity = shared.begin_native();
        if matches!(&job.operation, crate::actions::Operation::Send)
            && job.parts.iter().all(network)
        {
            let current = crate::database::account_root(&c, &native::Process::bound(&binding)?)?;
            if shared
                .bound_root
                .read()
                .unwrap()
                .as_ref()
                .is_none_or(|r| !crate::database::same_root(r, &current))
            {
                bail!("微信登录账号已改变，未发送原生媒体")
            }
            for (index, part) in job.parts.iter().enumerate() {
                let tracked = deliver_network(shared, job, part, index)?;
                crate::actions::append_sent(shared, job, &binding, tracked)?;
            }
            return Ok(());
        }
        let h = native::validate_window(&binding)?;
        native::offscreen(h)?;
        let _operation_guard = crate::popup::Guard::install(h, &shared.data)?;
        if !unsafe { windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled(h) }.as_bool() {
            let diagnostic = crate::diagnostics::collect(&c);
            let _ = config::atomic(&shared.data.join("ui-readiness.json"), &diagnostic);
            return Err(crate::ui_readiness::not_submitted(
                "微信主窗口被对话框阻塞，尚未发送；请检查微信提示（例如存储空间不足），处理后重试。控件诊断已保存到 data/ui-readiness.json",
            ));
        }
        let root = self.root(h)?;
        if self
            .find(&root, UIA_AutomationIdPropertyId, "chat_input_field")
            .is_err()
        {
            let _guard = crate::popup::Guard::install(h, &shared.data)?;
            self.cancel_actions(h, &shared.data);
        }
        let current = crate::database::account_root(&c, &native::Process::bound(&binding)?)?;
        if shared
            .bound_root
            .read()
            .unwrap()
            .as_ref()
            .is_none_or(|r| !crate::database::same_root(r, &current))
        {
            bail!("微信登录账号已改变，未执行发送或撤回");
        }
        match &job.operation {
            crate::actions::Operation::Recall(id) => {
                let result = self.recall(shared, *id);
                if result.is_err() {
                    self.cancel_actions(h, &shared.data);
                }
                return result;
            }
            crate::actions::Operation::Forward(nodes) => {
                let result = self.forward(shared, job, nodes);
                if result.is_err() {
                    self.cancel_actions(h, &shared.data);
                }
                return result;
            }
            crate::actions::Operation::Send => {}
        }
        let _guard = crate::popup::Guard::install(h, &shared.data)?;
        let prepared = crate::ui_readiness::prepare(
            self.title(h),
            || {
                // Initialize the accessibility provider by querying it, without
                // activating the window or changing the user's global input.
                let _ = self.msaa_wake(h);
                let until = Instant::now() + Duration::from_secs(2);
                loop {
                    let root = self.root(h)?;
                    if self.descendants(&root)?.len() > 5 {
                        break;
                    }
                    if Instant::now() >= until {
                        bail!("微信无障碍控件仍未加载");
                    }
                    if Instant::now() >= job.deadline {
                        bail!("任务已过期，未执行会话选择");
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                self.select(h, &job.name, job.group)
            },
            || {
                anyhow::ensure!(
                    title_matches(&self.title(h)?, &job.name, job.group),
                    "目标会话核对失败，未发送"
                );
                let root = self.root(h)?;
                self.find(&root, UIA_AutomationIdPropertyId, "chat_input_field")?;
                Ok(())
            },
        );
        if let Err(error) = prepared {
            let diagnostic = crate::diagnostics::collect(&c);
            let _ = config::atomic(&shared.data.join("ui-readiness.json"), &diagnostic);
            return Err(crate::ui_readiness::not_submitted(format!(
                "微信目标界面未就绪，未发送：{error:#}。诊断已保存到 data/ui-readiness.json；请双击 diagnose.cmd 核对窗口状态和客户端版本"
            )));
        }
        for (index, part) in job.parts.iter().enumerate() {
            if Instant::now() >= job.deadline
                || shared.stop.load(Ordering::SeqCst)
                || !shared.owner(job.owner)
                || !shared.config.read().unwrap().allows(&job.name, job.group)
            {
                bail!("发送动作过期、连接断开或目标已移出白名单");
            }
            let process = native::Process::bound(&binding)?;
            let account = crate::database::account_root(&shared.config.read().unwrap(), &process)?;
            let bound = shared.bound_root.read().unwrap();
            if bound
                .as_ref()
                .is_none_or(|r| !crate::database::same_root(r, &account))
            {
                bail!("微信账号已改变；未发送");
            }
            drop(bound);
            if network(part) {
                let tracked = deliver_network(shared, job, part, index)?;
                crate::actions::append_sent(shared, job, &binding, tracked)?;
                continue;
            }
            let checkpoint = shared.checkpoint();
            let sent_at = config::now();
            if let Err(error) = self.send_part(shared, h, &job.name, job.group, part, job) {
                self.cancel_actions(h, &shared.data);
                return Err(error);
            }
            let tracked = self.track(
                shared,
                h,
                &job.name,
                part,
                checkpoint,
                sent_at,
                Some(job.id),
            )?;
            crate::actions::append_sent(shared, job, &binding, tracked)?;
        }
        Ok(())
    }
    pub(crate) fn send_part(
        &self,
        shared: &Shared,
        h: HWND,
        target: &str,
        group: bool,
        part: &Part,
        job: &Job,
    ) -> Result<()> {
        if network(part) {
            bail!("音乐/语音需要独立原生网络回执，未走界面中转");
        }
        unsafe {
            self.verify(h, target, group)?;
            let root = self.root(h)?;
            let edit = self.find(&root, UIA_AutomationIdPropertyId, "chat_input_field")?;
            let value: IUIAutomationValuePattern = edit.GetCurrentPatternAs(UIA_ValuePatternId)?;
            let draft = value.CurrentValue()?.to_string();
            if draft.contains('\u{fffc}') {
                bail!("会话存在附件草稿，无法安全暂存；未发送");
            }
            let backup = shared.data.join("drafts").join(format!(
                "{}.json",
                config::contact_id(&format!("{}:{target}", h.0 as isize))
            ));
            if !draft.is_empty() {
                config::atomic(
                    &backup,
                    &serde_json::json!({"target":target,"text":draft,"saved_at":config::now()}),
                )?;
            }
            let before = shared.checkpoint();
            let mut invoked = false;
            let mut inserted = false;
            let action = (|| -> Result<()> {
                match part {
                    Part::Text(text) => {
                        value.SetValue(&BSTR::from(text))?;
                        inserted = true;
                        if value.CurrentValue()?.to_string() != *text {
                            bail!("输入框内容未完整写入；未发送");
                        }
                    }
                    Part::Image(raw) => {
                        value.SetValue(&BSTR::new())?;
                        self.attach_image(shared, h, raw)?;
                        inserted = true;
                    }
                    Part::Music(_) | Part::Voice(_) => {
                        unreachable!("media uses native network delivery")
                    }
                }
                self.verify(h, target, group)?;
                let mut button = None;
                for name in ["发送", "发送(S)", "Send"] {
                    if let Ok(found) = self.find(&root, UIA_NamePropertyId, name) {
                        button = Some(found);
                        break;
                    }
                }
                let button = button.context("发送按钮不可用")?;
                let limit = Instant::now() + Duration::from_secs(1);
                while !button.CurrentIsEnabled()?.as_bool() && Instant::now() < limit {
                    std::thread::sleep(Duration::from_millis(30));
                }
                if !button.CurrentIsEnabled()?.as_bool() {
                    bail!("发送按钮未启用；未提交");
                }
                self.verify(h, target, group)?;
                job.check_submission(shared)?;
                let submitted_at = config::now();
                if let Part::Image(raw) = part {
                    crate::submitted_image::register(shared, job, target, raw)?;
                }
                invoked = true;
                self.click_native(&button, h)?;
                let (kind, text) = match part {
                    Part::Text(t) => (1, Some(t.as_str())),
                    Part::Image(_) => (3, None),
                    Part::Music(_) | Part::Voice(_) => {
                        unreachable!("media uses native network delivery")
                    }
                };
                let confirmed = if let Part::Image(raw) = part {
                    let hash = crate::attachments::image_fingerprint(raw)?;
                    crate::submitted_image::wait(
                        shared,
                        target,
                        &hash,
                        before,
                        submitted_at,
                        (target == job.name
                            && matches!(&job.operation, crate::actions::Operation::Send))
                        .then_some(job.id),
                        (Instant::now() + Duration::from_secs(20)).min(job.deadline),
                    )
                } else {
                    shared.confirm(target, kind, text, before)
                };
                if !confirmed {
                    bail!("未观察到新的微信服务器消息编号；发送可能已尝试，不会自动重发");
                }
                shared.log(&format!(
                    "微信数据库确认发送：{}",
                    if kind == 1 { "text" } else { "image" }
                ));
                Ok(())
            })();
            if inserted && !invoked {
                if let Part::Text(text) = part {
                    if self.verify(h, target, group).is_ok()
                        && value.CurrentValue().is_ok_and(|s| s.to_string() == *text)
                    {
                        let _ = value.SetValue(&BSTR::new());
                    }
                }
            }
            if !draft.is_empty() && self.verify(h, target, group).is_ok() {
                if value.CurrentValue().is_ok_and(|s| s.is_empty()) {
                    if value.SetValue(&BSTR::from(&draft)).is_ok()
                        && value.CurrentValue().is_ok_and(|s| s.to_string() == draft)
                    {
                        let _ = std::fs::remove_file(backup);
                    }
                }
            }
            action
        }
    }
    fn verify(&self, h: HWND, name: &str, group: bool) -> Result<()> {
        if !title_matches(&self.title(h)?, name, group) {
            bail!("当前会话与目标不符；未发送");
        }
        Ok(())
    }
}
pub fn title_matches(title: &str, name: &str, group: bool) -> bool {
    title == name
        || (group
            && regex::Regex::new(&format!(r"^{}\s*[（(]\d+[)）]$", regex::escape(name)))
                .unwrap()
                .is_match(title))
}
pub fn run(shared: Arc<Shared>, rx: crossbeam_channel::Receiver<Job>) {
    let automation = match Automation::new() {
        Ok(a) => a,
        Err(e) => {
            shared.set_error(&format!("微信发送控件初始化失败：{e}"));
            return;
        }
    };
    let mut friend_scan = Instant::now() - Duration::from_secs(10);
    let mut friend_interval = Duration::from_secs(10);
    let mut idle_guard: Option<(isize, crate::popup::Guard)> = None;
    while !shared.stop.load(Ordering::Relaxed) {
        if !shared.ready.load(Ordering::SeqCst) {
            // Do not leave a thread-wide focus/position hook attached after
            // logout, modal blocking or a destroyed/replaced chat window.
            idle_guard = None;
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let binding = shared.config.read().unwrap().binding();
        if let Ok(binding) = binding {
            if let Err(e) = native::validate_chat_window(&binding) {
                shared.ready.store(false, Ordering::SeqCst);
                shared.set_error(&format!("微信不可用：{e}"));
                idle_guard = None;
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            if idle_guard.as_ref().is_none_or(|(h, _)| *h != binding.hwnd) {
                idle_guard = None;
                match native::validate_window(&binding)
                    .and_then(|h| crate::popup::Guard::install(h, &shared.data))
                {
                    Ok(guard) => idle_guard = Some((binding.hwnd, guard)),
                    Err(e) => {
                        shared.set_error(&format!("离屏输入保护未就绪：{e}"));
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        match crate::outbox::process_recall(&shared, &automation) {
            Ok(true) => continue,
            Ok(false) => (),
            Err(e) => shared.log(&format!("整批撤回队列错误：{e}")),
        }
        let job = if let Some(id) = crate::actions::due_recall(&shared) {
            let record = shared.ledger.lock().unwrap().sent.get(&id).cloned();
            let Some(record) = record else { continue };
            let (tx, _) = std::sync::mpsc::sync_channel(1);
            Job {
                name: record.name,
                group: record.group,
                owner: 0,
                id,
                parts: vec![],
                operation: crate::actions::Operation::Recall(id),
                deadline: Instant::now() + Duration::from_secs(5),
                state: Arc::new(std::sync::atomic::AtomicU8::new(0)),
                reply: tx,
            }
        } else {
            let durable = crate::outbox::enabled(&shared.config.read().unwrap().clone());
            if durable {
                match crate::outbox::process_one(&shared, &automation) {
                    Ok(true) => continue,
                    Ok(false) => (),
                    Err(e) => shared.log(&format!("持久化队列处理错误：{e}")),
                }
            }
            if friend_scan.elapsed() >= friend_interval {
                friend_scan = Instant::now();
                match crate::friends::poll(&shared, &automation) {
                    Ok(n) if n > 0 => friend_interval = Duration::from_secs(10),
                    Ok(_) => friend_interval = (friend_interval * 2).min(Duration::from_secs(60)),
                    Err(e) => {
                        friend_interval = (friend_interval * 2).min(Duration::from_secs(60));
                        shared.log(&format!("好友申请处理：{e}"));
                    }
                }
            }
            let Ok(job) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            job
        };
        if Instant::now() >= job.deadline
            || (job.owner != 0 && !shared.owner(job.owner))
            || !shared.ready.load(Ordering::SeqCst)
            || job
                .state
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            let _ = job.reply.send(Err("发送动作已取消或微信未就绪".into()));
            continue;
        }
        let started = Instant::now();
        let content = job
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(text) => serde_json::json!({"type":"text","text":text}),
                Part::Image(bytes) => serde_json::json!({"type":"image","bytes":bytes.len()}),
                Part::Music(card) => serde_json::json!({"type":"music","title":card.card.title,"artist":card.card.artist,"delivery":"native_type3_cgi"}),
                Part::Voice(note) => serde_json::json!({"type":"record","duration_ms":note.duration_ms,"delivery":"native_type34_cgi127"}),
            })
            .collect::<Vec<_>>();
        shared.event("outgoing",&serde_json::json!({"session":job.name,"group":job.group,"message_id":job.id,"content":content,"operation":match &job.operation {crate::actions::Operation::Send=>"发送",crate::actions::Operation::Forward(_)=>"合并转发",crate::actions::Operation::Recall(_)=>"撤回"}}));
        let result = automation.send(&shared, &job).map_err(|e| format!("{e}"));
        shared.event("delivery",&serde_json::json!({"session":job.name,"group":job.group,"message_id":job.id,"success":result.is_ok(),"error":result.as_ref().err(),"confirmation":if matches!(job.operation,crate::actions::Operation::Recall(_)){"wechat"}else{confirmation(&job.parts)},"elapsed_ms":started.elapsed().as_millis()}));
        shared
            .last_send_us
            .store(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        if let Err(ref e) = result {
            shared.log(&format!("发送失败：{e}"));
        }
        job.state.store(3, Ordering::SeqCst);
        let _ = job.reply.send(result);
        if job.owner == 0 {
            shared.ledger.lock().unwrap().scheduled.remove(&job.id);
            let _ = crate::actions::save(&shared);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_title_does_not_match_private_or_prefix() {
        assert!(title_matches("群(12)", "群", true));
        assert!(!title_matches("群2", "群", true));
        assert!(!title_matches("测试好友(12)", "测试好友", false));
    }
    #[test]
    fn cq_keeps_case_and_escaped_comma() {
        let parts = parse_cq("ccc1[CQ:at,qq=123]A&#44;B");
        assert_eq!(parts[0]["data"]["text"], "ccc1");
        assert_eq!(parts[2]["data"]["text"], "A,B");
    }
}
