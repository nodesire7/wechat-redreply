//! Optional text-only AI orchestration. No native UI, clipboard, input or
//! transport calls occur during generation. RedReply explicitly owns routing.
pub mod config;
mod store;
#[cfg(test)]
mod tests;
mod worker;

use crate::{config::Config, service::Shared};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::sync::Arc;
pub use store::{cancel, clear_context, list};
pub use worker::run;

pub fn observe(
    shared: &Shared,
    peer: &crate::database::Peer,
    uid: u64,
    row: &crate::database::Message,
    id: u64,
) -> Result<()> {
    let c = shared.config.read().unwrap().clone();
    if !config::Settings::read(&c.raw["ai"])?.enabled {
        return Ok(());
    }
    let account = shared
        .account_wxid
        .read()
        .unwrap()
        .clone()
        .context("AI 尚未核对微信账号")?;
    if peer.wxid.is_empty() || row.sender.is_empty() {
        return Ok(());
    }
    store::observe(&shared.data, &account, peer, uid, row, id)
}
pub fn api(shared: &Arc<Shared>, action: &str, params: &Value) -> Result<Value> {
    let c = shared.config.read().unwrap().clone();
    let account = shared
        .account_wxid
        .read()
        .unwrap()
        .clone()
        .context("AI 尚未核对微信账号")?;
    match action {
        "submit_ai_task" => {
            let settings = config::Settings::read(&c.raw["ai"])?;
            if !settings.enabled {
                bail!("AI 尚未启用，请在 GUI 的 AI 模型页配置");
            }
            let profile = settings.profile(params["profile_id"].as_str())?;
            config::key(&shared.data, &profile)?;
            let source =
                store::source(&shared.data, &account, number(&params["source_message_id"]))?;
            if !c.allows(&source.name, source.group) {
                bail!("AI 来源会话已移出白名单");
            }
            if number(&params["user_id"]) != source.uid
                || number(&params["group_id"]) != source.group_id
            {
                bail!("AI 目标或发送成员与来源消息不匹配");
            }
            store::submit(&shared.data, &settings, &profile, Some(&source), params)
        }
        "get_ai_task" => {
            let t = store::get(&shared.data, task_id(params)?)?;
            if t["account"] != account {
                bail!("AI 任务不属于当前账号");
            }
            Ok(t)
        }
        "cancel_ai_task" => {
            let id = task_id(params)?;
            let t = store::get(&shared.data, id)?;
            if t["account"] != account {
                bail!("AI 任务不属于当前账号");
            }
            cancel(&shared.data, id)
        }
        "send_ai_task" => send(shared, &c, &account, params),
        "clear_ai_context" => {
            let source =
                store::source(&shared.data, &account, number(&params["source_message_id"]))?;
            if !c.allows(&source.name, source.group)
                || number(&params["user_id"]) != source.uid
                || number(&params["group_id"]) != source.group_id
            {
                bail!("清理上下文的会话或成员不匹配");
            }
            clear_context(&shared.data, Some(&source.session))
        }
        _ => bail!("未知 AI 动作"),
    }
}
fn send(shared: &Arc<Shared>, c: &Config, account: &str, p: &Value) -> Result<Value> {
    let id = task_id(p)?;
    store::submit_output(&shared.data, id, |task| {
        if task["account"] != account || task["kind"] != "chat" {
            bail!("此 AI 任务不能发到当前微信账号");
        }
        let settings = config::Settings::read(&c.raw["ai"])?;
        if !settings.enabled {
            bail!("AI 已关闭，未提交发送");
        }
        let generated = task["status"] == "generated";
        if !generated
            && !(matches!(
                task["status"].as_str(),
                Some("awaiting_delivery" | "delivered" | "uncertain")
            ) && !task["message_id"].is_null())
        {
            bail!("AI 尚未生成可发送的结果，或任务已取消/过期");
        }
        if task["message_id"].is_null() {
            let input = store::input(&shared.data, id)?;
            if settings.profile(Some(&input.profile.id)).ok().as_ref() != Some(&input.profile) {
                bail!("此任务的模型配置已变化，未继续提交旧结果；请取消后重新生成");
            }
        }
        if crate::config::now() > task["expires"].as_f64().unwrap_or(0.)
            && task["message_id"].is_null()
        {
            bail!("AI 结果已过期，未发送");
        }
        let group = task["group"].as_bool().unwrap_or(false);
        let name = task["session_name"].as_str().context("AI 会话无效")?;
        if !c.allows(name, group) {
            bail!("AI 会话已移出白名单");
        }
        if !crate::outbox::enabled(c) {
            bail!("AI 发送需要启用持久化队列");
        }
        let mut params = json!({"message":[{"type":"text","data":{"text":task["text"]}}],"auto_escape":true,"source_message_id":task["source_message_id"]});
        params[if group { "group_id" } else { "user_id" }] = json!(if group {
            crate::config::group_id(name)
        } else {
            crate::config::contact_id(name)
        });
        let request =
            json!({"action":if group{"send_group_msg"}else{"send_private_msg"},"params":params});
        let receipt = crate::outbox::enqueue_ai(shared, c, &request, id)?;
        Ok(receipt)
    })
}
pub fn probe(c: &Config, p: &Value) -> Result<Value> {
    let settings = config::Settings::read(&c.raw["ai"])?;
    let profile = settings.profile(p["profile_id"].as_str())?;
    config::key(&c.path.parent().unwrap().join("data"), &profile)?;
    let task = store::submit(
        &c.path.parent().unwrap().join("data"),
        &settings,
        &profile,
        None,
        &json!({"text":p["text"].as_str().unwrap_or("请只回复：连接成功"),"request_id":format!("probe-{:016x}",rand::random::<u64>())}),
    )?;
    worker::launch_probe(c, task["task_id"].as_str().unwrap().into())?;
    Ok(task)
}
pub fn validate(c: &Config) -> Result<()> {
    config::Settings::read(&c.raw["ai"])?;
    Ok(())
}
fn task_id(p: &Value) -> Result<&str> {
    p["task_id"]
        .as_str()
        .filter(|s| s.len() <= 80)
        .context("缺少 AI task_id")
}
fn number(v: &Value) -> u64 {
    v.as_u64().or_else(|| v.as_str()?.parse().ok()).unwrap_or(0)
}
