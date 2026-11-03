//! Compatibility listener for the unmodified RedReply/RedLang packages.
//!
//! The original movie package emits the resource list as a plain `send_*_msg`
//! text and only uses `send_group_forward_msg` on Telegram.  This layer keeps
//! the package untouched and promotes that long list to a native merged-forward
//! card, attaching the cover image that was sent just before it.
use crate::service::Shared;
use serde_json::{Value, json};

fn segments(message: &Value) -> Vec<Value> {
    match message {
        Value::String(text) => crate::delivery::parse_cq(text),
        Value::Array(items) => items.clone(),
        _ => vec![],
    }
}

fn target_key(group: bool, p: &Value) -> String {
    if group {
        format!("g{}", p["group_id"])
    } else {
        format!("u{}", p["user_id"])
    }
}

pub fn rewrite(shared: &Shared, request: &mut Value) {
    let Some(action) = request["action"].as_str() else {
        return;
    };
    if !matches!(action, "send_private_msg" | "send_group_msg" | "send_msg") {
        return;
    }
    let params = request["params"].clone();
    let group =
        action == "send_group_msg" || (action == "send_msg" && params["message_type"] == "group");
    let key = target_key(group, &params);
    let now = crate::config::now();
    let items = segments(&params["message"]);

    // Remember the most recent cover image for this target.
    if let Some(image) = items.iter().find(|s| s["type"] == "image").cloned() {
        if let Ok(mut cover) = shared.cover.lock() {
            *cover = Some((key.clone(), now, image));
        }
    }

    let text = items
        .iter()
        .filter(|s| s["type"] == "text")
        .filter_map(|s| s["data"]["text"].as_str())
        .collect::<String>();
    if !(text.contains("资源列表") && text.contains("magnet:")) {
        return;
    }

    let cover = if items.iter().any(|s| s["type"] == "image") {
        None
    } else {
        shared
            .cover
            .lock()
            .ok()
            .and_then(|mut buffer| match buffer.as_ref() {
                Some((stored, at, image)) if stored == &key && now - *at < 90. => {
                    let image = image.clone();
                    *buffer = None;
                    Some(image)
                }
                _ => None,
            })
    };

    let mut content = vec![];
    if let Some(image) = cover {
        content.push(image);
    }
    for segment in items {
        if segment["type"] == "reply" {
            continue;
        }
        content.push(segment);
    }
    if content.is_empty() {
        return;
    }
    let node = json!({
        "type":"node",
        "data":{
            "name":"电影资源",
            "uin":"0",
            "content":content
        }
    });
    let (action, mut params) = if group {
        (
            "send_group_forward_msg",
            json!({"group_id":request["params"]["group_id"].clone(),"messages":[node]}),
        )
    } else {
        (
            "send_private_forward_msg",
            json!({"user_id":request["params"]["user_id"].clone(),"messages":[node]}),
        )
    };
    if let Some(batch) = request["params"].get("wechat_batch_id") {
        params["wechat_batch_id"] = batch.clone();
    }
    *request = json!({"action":action,"params":params});
}
