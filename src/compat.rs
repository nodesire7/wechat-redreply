//! Generic OneBot compatibility for the RedReply bridge.
//!
//! These rules are package-agnostic: any package that emits CQ codes inside a
//! text segment, or a `node` object inside a normal send, gets the standard
//! OneBot semantics instead of a movie-specific special case.
use crate::service::Shared;
use serde_json::{Value, json};

fn segments(message: &Value) -> Vec<Value> {
    let raw = match message {
        Value::String(text) => crate::delivery::parse_cq(text),
        Value::Array(items) => items.clone(),
        _ => vec![],
    };
    let mut out = Vec::with_capacity(raw.len());
    for segment in raw {
        let text = (segment["type"] == "text")
            .then(|| segment["data"]["text"].as_str())
            .flatten();
        if let Some(text) = text.filter(|text| text.contains("[CQ:")) {
            out.extend(crate::delivery::parse_cq(text));
        } else {
            out.push(segment);
        }
    }
    out
}

pub fn rewrite(_shared: &Shared, request: &mut Value) {
    let Some(action) = request["action"].as_str() else {
        return;
    };
    if !matches!(action, "send_private_msg" | "send_group_msg" | "send_msg") {
        return;
    }
    let params = request["params"].clone();
    let group =
        action == "send_group_msg" || (action == "send_msg" && params["message_type"] == "group");
    let items = segments(&params["message"]);

    // A node object delivered through a normal send is a merged-forward
    // payload. Promote it to the standard forward action for this target.
    let nodes: Vec<Value> = items
        .iter()
        .filter(|s| s["type"] == "node")
        .cloned()
        .collect();
    if nodes.is_empty() {
        return;
    }
    let batch = params.get("wechat_batch_id").cloned();
    let (action, mut out) = if group {
        (
            "send_group_forward_msg",
            json!({"group_id":params["group_id"].clone(),"messages":nodes}),
        )
    } else {
        (
            "send_private_forward_msg",
            json!({"user_id":params["user_id"].clone(),"messages":nodes}),
        )
    };
    if let Some(batch) = batch {
        out["wechat_batch_id"] = batch;
    }
    *request = json!({"action":action,"params":out});
}
