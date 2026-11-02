use crate::{
    config::{self, Binding},
    database::Message,
    delivery::{Automation, Part},
    service::Shared,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::*,
        System::Ole::*,
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::BSTR,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Tracked {
    pub row: Message,
    pub runtime_id: Vec<i32>,
    pub sent_at: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_cgi: Option<crate::music_backend::Receipt>,
}
impl Tracked {
    pub fn was_recalled(&self) -> bool {
        self.native_cgi.as_ref().is_some_and(|r| r.recalled)
            || (self.row.kind == 10000 && self.row.content.contains("revokemsg"))
    }
}
pub fn append_sent(
    shared: &Shared,
    job: &crate::bridge::Job,
    binding: &Binding,
    tracked: Tracked,
) -> Result<()> {
    let mut ledger = shared.ledger.lock().unwrap();
    let record = ledger.sent.entry(job.id).or_insert_with(|| Record {
        name: job.name.clone(),
        group: job.group,
        binding: binding.clone(),
        messages: vec![],
        nodes: None,
        recalled: false,
    });
    record.messages.push(tracked);
    drop(ledger);
    save(shared)
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub name: String,
    pub group: bool,
    pub binding: Binding,
    pub messages: Vec<Tracked>,
    #[serde(default)]
    pub nodes: Option<Value>,
    #[serde(default)]
    pub recalled: bool,
}
#[derive(Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub sent: BTreeMap<u64, Record>,
    #[serde(default)]
    pub scheduled: BTreeMap<u64, f64>,
}
fn reconcile_row(ledger: &mut Ledger, name: &str, row: &Message) -> Vec<u64> {
    if row.kind != 10000 || !row.content.contains("revokemsg") || row.server_id == 0 {
        return vec![];
    }
    let mut changed = vec![];
    for (&id, record) in &mut ledger.sent {
        if record.name != name || record.recalled {
            continue;
        }
        for part in &mut record.messages {
            if part.native_cgi.is_none()
                && !part.was_recalled()
                && part.row.key() == row.key()
                && part.row.server_id == row.server_id
                && part.row.sort_seq == row.sort_seq
            {
                part.row = row.clone();
                changed.push(id);
            }
        }
        record.recalled =
            !record.messages.is_empty() && record.messages.iter().all(Tracked::was_recalled);
    }
    changed.sort_unstable();
    changed.dedup();
    changed
}
pub fn confirm_database_recall(shared: &Shared, name: &str, row: &Message) -> Result<()> {
    if row.kind != 10000 || !row.content.contains("revokemsg") {
        return Ok(());
    }
    let ids = reconcile_row(&mut shared.ledger.lock().unwrap(), name, row);
    if !ids.is_empty() {
        save(shared)?;
        for id in ids {
            crate::outbox::confirm_late_recall(shared, id)?;
        }
        shared.log(&format!(
            "数据库已确认撤回：会话={name}，消息={}",
            row.key()
        ));
    }
    if let Some(watches) = shared.recall_watch.lock().unwrap().get_mut(name) {
        watches.retain(|m| m.key() != row.key());
    }
    Ok(())
}
pub fn schedule(shared: &Shared, id: u64, delay_ms: f64) -> Result<f64> {
    if !delay_ms.is_finite() {
        bail!("撤回延迟不是有效数字");
    }
    let record = ensure_recall(shared, id)?;
    let at = record
        .messages
        .iter()
        .map(|m| m.sent_at)
        .fold(f64::INFINITY, f64::min);
    let c = shared.config.read().unwrap();
    let delay = recall_delay(&c.raw, delay_ms / 1000.);
    let due = (at + delay - (delay * 0.1).min(3.)).max(config::now());
    drop(c);
    let due = {
        let mut ledger = shared.ledger.lock().unwrap();
        let existing = ledger.scheduled.entry(id).or_insert(due);
        *existing = existing.min(due);
        *existing
    };
    save(shared)?;
    Ok(due)
}
pub fn due_recall(shared: &Shared) -> Option<u64> {
    let c = shared.config.read().unwrap();
    let ledger = shared.ledger.lock().unwrap();
    ledger
        .scheduled
        .iter()
        .filter_map(|(&id, &due)| {
            let record = ledger.sent.get(&id)?;
            let age_limit = record
                .messages
                .iter()
                .map(|m| m.sent_at + recall_limit(&c.raw) - 3.)
                .fold(f64::INFINITY, f64::min);
            let due = due.min(age_limit);
            if due <= config::now() {
                Some((id, due))
            } else {
                None
            }
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}
#[derive(Clone)]
pub struct Node {
    pub name: String,
    pub content: Value,
}
pub enum Operation {
    Send,
    Forward(Vec<Node>),
    Recall(u64),
}
pub fn recall_limit(raw: &Value) -> f64 {
    raw["recall_limit_seconds"]
        .as_f64()
        .filter(|v| v.is_finite())
        .unwrap_or(120.)
        .clamp(1., 120.)
}
pub fn recall_delay(raw: &Value, requested: f64) -> f64 {
    requested.max(0.).min(recall_limit(raw))
}
pub fn nodes(value: &Value, shared: &Shared) -> Result<Vec<Node>> {
    let mut result = vec![];
    let a = value.as_array().context("合并转发需要 node 数组")?;
    if a.is_empty() || a.len() > 100 || value.to_string().len() > 256 * 1024 {
        bail!("合并转发节点为空或内容超过限制");
    }
    for node in a {
        if node["type"] != "node" {
            bail!("合并转发只接受 node 节点");
        }
        let data = &node["data"];
        let name = data["nickname"]
            .as_str()
            .or(data["name"].as_str())
            .unwrap_or("转发消息")
            .to_owned();
        let content = if !data["content"].is_null() {
            data["content"].clone()
        } else {
            let id = data["id"]
                .as_u64()
                .or_else(|| data["id"].as_str().and_then(|s| s.parse().ok()))
                .context("节点缺少 content 或有效 id")?;
            let ledger = shared.ledger.lock().unwrap();
            let record = ledger
                .sent
                .get(&id)
                .context("没有对应原消息，未替换为其他消息")?;
            json!([{"type":"text","data":{"text":record.messages.iter().filter(|m|m.row.kind==1).map(|m|m.row.content.clone()).collect::<Vec<_>>().join("\n")}}])
        };
        result.push(Node { name, content });
    }
    Ok(result)
}
pub fn forward_parts(nodes: &[Node], shared: &Shared) -> Result<Vec<Part>> {
    let mut out = vec![];
    for node in nodes {
        let segments = if let Some(text) = node.content.as_str() {
            crate::delivery::parse_cq(text)
        } else {
            node.content
                .as_array()
                .context("节点 content 必须是文字或消息段数组")?
                .clone()
        };
        let mut buffer = format!("{}\n", node.name);
        for segment in segments {
            if segment["type"] == "text" {
                let text = segment["data"]["text"].as_str().context("节点文本无效")?;
                for line in text.split_inclusive('\n') {
                    if line.chars().count() > 3500 {
                        bail!("单行节点超过微信正文限制，未拆断链接或降级发送");
                    }
                    if buffer.chars().count() + line.chars().count() > 3500 {
                        out.push(Part::Text(std::mem::take(&mut buffer)));
                    }
                    buffer.push_str(line);
                }
            } else {
                if !buffer.is_empty() {
                    out.push(Part::Text(std::mem::take(&mut buffer)));
                }
                out.extend(crate::delivery::prepare(&json!([segment]), false, shared)?);
            }
        }
        if !buffer.is_empty() {
            out.push(Part::Text(buffer));
        }
    }
    if out.len() > 20 {
        bail!("原生合并转发需要超过20条中转消息，无法在OneBot请求期限内完成；未发送");
    }
    if out.iter().any(|p| matches!(p, Part::Music(_))) {
        bail!("原生音乐网络卡暂不支持合并转发节点，未发送中转消息")
    }
    Ok(out)
}
pub fn save(shared: &Shared) -> Result<()> {
    let mut ledger = shared.ledger.lock().unwrap();
    while ledger.sent.len() > 512 {
        ledger.sent.pop_first();
    }
    config::atomic(
        &shared.data.join("messages.json"),
        &serde_json::to_value(&*ledger)?,
    )
}
pub fn ensure_recall(shared: &Shared, id: u64) -> Result<Record> {
    let c = shared.config.read().unwrap();
    let b = c.binding()?;
    let ledger = shared.ledger.lock().unwrap();
    let record = ledger
        .sent
        .get(&id)
        .context("无法定位该消息；仅撤回本适配器已确认发送的消息")?;
    if record.recalled {
        return Ok(record.clone());
    }
    if record.binding.pid != b.pid || (record.binding.created_at - b.created_at).abs() > 0.01 {
        bail!("消息属于其他微信登录会话，未执行撤回");
    }
    if !c.allows(&record.name, record.group) {
        bail!("消息目标已移出白名单，未执行撤回");
    }
    let remaining = record
        .messages
        .iter()
        .filter(|m| !m.was_recalled())
        .collect::<Vec<_>>();
    if !remaining.is_empty()
        && remaining
            .iter()
            .all(|m| config::now() - m.sent_at >= recall_limit(&c.raw))
    {
        bail!("该回复的所有待撤回内容均已超过时限（最多120秒）");
    }
    Ok(record.clone())
}
impl Automation {
    pub(crate) fn cancel_actions(&self, h: HWND, data: &std::path::Path) {
        // Cleanup is a native UI operation too. Reuse the outer guard rather
        // than dropping focus protection before closing menus/multi-select.
        let Ok(_guard) = crate::popup::Guard::install(h, data) else {
            return;
        };
        unsafe {
            for popup in crate::native::thread_windows(h) {
                let _ = PostMessageW(Some(popup), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Ok(root) = self.root(h) {
                for name in ["取消多选", "取消"] {
                    if let Ok(button) = self.find(&root, UIA_NamePropertyId, name) {
                        let _ = self.click_native(&button, h);
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(80));
    }
    pub(crate) fn forward(
        &self,
        shared: &Shared,
        job: &crate::bridge::Job,
        nodes: &[Node],
    ) -> Result<()> {
        unsafe {
            let c = shared.config.read().unwrap().clone();
            let binding = c.binding()?;
            let h = crate::native::validate_window(&binding)?;
            let staging = shared
                .staging
                .read()
                .unwrap()
                .clone()
                .context("中转会话尚未验证为本账号自己，请核对账号页设置的自聊名称")?;
            let _guard = crate::popup::Guard::install(h, &shared.data)?;
            self.select(h, &staging.name, false)?;
            let mut staged = vec![];
            for part in &job.parts {
                if Instant::now() >= job.deadline || !shared.owner(job.owner) {
                    bail!("中转动作已过期或连接已关闭，未合并发出");
                }
                let before = shared.checkpoint();
                let at = config::now();
                self.send_part(shared, h, &staging.name, false, part, job)?;
                let tracked = self.track(shared, h, &staging.name, part, before, at)?;
                if tracked.runtime_id.is_empty() {
                    bail!("中转消息没有可验证的控件标识，未选择历史消息");
                }
                staged.push(tracked);
            }
            let newest = staged.last().context("没有可合并的中转消息")?;
            let newest_row =
                self.locate_message_header(shared, h, &staging.name, &newest.row.key())?;
            self.context(h, &newest_row)?;
            self.click_native(&self.named_action(h, "多选")?, h)?;
            std::thread::sleep(Duration::from_millis(160));
            // Entering native multi-select starts a fresh selection. Clear its
            // initially selected context row before adding exactly our DB keys.
            for row in self.rows(h)? {
                self.choose_forward_row(h, &row, false)?;
            }
            let mut verified = std::collections::BTreeSet::new();
            let mut view = crate::message_view::ForwardView::default();
            for tracked in staged.iter().rev() {
                job.check_submission(shared)?;
                let row = self.locate_forward_message(
                    shared,
                    h,
                    &staging.name,
                    &tracked.row.key(),
                    &mut view,
                )?;
                self.choose_forward_row(h, &row, true)?;
                verified.insert(tracked.row.key());
            }
            if verified.len() != staged.len() {
                bail!("中转消息未全部核对选中，未转发");
            }
            shared.log(&format!(
                "已按数据库键逐条核对中转选择：{}/{}",
                verified.len(),
                staged.len()
            ));
            self.click_native(&self.named_action(h, "合并转发")?, h)?;
            let (before, at) = self.forward_picker(shared, job, h)?;
            let row = shared
                .own
                .lock()
                .unwrap()
                .rows
                .get(&job.name)
                .and_then(|rows| {
                    rows.iter()
                        .rev()
                        .find(|r| {
                            r.serial > before
                                && r.message.kind == 49
                                && r.message.server_id > 0
                                && r.message.content.contains("<type>19</type>")
                        })
                        .map(|r| r.message.clone())
                })
                .context("未观察到微信原生type19聊天记录卡片")?;
            self.select(h, &job.name, job.group)?;
            let mut runtime_id = vec![];
            for r in self.rows(h)? {
                if r.CurrentClassName()?.to_string().contains("ChatRecord")
                    || r.CurrentName()?.to_string().contains("聊天记录")
                {
                    runtime_id = self.runtime_id(&r)?;
                }
            }
            let node_data = Value::Array(
                nodes
                    .iter()
                    .map(|n| json!({"type":"node","data":{"nickname":n.name,"content":n.content}}))
                    .collect(),
            );
            shared.ledger.lock().unwrap().sent.insert(
                job.id,
                Record {
                    name: job.name.clone(),
                    group: job.group,
                    binding,
                    messages: vec![Tracked {
                        native_cgi: None,
                        row,
                        runtime_id,
                        sent_at: at,
                    }],
                    nodes: Some(node_data),
                    recalled: false,
                },
            );
            save(shared)?;
            let count = crate::record_card::verify(
                &shared.ledger.lock().unwrap().sent[&job.id].messages[0]
                    .row
                    .content,
                &staged,
            )?;
            shared.log(&format!(
                "微信原生合并卡完整性确认：{count}个中转项，文本及顺序全部一致"
            ));
            shared.log("微信原生合并转发卡片已由数据库确认（type19）");
            Ok(())
        }
    }
    pub(crate) fn forward_picker(
        &self,
        shared: &Shared,
        job: &crate::bridge::Job,
        h: HWND,
    ) -> Result<(u64, f64)> {
        unsafe {
            let deadline = Instant::now() + Duration::from_secs(2);
            let (mut picker_h, mut picker) = (None, None);
            while Instant::now() < deadline {
                for p in crate::native::thread_windows(h) {
                    let root = self.root(p)?;
                    if root
                        .CurrentClassName()?
                        .to_string()
                        .contains("SessionPickerWindow")
                    {
                        picker_h = Some(p);
                        picker = Some(root);
                        break;
                    }
                }
                if picker.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(60));
            }
            let picker = picker.context("微信原生收件人选择框未出现")?;
            let ph = picker_h.unwrap();
            let mut search = None;
            for node in self.descendants(&picker)? {
                if node.CurrentControlType()? == UIA_EditControlTypeId
                    && node.CurrentName()?.to_string() == "搜索"
                {
                    search = Some(node);
                    break;
                }
            }
            let search = search.context("原生转发搜索框不可用")?;
            let input: IUIAutomationValuePattern =
                search.GetCurrentPatternAs(UIA_ValuePatternId)?;
            input.SetValue(&BSTR::from(&job.name))?;
            let until = Instant::now() + Duration::from_secs(3);
            let mut candidates = vec![];
            while Instant::now() < until {
                candidates = self
                    .descendants(&picker)?
                    .into_iter()
                    .filter(|node| {
                        node.CurrentClassName().is_ok_and(|s| {
                            let s = s.to_string();
                            s.contains("SPSelectionContactRow")
                                || s.contains("SearchContactCellView")
                        }) && node
                            .CurrentName()
                            .is_ok_and(|s| s.to_string().trim() == job.name)
                    })
                    .collect();
                if !candidates.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(70));
            }
            if candidates.len() != 1 {
                bail!("原生转发的精确收件人未找到或不唯一，未发送");
            }
            let selected_before: Vec<_> = self
                .descendants(&picker)?
                .into_iter()
                .filter(|n| {
                    n.CurrentClassName()
                        .is_ok_and(|s| s.to_string().contains("SPChoiceContactRow"))
                })
                .collect();
            let row_rect = candidates[0].CurrentBoundingRectangle()?;
            if selected_before.is_empty() {
                self.click_position(
                    &candidates[0],
                    ph,
                    Some((row_rect.left + 70, (row_rect.top + row_rect.bottom) / 2)),
                )?;
            }
            let until = Instant::now() + Duration::from_millis(1500);
            let mut choices = vec![];
            while Instant::now() < until {
                choices = self
                    .descendants(&picker)?
                    .into_iter()
                    .filter(|node| {
                        node.CurrentClassName()
                            .is_ok_and(|s| s.to_string().contains("SPChoiceContactRow"))
                    })
                    .collect();
                if !choices.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(60));
            }
            let chosen_names: std::collections::BTreeSet<_> = choices
                .iter()
                .filter_map(|n| n.CurrentName().ok())
                .filter_map(|name| {
                    name.to_string()
                        .strip_prefix("移除")
                        .map(|s| s.trim().to_owned())
                })
                .collect();
            if chosen_names.len() != 1 || !chosen_names.contains(&job.name) {
                let mut window = RECT::default();
                GetWindowRect(ph, &mut window)?;
                config::atomic(
                    &shared.data.join("picker-selection-probe.json"),
                    &json!({"selected_before":selected_before.len(),"choices":choices.iter().map(|n|n.CurrentName().map(|s|s.to_string()).unwrap_or_default()).collect::<Vec<_>>(),"candidate_class":candidates[0].CurrentClassName()?.to_string(),"candidate_rect":[row_rect.left,row_rect.top,row_rect.right,row_rect.bottom],"window_rect":[window.left,window.top,window.right,window.bottom]}),
                )?;
                crate::native::capture(ph)?.save(shared.data.join("picker-selection-probe.png"))?;
                bail!("原生转发的已选收件人核对失败，未发送");
            }
            if !shared.config.read().unwrap().allows(&job.name, job.group)
                || Instant::now() >= job.deadline
            {
                bail!("原生转发提交前已过期或目标已移出白名单");
            }
            let button = self.find(&picker, UIA_AutomationIdPropertyId, "confirm_btn")?;
            if button.CurrentName()?.to_string() != "发送" {
                bail!("原生转发收件人数量不为1，未发送");
            }
            let before = shared.checkpoint();
            let at = config::now();
            job.check_submission(shared)?;
            self.click_native(&button, ph)?;
            if !shared.confirm(&job.name, 49, None, before) {
                bail!("未确认原生转发卡片；不替换成文字或盲目重发");
            }
            Ok((before, at))
        }
    }
    fn choose_forward_row(&self, h: HWND, row: &IUIAutomationElement, want: bool) -> Result<()> {
        unsafe {
            if let Ok(p) = row.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                UIA_SelectionItemPatternId,
            ) {
                if p.CurrentIsSelected()?.as_bool() != want {
                    if want {
                        p.AddToSelection()?;
                    } else {
                        p.RemoveFromSelection()?;
                    }
                }
                if p.CurrentIsSelected()?.as_bool() == want {
                    return Ok(());
                }
                bail!("原生多选状态未确认，未继续转发");
            }
            let mut candidates = vec![row.clone()];
            candidates.extend(self.descendants(row)?);
            for check in candidates {
                if check.CurrentControlType()? != UIA_CheckBoxControlTypeId {
                    continue;
                }
                let Ok(toggle) =
                    check.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                else {
                    continue;
                };
                if (toggle.CurrentToggleState()? == ToggleState_On) == want {
                    return Ok(());
                }
                toggle.Toggle()?;
                std::thread::sleep(Duration::from_millis(60));
                for _ in 0..80 {
                    if (toggle.CurrentToggleState()? == ToggleState_On) == want {
                        return Ok(());
                    }
                    let r = check.CurrentBoundingRectangle()?;
                    let list = self
                        .find(
                            &self.root(h)?,
                            UIA_AutomationIdPropertyId,
                            "chat_message_list",
                        )?
                        .CurrentBoundingRectangle()?;
                    let top = r.top.max(list.top + 8);
                    let bottom = r.bottom.min(list.bottom - 8);
                    if bottom - top < 16 {
                        self.scroll_messages(h, r.top < list.top + 8)?;
                    } else {
                        // Qt exposes the entire selectable message as a
                        // checkbox, including long rows taller than the viewport.
                        // Click its verified visible intersection, not its
                        // offscreen geometric centre.
                        self.click_position(
                            &check,
                            h,
                            Some(((r.left + r.right) / 2, (top + bottom) / 2)),
                        )?;
                        std::thread::sleep(Duration::from_millis(80));
                    }
                }
                bail!("中转消息复选框状态未确认，未转发");
            }
            bail!("中转消息缺少可读的原生选择状态，未转发")
        }
    }
    pub(crate) fn open_search(&self, h: HWND, name: &str, group: bool) -> Result<()> {
        unsafe {
            let root = self.root(h)?;
            let search = (|| -> Result<IUIAutomationElement> {
                for node in self.descendants(&root)? {
                    if node.CurrentControlType()? == UIA_EditControlTypeId
                        && ["搜索", "Search"].contains(&node.CurrentName()?.to_string().as_str())
                    {
                        return Ok(node);
                    }
                }
                bail!("会话搜索框未展开")
            })();
            let search = match search {
                Ok(s) => s,
                Err(_) => {
                    let button = self
                        .find(&root, UIA_NamePropertyId, "搜索")
                        .or_else(|_| self.find(&root, UIA_NamePropertyId, "Search"))?;
                    self.click_native(&button, h)?;
                    std::thread::sleep(Duration::from_millis(150));
                    let mut found = None;
                    for node in self.descendants(&root)? {
                        if node.CurrentControlType()? == UIA_EditControlTypeId
                            && ["搜索", "Search"]
                                .contains(&node.CurrentName()?.to_string().as_str())
                        {
                            found = Some(node);
                            break;
                        }
                    }
                    found.context("会话搜索输入框不可用")?
                }
            };
            let value: IUIAutomationValuePattern =
                search.GetCurrentPatternAs(UIA_ValuePatternId)?;
            if !value.CurrentValue()?.is_empty() {
                value.SetValue(&BSTR::new())?;
                std::thread::sleep(Duration::from_millis(40));
            }
            value.SetValue(&BSTR::from(name))?;
            let until = Instant::now() + Duration::from_millis(1500);
            let mut selected = None;
            while Instant::now() < until {
                let mut matches = vec![];
                for node in self.descendants(&root)? {
                    if node.CurrentName()?.to_string() == name
                        && (node.CurrentControlType()? == UIA_ListItemControlTypeId
                            || node
                                .CurrentClassName()?
                                .to_string()
                                .contains("SearchResult"))
                    {
                        matches.push(node);
                    }
                }
                if matches.len() == 1 {
                    selected = matches.pop();
                    break;
                }
                if matches.len() > 1 {
                    let _ = value.SetValue(&BSTR::new());
                    bail!("搜索结果不唯一，未选择其他会话");
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            let row = selected.context("未找到精确会话搜索结果")?;
            self.click_native(&row, h)?;
            let until = Instant::now() + Duration::from_millis(1200);
            while Instant::now() < until {
                if crate::delivery::title_matches(&self.title(h).unwrap_or_default(), name, group) {
                    let _ = value.SetValue(&BSTR::new());
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(60));
            }
            let _ = value.SetValue(&BSTR::new());
            bail!("搜索后目标会话复核失败；未发送")
        }
    }
    pub(crate) fn rows(&self, h: HWND) -> Result<Vec<IUIAutomationElement>> {
        unsafe {
            let root = self.root(h)?;
            let list = self.find(&root, UIA_AutomationIdPropertyId, "chat_message_list")?;
            let condition = self.automation.CreatePropertyCondition(
                UIA_AutomationIdPropertyId,
                &windows::Win32::System::Variant::VARIANT::from(
                    "chat_message_list.qt_scrollarea_viewport.chat_bubble_item_view",
                ),
            )?;
            if let Ok(rows) = list.FindAll(TreeScope_Descendants, &condition) {
                let n = rows.Length()?;
                if n > 0 {
                    return (0..n)
                        .map(|i| rows.GetElement(i).map_err(Into::into))
                        .collect();
                }
            }
            Ok(self
                .descendants(&list)?
                .into_iter()
                .filter(|e| {
                    e.CurrentAutomationId()
                        .is_ok_and(|s| s.to_string().ends_with("chat_bubble_item_view"))
                })
                .collect())
        }
    }
    pub(crate) fn runtime_id(&self, node: &IUIAutomationElement) -> Result<Vec<i32>> {
        unsafe {
            let array = node.GetRuntimeId()?;
            let result = (|| -> Result<Vec<i32>> {
                let low = SafeArrayGetLBound(array, 1)?;
                let high = SafeArrayGetUBound(array, 1)?;
                let mut result = vec![];
                for i in low..=high {
                    let mut v = 0i32;
                    SafeArrayGetElement(array, &i, &mut v as *mut _ as _)?;
                    result.push(v);
                }
                Ok(result)
            })();
            let _ = SafeArrayDestroy(array);
            result
        }
    }
    pub(crate) fn track(
        &self,
        shared: &Shared,
        h: HWND,
        name: &str,
        part: &Part,
        before: u64,
        sent_at: f64,
    ) -> Result<Tracked> {
        if matches!(part, Part::Music(_)) {
            bail!("音乐应使用匹配的原生网络回执，不能伪造数据库确认")
        }
        let row = shared
            .own
            .lock()
            .unwrap()
            .rows
            .get(name)
            .and_then(|rows| {
                rows.iter()
                    .rev()
                    .find(|r| {
                        r.serial > before
                            && r.message.server_id > 0
                            && match part {
                                Part::Text(t) => {
                                    r.message.kind == 1
                                        && normalize(&r.message.content) == normalize(t)
                                }
                                Part::Image(_) => r.message.kind == 3,
                                Part::Music(card) => {
                                    r.message.kind == 49
                                        && crate::music::parse_native(&r.message.content)
                                            .ok()
                                            .flatten()
                                            .is_some_and(|native| {
                                                crate::music::MusicRequest::Card(card.card.clone())
                                                    .matches_cached(&native)
                                            })
                                }
                            }
                    })
                    .map(|r| r.message.clone())
            })
            .context("发送已尝试，但消息关联记录未确认")?;
        let deadline = Instant::now() + Duration::from_millis(700);
        loop {
            let mut found = None;
            for node in self.rows(h)? {
                unsafe {
                    let matches = match part {
                        Part::Text(t) => {
                            normalize(&node.CurrentName()?.to_string()) == normalize(t)
                        }
                        Part::Image(_) => {
                            node.CurrentClassName()?.to_string().contains("Image")
                                || node.CurrentClassName()?.to_string().contains("Pic")
                                || node.CurrentName()?.to_string() == "[图片]"
                                || node.CurrentName()?.to_string() == "图片"
                        }
                        Part::Music(_) => false,
                    };
                    if matches {
                        found = Some(node);
                    }
                }
            }
            if let Some(node) = found {
                return Ok(Tracked {
                    native_cgi: None,
                    row,
                    runtime_id: self.runtime_id(&node)?,
                    sent_at,
                });
            }
            if Instant::now() >= deadline {
                return Ok(Tracked {
                    native_cgi: None,
                    row,
                    runtime_id: vec![],
                    sent_at,
                });
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    pub(crate) fn context(&self, h: HWND, row: &IUIAutomationElement) -> Result<()> {
        unsafe {
            let rect = row.CurrentBoundingRectangle()?;
            let root = self.root(h)?;
            let list = self
                .find(&root, UIA_AutomationIdPropertyId, "chat_message_list")?
                .CurrentBoundingRectangle()?;
            let mut window = RECT::default();
            GetWindowRect(h, &mut window)?;
            let mut origin = POINT::default();
            windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut origin).ok()?;
            if rect.bottom <= list.top || rect.top >= list.bottom {
                bail!("目标消息不在已验证可操作区域，未操作其他消息");
            }
            let picture = crate::native::capture(h)?;
            let mut sum = 0u64;
            let mut count = 0u64;
            for y in (rect.top.max(list.top) - window.top).max(0)
                ..(rect.bottom.min(list.bottom) - window.top).min(picture.height() as i32)
            {
                for x in (rect.left - window.left).max(0)
                    ..(rect.right - window.left).min(picture.width() as i32)
                {
                    let p = picture.get_pixel(x as _, y as _);
                    if p[1] as i32 > p[0] as i32 + 18 && p[1] as i32 > p[2] as i32 + 18 && p[1] > 45
                    {
                        sum += x as u64;
                        count += 1;
                    }
                }
            }
            let x = if count > 20 {
                (sum / count) as i32 + window.left - origin.x
            } else {
                // All callers correlate an outgoing row with an acknowledged
                // own database message. Cards/images have no green bubble.
                rect.right - 140 - origin.x
            };
            let y = (rect.top.max(list.top) + rect.bottom.min(list.bottom)) / 2 - origin.y;
            for (msg, key) in [(WM_MOUSEMOVE, 0), (WM_RBUTTONDOWN, 2), (WM_RBUTTONUP, 0)] {
                PostMessageW(
                    Some(h),
                    msg,
                    WPARAM(key),
                    LPARAM(((y << 16) | (x & 0xffff)) as isize),
                )?;
                std::thread::sleep(Duration::from_millis(25));
            }
            std::thread::sleep(Duration::from_millis(120));
            Ok(())
        }
    }
    fn action_roots(&self, h: HWND) -> Result<Vec<IUIAutomationElement>> {
        let mut roots = vec![self.root(h)?];
        for popup in crate::native::thread_windows(h) {
            roots.push(self.root(popup)?);
        }
        Ok(roots)
    }
    pub(crate) fn named_action(&self, h: HWND, name: &str) -> Result<IUIAutomationElement> {
        {
            let until = Instant::now() + Duration::from_millis(900);
            loop {
                for root in self.action_roots(h)? {
                    if let Ok(node) = self.find(&root, UIA_NamePropertyId, name) {
                        return Ok(node);
                    }
                }
                if Instant::now() >= until {
                    bail!("原生操作【{name}】不可用");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    pub(crate) fn click_native(&self, node: &IUIAutomationElement, fallback: HWND) -> Result<()> {
        self.click_position(node, fallback, None)
    }
    pub(crate) fn click_position(
        &self,
        node: &IUIAutomationElement,
        fallback: HWND,
        at: Option<(i32, i32)>,
    ) -> Result<()> {
        unsafe {
            let rect = node.CurrentBoundingRectangle()?;
            if rect.right >= GetSystemMetrics(SM_XVIRTUALSCREEN)
                || rect.bottom >= GetSystemMetrics(SM_YVIRTUALSCREEN)
            {
                bail!("操作控件未保持离屏，已停止");
            }
            let mut h = node.CurrentNativeWindowHandle()?;
            if h.is_invalid() {
                let walker = self.automation.RawViewWalker()?;
                let mut parent = walker.GetParentElement(node).ok();
                for _ in 0..30 {
                    let Some(p) = parent else { break };
                    h = p.CurrentNativeWindowHandle()?;
                    if !h.is_invalid() {
                        break;
                    }
                    parent = walker.GetParentElement(&p).ok();
                }
                if h.is_invalid() {
                    h = fallback;
                }
            }
            let top = GetAncestor(h, GA_ROOT);
            if !top.is_invalid() {
                h = top;
            }
            let mut origin = POINT::default();
            windows::Win32::Graphics::Gdi::ClientToScreen(h, &mut origin).ok()?;
            let (x, y) = at.unwrap_or(((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2));
            let x = x - origin.x;
            let y = y - origin.y;
            for (msg, key) in [(WM_MOUSEMOVE, 0), (WM_LBUTTONDOWN, 1), (WM_LBUTTONUP, 0)] {
                PostMessageW(
                    Some(h),
                    msg,
                    WPARAM(key),
                    LPARAM(((y << 16) | (x & 0xffff)) as isize),
                )?;
            }
            Ok(())
        }
    }
    pub(crate) fn recall(&self, shared: &Shared, id: u64) -> Result<()> {
        let record = ensure_recall(shared, id)?;
        if record.recalled {
            return Ok(());
        }
        let c = shared.config.read().unwrap().clone();
        let current = crate::native::Process::bound(&c.binding()?)?.account_root()?;
        if shared
            .bound_root
            .read()
            .unwrap()
            .as_ref()
            .is_none_or(|root| !crate::database::same_root(root, &current))
        {
            bail!("微信账号已改变，未执行撤回")
        }
        let h = crate::native::validate_window(&c.binding()?)?;
        let has_ui = record.messages.iter().any(|m| m.native_cgi.is_none());
        let _guard = if has_ui {
            Some(crate::popup::Guard::install(h, &shared.data)?)
        } else {
            None
        };
        if has_ui {
            self.select(h, &record.name, record.group)?;
        }
        let mut errors = vec![];
        for tracked in &record.messages {
            if tracked.was_recalled() {
                continue;
            }
            let attempt = (|| -> Result<()> {
                if shared.stop.load(std::sync::atomic::Ordering::SeqCst)
                    || config::now() - tracked.sent_at >= recall_limit(&c.raw)
                {
                    bail!("撤回动作执行前已过期，未撤回其他消息");
                }
                if let Some(receipt) = &tracked.native_cgi {
                    return crate::music_backend::recall(
                        shared,
                        &record.name,
                        record.group,
                        receipt,
                        tracked.sent_at,
                    );
                }
                let row = self.locate_message(shared, h, &record.name, &tracked.row.key())?;
                if config::now() - tracked.sent_at >= recall_limit(&c.raw)
                    || shared.stop.load(std::sync::atomic::Ordering::SeqCst)
                {
                    bail!("重新定位后撤回时限已到，未提交");
                }
                self.context(h, &row)?;
                // Keep observing after a UI timeout: the database can confirm
                // later. This watch never re-submits an action and expires in
                // the receiver, independently of the 120-second submit limit.
                {
                    let mut watches = shared.recall_watch.lock().unwrap();
                    let rows = watches.entry(record.name.clone()).or_default();
                    if !rows.iter().any(|r| r.key() == tracked.row.key()) {
                        rows.push(tracked.row.clone());
                    }
                }
                let action = self.named_action(h, "撤回")?;
                self.click_native(&action, h)?;
                std::thread::sleep(Duration::from_millis(150));
                // Some builds ask for confirmation. Only invoke an explicit recall
                // confirmation in an owned offscreen window, never a generic Enter.
                for popup in crate::native::thread_windows(h) {
                    let root = self.root(popup)?;
                    if let Ok(button) = self.find(&root, UIA_NamePropertyId, "撤回") {
                        self.click_native(&button, popup)?;
                    }
                }
                let limit = Instant::now() + Duration::from_secs(5);
                loop {
                    let changed = shared
                        .own
                        .lock()
                        .unwrap()
                        .rows
                        .get(&record.name)
                        .is_some_and(|rows| {
                            rows.iter().any(|r| {
                                r.message.key() == tracked.row.key()
                                    && r.message.kind == 10000
                                    && r.message.content.contains("revokemsg")
                            })
                        });
                    if changed {
                        break;
                    }
                    if Instant::now() >= limit {
                        let mut popups = vec![];
                        for (index, popup) in
                            crate::native::thread_windows(h).into_iter().enumerate()
                        {
                            if let Ok(root) = self.root(popup) {
                                let mut nodes = vec![];
                                for node in self.descendants(&root)? {
                                    unsafe {
                                        nodes.push(json!({"class":node.CurrentClassName()?.to_string(),"name":node.CurrentName()?.to_string(),"role":node.CurrentControlType()?.0}));
                                    }
                                }
                                popups.push(json!({"nodes":nodes,"class":unsafe{root.CurrentClassName()?.to_string()}}));
                                if let Ok(picture) = crate::native::capture(popup) {
                                    let _ = picture.save(
                                        shared.data.join(format!("recall-popup-{index}.png")),
                                    );
                                }
                            }
                        }
                        config::atomic(
                            &shared.data.join("recall-popup-probe.json"),
                            &json!(popups),
                        )?;
                        bail!("已请求撤回，但数据库尚未确认撤回结果；不重复执行");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(())
            })();
            match attempt {
                Ok(()) => {
                    if let Some(r) = shared.ledger.lock().unwrap().sent.get_mut(&id) {
                        if let Some(part) = r
                            .messages
                            .iter_mut()
                            .find(|p| p.row.key() == tracked.row.key())
                        {
                            if let Some(receipt) = part.native_cgi.as_mut() {
                                receipt.recalled = true
                            } else {
                                part.row.kind = 10000;
                                part.row.content = "<revokemsg/>".into();
                            }
                        }
                    }
                    save(shared)?;
                }
                Err(e) => {
                    errors.push(format!("{}: {e}", tracked.row.key()));
                    if tracked.native_cgi.is_none() {
                        self.cancel_actions(h, &shared.data);
                    }
                }
            }
        }
        {
            let mut ledger = shared.ledger.lock().unwrap();
            if let Some(r) = ledger.sent.get_mut(&id) {
                r.recalled = r.messages.iter().all(Tracked::was_recalled);
            }
            ledger.scheduled.remove(&id);
        }
        save(shared)?;
        if !errors.is_empty() {
            bail!(
                "批次中的原生消息已逐条尝试；{}项失败：{}",
                errors.len(),
                errors.join("；")
            );
        }
        shared.log("该输出的撤回已确认（微信数据库或原生网络回执）");
        Ok(())
    }
}
fn normalize(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_database_recall_requires_identity_and_preserves_other_outputs() {
        let row = Message {
            local_id: 1,
            server_id: 91,
            sort_seq: 100,
            create_time: 100,
            kind: 3,
            sender: "self".into(),
            content: "poster".into(),
            shard: "message_0".into(),
        };
        let mut text = row.clone();
        text.local_id = 2;
        text.server_id = 92;
        text.kind = 1;
        let binding = Binding {
            pid: 1,
            created_at: 1.,
            hwnd: 1,
            bound_at: 1.,
        };
        let part = |r: Message| Tracked {
            row: r,
            runtime_id: vec![],
            sent_at: 100.,
            native_cgi: None,
        };
        let mut ledger = Ledger::default();
        let record = Record {
            name: "甲".into(),
            group: false,
            binding,
            messages: vec![part(row.clone()), part(text.clone())],
            nodes: None,
            recalled: false,
        };
        ledger.sent.insert(1, record.clone());
        let mut other = record;
        other.messages[0].row.local_id = 3;
        other.messages[0].row.server_id = 93;
        ledger.sent.insert(2, other);
        let mut revoked = row.clone();
        revoked.kind = 10000;
        revoked.content = "<sysmsg><revokemsg/></sysmsg>".into();
        assert!(reconcile_row(&mut ledger, "乙", &revoked).is_empty());
        revoked.server_id = 999;
        assert!(reconcile_row(&mut ledger, "甲", &revoked).is_empty());
        revoked.server_id = 91;
        assert_eq!(reconcile_row(&mut ledger, "甲", &revoked), vec![1]);
        assert!(!ledger.sent[&1].recalled);
        text.kind = 10000;
        text.content = revoked.content.clone();
        assert_eq!(reconcile_row(&mut ledger, "甲", &text), vec![1, 2]);
        assert!(ledger.sent[&1].recalled);
        assert!(!ledger.sent[&2].recalled);
        assert!(reconcile_row(&mut ledger, "甲", &revoked).is_empty());
    }
    #[test]
    fn native_receipt_roundtrips_without_claiming_a_database_revoke() {
        let row = Message {
            local_id: 1,
            server_id: -1,
            sort_seq: 1,
            create_time: 1,
            kind: 49,
            sender: "wxid_self".into(),
            content: "native-music".into(),
            shard: "native-cgi-local".into(),
        };
        let receipt = crate::music_backend::Receipt {
            from: "wxid_self".into(),
            to: "wxid_peer".into(),
            client_id: "peer_1_1".into(),
            local_id: 1,
            create_time: 1,
            server_id: u64::MAX.to_string(),
            recalled: false,
        };
        let mut item = Tracked {
            row,
            runtime_id: vec![],
            sent_at: 1.,
            native_cgi: Some(receipt),
        };
        assert!(!item.was_recalled());
        let value = serde_json::to_value(&item).unwrap();
        let restored: Tracked = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(restored.native_cgi.unwrap().server_id, u64::MAX.to_string());
        item.native_cgi.as_mut().unwrap().recalled = true;
        assert!(item.was_recalled());
        assert_eq!(item.row.kind, 49);
        assert_eq!(item.row.content, "native-music");
        let mut legacy = value;
        legacy.as_object_mut().unwrap().remove("native_cgi");
        let legacy: Tracked = serde_json::from_value(legacy).unwrap();
        assert!(legacy.native_cgi.is_none());
    }
    #[test]
    fn recall_never_exceeds_two_minutes() {
        assert_eq!(recall_limit(&json!({"recall_limit_seconds":999})), 120.);
        assert_eq!(recall_delay(&json!({"recall_limit_seconds":90}), 180.), 90.);
        assert_eq!(recall_limit(&json!({"recall_limit_seconds":-5})), 1.);
    }
    #[test]
    fn timer_clamps_native_age_persists_and_keeps_earliest_deadline() {
        let d = tempfile::tempdir().unwrap();
        let mut c = crate::config::Config::default(d.path().join("config.json"));
        c.raw["whitelist"] = json!(["测试"]);
        c.raw["wechat_binding"] = json!({"pid":1,"created_at":1.,"hwnd":1});
        let binding = c.binding().unwrap();
        let (shared, _) = Shared::new(c);
        let at = config::now();
        let record = Record {
            name: "测试".into(),
            group: false,
            binding,
            messages: vec![Tracked {
                native_cgi: None,
                row: Message {
                    local_id: 1,
                    server_id: 2,
                    sort_seq: 1,
                    create_time: at as i64,
                    kind: 1,
                    sender: "self".into(),
                    content: "正文".into(),
                    shard: "message_0".into(),
                },
                runtime_id: vec![1],
                sent_at: at,
            }],
            nodes: None,
            recalled: false,
        };
        shared.ledger.lock().unwrap().sent.insert(1, record);
        let cap = schedule(&shared, 1, 999999.).unwrap();
        assert!((cap - at - 117.).abs() < 0.1);
        let earlier = schedule(&shared, 1, 5000.).unwrap();
        assert!(earlier < cap);
        assert_eq!(schedule(&shared, 1, 10000.).unwrap(), earlier);
        let persisted: Ledger =
            serde_json::from_value(config::load(&shared.data.join("messages.json")).unwrap())
                .unwrap();
        assert!((persisted.scheduled[&1] - earlier).abs() < 0.00001);
        shared
            .ledger
            .lock()
            .unwrap()
            .sent
            .get_mut(&1)
            .unwrap()
            .messages[0]
            .sent_at -= 121.;
        assert!(ensure_recall(&shared, 1).is_err());
    }
    #[test]
    fn forward_keeps_both_node_aliases_case_and_complete_magnets() {
        let d = tempfile::tempdir().unwrap();
        let (shared, _) = Shared::new(crate::config::Config::default(d.path().join("config.json")));
        let values = json!([{"type":"node","data":{"name":"甲","uin":"1","content":"ccc1\nmagnet:?xt=urn:btih:ABCDEF"}},{"type":"node","data":{"nickname":"乙","user_id":"2","content":[{"type":"text","data":{"text":"CCC1"}}]}}]);
        let nodes = nodes(&values, &shared).unwrap();
        let parts = forward_parts(&nodes, &shared).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0],Part::Text(t) if t=="甲\nccc1\nmagnet:?xt=urn:btih:ABCDEF"));
        assert!(matches!(&parts[1],Part::Text(t) if t=="乙\nCCC1"));
        let too_long = vec![Node {
            name: "甲".into(),
            content: json!("x".repeat(3501)),
        }];
        assert!(forward_parts(&too_long, &shared).is_err());
    }
}
