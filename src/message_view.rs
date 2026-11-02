//! Operation-scoped DB-to-viewport alignment; UIA RuntimeIds are not message IDs.
use crate::{database::Message, delivery::Automation, service::Shared};
use anyhow::{Result, bail};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, time::Duration};
use windows::Win32::{
    Foundation::*,
    UI::{Accessibility::*, WindowsAndMessaging::*},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Label {
    Text([u8; 32]),
    Image,
    Forward,
    Music([u8; 32]),
}
#[derive(Clone, PartialEq, Eq)]
pub struct Item {
    pub key: String,
    pub labels: Vec<Label>,
    pub own: bool,
    order: (i64, String, i64),
}
#[derive(Default)]
pub struct Timeline {
    pub revision: u64,
    pub rows: Vec<Item>,
}
pub type Timelines = HashMap<String, Timeline>;
/// Short-lived overlap anchors, owned by one forward operation only.
#[derive(Default)]
pub(crate) struct ForwardView {
    revision: u64,
    name: String,
    hwnd: isize,
    rows: Vec<(usize, Label, Vec<i32>)>,
}
fn digest(s: &str) -> [u8; 32] {
    Sha256::digest(
        s.chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .as_bytes(),
    )
    .into()
}
fn outer_type(xml: &str) -> Option<i64> {
    use quick_xml::{Reader, events::Event};
    let mut r = Reader::from_str(xml);
    let mut path = Vec::new();
    for _ in 0..10000 {
        match r.read_event().ok()? {
            Event::Start(e) => {
                path.push(std::str::from_utf8(e.name().as_ref()).ok()?.to_owned());
                if path.len() > 32 {
                    return None;
                }
                if path.join("/") == "msg/appmsg/type" {
                    let text = r.read_text(e.name()).ok()?;
                    return text
                        .trim()
                        .strip_prefix("<![CDATA[")
                        .and_then(|s| s.strip_suffix("]]>"))
                        .unwrap_or(text.trim())
                        .parse()
                        .ok();
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => return None,
            _ => {}
        }
    }
    None
}
pub fn remember(shared: &Shared, name: &str, row: &Message, own: bool, nick: &str) {
    let mut labels = match row.kind {
        1 => vec![Label::Text(digest(&row.content))],
        3 => vec![Label::Image],
        49 => match outer_type(&row.content) {
            Some(19) => vec![Label::Forward],
            Some(3) => crate::music::parse_native(&row.content)
                .ok()
                .flatten()
                .map(|c| vec![Label::Music(digest(&format!("{}{}", c.title, c.artist)))])
                .unwrap_or_default(),
            _ => vec![],
        },
        _ => vec![],
    };
    // WeChat adds the sender name to text item accessibility names in multi-select,
    // including our own messages. Keep both exact forms; never strip an arbitrary prefix.
    if row.kind == 1 && !nick.is_empty() {
        for sep in ["", ":", "："] {
            labels.push(Label::Text(digest(&format!("{nick}{sep}{}", row.content))));
        }
    }
    let mut timelines = shared.timeline.lock().unwrap();
    let t = timelines.entry(name.to_owned()).or_default();
    let item = Item {
        key: row.key(),
        labels,
        own,
        order: (row.sort_seq, row.shard.clone(), row.local_id),
    };
    if let Some(old) = t.rows.iter_mut().find(|x| x.key == item.key) {
        if *old == item {
            return;
        }
        *old = item;
    } else {
        t.rows.push(item);
    }
    t.rows.sort_by(|a, b| a.order.cmp(&b.order));
    if t.rows.len() > 2048 {
        let n = t.rows.len() - 2048;
        t.rows.drain(..n);
    }
    t.revision = t.revision.wrapping_add(1);
}
#[derive(Clone)]
struct Visible {
    label: Label,
    runtime: Vec<i32>,
    rect: [i32; 4],
    node: IUIAutomationElement,
}
fn ui_label(node: &IUIAutomationElement) -> Result<Option<Label>> {
    unsafe {
        let class = node.CurrentClassName()?.to_string();
        let name = node.CurrentName()?.to_string();
        if class.contains("Revoke") || name.contains("撤回了一条消息") {
            return Ok(None);
        }
        Ok(if class.contains("ChatText") {
            if name.contains("\n引用 ") {
                None
            } else {
                Some(Label::Text(digest(&name)))
            }
        } else if class.contains("Image")
            || class.contains("Pic")
            || name == "[图片]"
            || name == "图片"
        {
            Some(Label::Image)
        } else if class.contains("ChatRecord") || name.contains("的聊天记录") || name == "聊天记录"
        {
            Some(Label::Forward)
        } else if let Some(body) = name
            .strip_prefix("QQ音乐")
            .or_else(|| name.strip_prefix("网易云音乐"))
        {
            Some(Label::Music(digest(body)))
        } else {
            None
        })
    }
}
fn compatible(db: &Item, ui: &Label) -> bool {
    db.labels.contains(ui)
}
fn tail_start(db: &[Item], ui: &[Label]) -> Option<usize> {
    if ui.is_empty() || ui.len() > db.len() {
        return None;
    }
    let start = db.len() - ui.len();
    db[start..]
        .iter()
        .zip(ui)
        .all(|(d, u)| compatible(d, u))
        .then_some(start)
}
fn anchored_start(
    db: &[Item],
    ui: &[Label],
    anchors: &[(usize, usize)],
    max_end: usize,
) -> Option<usize> {
    if ui.is_empty() || ui.len() > db.len() {
        return None;
    }
    let candidates: Vec<_> = (0..=db.len() - ui.len())
        .filter(|&s| {
            s + ui.len() <= max_end
                && db[s..s + ui.len()]
                    .iter()
                    .zip(ui)
                    .all(|(d, u)| compatible(d, u))
                && anchors.iter().all(|(u, d)| s + u == *d)
        })
        .collect();
    (candidates.len() == 1).then(|| candidates[0])
}
impl Automation {
    fn visible_messages(&self, h: HWND) -> Result<Vec<Visible>> {
        let list = unsafe {
            self.find(
                &self.root(h)?,
                UIA_AutomationIdPropertyId,
                "chat_message_list",
            )?
            .CurrentBoundingRectangle()?
        };
        let mut result = vec![];
        for node in self.rows(h)? {
            let r = unsafe { node.CurrentBoundingRectangle()? };
            if r.bottom.min(list.bottom) - r.top.max(list.top) < 2 {
                continue;
            }
            if let Some(label) = ui_label(&node)? {
                result.push(Visible {
                    label,
                    runtime: self.runtime_id(&node)?,
                    rect: [r.left, r.top, r.right, r.bottom],
                    node,
                });
            }
        }
        result.sort_by_key(|r| r.rect[1]);
        Ok(result)
    }
    /// Targeted window messages only. Mouse-move establishes Qt's hover target;
    /// it does not call SetCursorPos, SendInput or change the user's cursor.
    pub(crate) fn scroll_messages(&self, h: HWND, up: bool) -> Result<()> {
        self.scroll_message_steps(h, up, 1)
    }
    fn scroll_message_steps(&self, h: HWND, up: bool, notches: i16) -> Result<()> {
        unsafe {
            let r = self
                .find(
                    &self.root(h)?,
                    UIA_AutomationIdPropertyId,
                    "chat_message_list",
                )?
                .CurrentBoundingRectangle()?;
            if r.right >= GetSystemMetrics(SM_XVIRTUALSCREEN)
                || r.bottom >= GetSystemMetrics(SM_YVIRTUALSCREEN)
            {
                bail!("消息列表没有保持离屏，未滚动");
            }
            let x = (r.left + r.right) / 2;
            let y = (r.top + r.bottom) / 2;
            let mut origin = POINT::default();
            windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut origin).ok()?;
            PostMessageW(
                Some(h),
                WM_MOUSEMOVE,
                WPARAM(0),
                LPARAM((((y - origin.y) << 16) | ((x - origin.x) & 0xffff)) as isize),
            )?;
            let delta: i16 = if up { 120 } else { -120 };
            for _ in 0..notches.clamp(1, 12) {
                PostMessageW(
                    Some(h),
                    WM_MOUSEWHEEL,
                    WPARAM((delta as u16 as usize) << 16),
                    LPARAM(((y << 16) | (x & 0xffff)) as isize),
                )?;
            }
        }
        std::thread::sleep(Duration::from_millis(if notches > 1 { 320 } else { 70 }));
        Ok(())
    }
    pub(crate) fn locate_message(
        &self,
        shared: &Shared,
        h: HWND,
        name: &str,
        key: &str,
    ) -> Result<IUIAutomationElement> {
        self.locate_message_inner(shared, h, name, key, false, None)
    }
    pub(crate) fn locate_forward_message(
        &self,
        shared: &Shared,
        h: HWND,
        name: &str,
        key: &str,
        view: &mut ForwardView,
    ) -> Result<IUIAutomationElement> {
        self.locate_message_inner(shared, h, name, key, false, Some(view))
    }
    #[allow(dead_code)]
    pub(crate) fn locate_message_header(
        &self,
        shared: &Shared,
        h: HWND,
        name: &str,
        key: &str,
    ) -> Result<IUIAutomationElement> {
        self.locate_message_inner(shared, h, name, key, true, None)
    }
    fn locate_message_inner(
        &self,
        shared: &Shared,
        h: HWND,
        name: &str,
        key: &str,
        header: bool,
        mut cache: Option<&mut ForwardView>,
    ) -> Result<IUIAutomationElement> {
        let (revision, db) = {
            let t = shared.timeline.lock().unwrap();
            let t = t
                .get(name)
                .ok_or_else(|| anyhow::anyhow!("会话数据库时间线尚未就绪"))?;
            (
                t.revision,
                t.rows
                    .iter()
                    .filter(|i| !i.labels.is_empty())
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        };
        let target = db
            .iter()
            .position(|i| i.key == key && i.own)
            .ok_or_else(|| anyhow::anyhow!("目标消息不在本账号已确认数据库时间线中"))?;
        let stamp = |rows: &[Visible]| {
            rows.iter()
                .map(|r| (r.runtime.clone(), r.rect))
                .collect::<Vec<_>>()
        };
        let first = self.visible_messages(h)?;
        let first_anchors = cache
            .as_ref()
            .filter(|c| c.revision == revision && c.name == name && c.hwnd == h.0 as isize)
            .map(|c| {
                first
                    .iter()
                    .enumerate()
                    .filter_map(|(u, row)| {
                        c.rows
                            .iter()
                            .find(|(_, label, runtime)| {
                                *label == row.label && *runtime == row.runtime
                            })
                            .map(|(d, _, _)| (u, *d))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let direct = anchored_start(
            &db,
            &first.iter().map(|r| r.label.clone()).collect::<Vec<_>>(),
            &first_anchors,
            db.len(),
        )
        .filter(|&s| target < s + first.len());
        let (mut visible, mut start) = if let Some(start) = direct {
            (first, start)
        } else {
            self.scroll_messages(h, true)?;
            let mut previous = self.visible_messages(h)?;
            let mut moved = stamp(&first) != stamp(&previous);
            let mut stable = 0;
            for _ in 0..80 {
                if shared.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    bail!("监听正在停止，未继续定位撤回消息");
                }
                self.scroll_message_steps(h, false, 12)?;
                let next = self.visible_messages(h)?;
                if stamp(&next) == stamp(&previous) {
                    stable += 1;
                } else {
                    moved = true;
                    stable = 0;
                }
                previous = next;
                if stable >= 3 {
                    break;
                }
            }
            let visible = previous;
            let start = tail_start(
            &db,
            &visible.iter().map(|r| r.label.clone()).collect::<Vec<_>>(),
        )
        .ok_or_else(|| {
            let _=crate::config::atomic(&shared.data.join("view-lookup-error.json"),&serde_json::json!({"key":key,"moved":moved,"stable":stable,"db_tail":db.iter().rev().take(12).map(|i|serde_json::json!({"key":i.key,"own":i.own,"labels":format!("{:?}",i.labels)})).collect::<Vec<_>>(),"ui":visible.iter().map(|v|serde_json::json!({"label":format!("{:?}",v.label),"rect":v.rect,"runtime":v.runtime})).collect::<Vec<_>>()}));
            anyhow::anyhow!("当前视口不能与数据库末尾逐条对齐，未猜测消息")
        })?;
            if stable < 3 || (!moved && db.len() != visible.len()) {
                let _ = crate::config::atomic(
                    &shared.data.join("view-end-error.json"),
                    &serde_json::json!({"stable":stable,"moved":moved,"ui":visible.iter().map(|r|serde_json::json!({"rect":r.rect,"runtime":r.runtime})).collect::<Vec<_>>(),"db_len":db.len()}),
                );
                bail!("无法确认消息列表已到末尾，未猜测撤回目标");
            }
            (visible, start)
        };
        for _ in 0..100 {
            let mut up = true;
            let mut steps = 4;
            if let Some(index) = target.checked_sub(start).filter(|&i| i < visible.len()) {
                let list = unsafe {
                    self.find(
                        &self.root(h)?,
                        UIA_AutomationIdPropertyId,
                        "chat_message_list",
                    )?
                    .CurrentBoundingRectangle()?
                };
                let r = visible[index].rect;
                let enough = if header {
                    r[1] >= list.top && r[1] + 32 <= list.bottom
                } else {
                    (r[3].min(list.bottom) - r[1].max(list.top))
                        >= ((r[3] - r[1]).min(64) - 4).max(16)
                };
                if enough {
                    let timelines = shared.timeline.lock().unwrap();
                    if timelines.get(name).is_none_or(|t| t.revision != revision) {
                        bail!("定位期间会话消息发生变化，未撤回其他消息");
                    }
                    shared.log(&format!("已按数据库时间线重新关联原消息：{key}"));
                    if let Some(cache) = cache.as_mut() {
                        **cache = ForwardView {
                            revision,
                            name: name.to_owned(),
                            hwnd: h.0 as isize,
                            rows: visible
                                .iter()
                                .enumerate()
                                .map(|(u, r)| (start + u, r.label.clone(), r.runtime.clone()))
                                .collect(),
                        };
                    }
                    return Ok(visible[index].node.clone());
                }
                up = r[1] < list.top;
                // A single text bubble can be several screens tall. Move
                // towards its required area without enumerating every 3 lines.
                let distance = if up {
                    list.top - r[1]
                } else {
                    r[1] - list.bottom
                };
                steps = (distance.max(0) / 48).clamp(1, 12) as i16;
            }
            if target >= start + visible.len() {
                bail!("目标在当前视口之后，未猜测消息");
            }
            self.scroll_message_steps(h, up, steps)?;
            let next = self.visible_messages(h)?;
            let anchors = next
                .iter()
                .enumerate()
                .filter_map(|(u, n)| {
                    visible
                        .iter()
                        .position(|o| o.runtime == n.runtime && o.label == n.label)
                        .map(|v| (u, start + v))
                })
                .collect::<Vec<_>>();
            let labels = next.iter().map(|r| r.label.clone()).collect::<Vec<_>>();
            let next_start = anchored_start(
                &db,
                &labels,
                &anchors,
                if up { start + visible.len() } else { db.len() },
            )
            .ok_or_else(|| {
                anyhow::anyhow!("滚动后的消息序列不能唯一关联数据库记录，未撤回其他消息")
            })?;
            start = next_start;
            visible = next;
        }
        bail!("原消息超过安全历史查找范围，未撤回其他消息")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn text(n: u8) -> Label {
        Label::Text([n; 32])
    }
    fn item(i: usize, label: Label) -> Item {
        Item {
            key: i.to_string(),
            labels: vec![label],
            own: true,
            order: (i as i64, String::new(), i as i64),
        }
    }
    #[test]
    fn repeated_old_cluster_is_not_mistaken_for_database_tail() {
        let labels = vec![
            text(1),
            text(2),
            text(3),
            Label::Image,
            text(1),
            text(2),
            text(3),
            Label::Image,
        ];
        let db: Vec<_> = labels
            .into_iter()
            .enumerate()
            .map(|(i, l)| item(i, l))
            .collect();
        assert_eq!(tail_start(&db, &[text(1), text(2), text(3)]), None);
        assert_eq!(tail_start(&db, &[text(2), text(3), Label::Image]), Some(5));
        assert_eq!(
            anchored_start(&db, &[text(1), text(2), text(3)], &[], 8),
            None
        );
        assert_eq!(
            anchored_start(&db, &[text(1), text(2), text(3)], &[(1, 5)], 8),
            Some(4)
        );
        assert_eq!(
            anchored_start(&db, &[text(1), text(2), text(3)], &[(1, 5), (2, 2)], 8),
            None
        );
    }
}
