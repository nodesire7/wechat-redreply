//! Translate RedLang/OneBot wire formats before either delivery backend.
//! No package-specific routing, guessed conversations or trigger-wide recalls.
use anyhow::{Result, bail};
use serde_json::{Value, json};

pub fn segments(message: &Value, auto_escape: bool) -> Result<Vec<Value>> {
    let raw = match message {
        Value::String(text) if auto_escape => vec![json!({"type":"text","data":{"text":text}})],
        Value::String(text) => crate::delivery::parse_cq(text),
        Value::Array(items) => items.clone(),
        _ => bail!("message需要文字或消息段数组"),
    };
    let mut out = Vec::with_capacity(raw.len());
    for segment in raw {
        if !auto_escape
            && segment["type"] == "text"
            && let Some(text) = segment["data"]["text"]
                .as_str()
                .filter(|s| s.contains("[CQ:"))
        {
            out.extend(crate::delivery::parse_cq(text));
        } else {
            out.push(segment);
        }
    }
    Ok(out)
}
fn positive(value: &Value) -> bool {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .is_some_and(|n| n > 0)
}
/// Preserve echo and all tracing metadata, changing only protocol fields.
pub fn rewrite(request: &mut Value) -> Result<()> {
    let action = request["action"].as_str().unwrap_or("").to_owned();
    let normal = matches!(
        action.as_str(),
        "send_private_msg" | "send_group_msg" | "send_msg"
    );
    let forward = matches!(
        action.as_str(),
        "send_private_forward_msg" | "send_group_forward_msg" | "send_forward_msg"
    );
    if !normal && !forward {
        return Ok(());
    }
    let params = &mut request["params"];
    let group = match action.as_str() {
        "send_private_msg" | "send_private_forward_msg" => false,
        "send_group_msg" => true,
        "send_group_forward_msg"
            if !positive(&params["group_id"]) && positive(&params["user_id"]) =>
        {
            false
        }
        "send_group_forward_msg" => true,
        _ => match params["message_type"].as_str() {
            Some("private") => false,
            Some("group") => true,
            Some(_) => bail!("message_type须为private或group"),
            None => positive(&params["group_id"]),
        },
    };
    if !positive(&params[if group { "group_id" } else { "user_id" }]) {
        bail!("发送需要明确的目标ID，未猜测最近会话");
    }
    if normal && !group && positive(&params["group_id"]) && action == "send_private_msg" {
        bail!("临时群私聊不受支持");
    }
    let mut is_forward = forward;
    if normal {
        let items = segments(&params["message"], params["auto_escape"] == true)?;
        if items.iter().any(|s| s["type"] == "node") {
            // Preserve text/images surrounding nodes, in their original order.
            let mut nodes = vec![];
            let mut loose = vec![];
            for segment in items {
                if segment["type"] == "node" {
                    if !loose.is_empty() {
                        nodes.push(json!({"type":"node","data":{"name":"消息","content":std::mem::take(&mut loose)}}));
                    }
                    nodes.push(segment);
                } else {
                    loose.push(segment);
                }
            }
            if !loose.is_empty() {
                nodes.push(json!({"type":"node","data":{"name":"消息","content":loose}}));
            }
            params["messages"] = nodes.into();
            params.as_object_mut().unwrap().remove("message");
            is_forward = true;
        } else {
            params["message"] = items.into();
        }
    }
    request["action"] = match (group, is_forward) {
        (true, true) => "send_group_forward_msg",
        (false, true) => "send_private_forward_msg",
        (true, false) => "send_group_msg",
        (false, false) => "send_private_msg",
    }
    .into();
    Ok(())
}
pub fn capabilities() -> Value {
    json!({"protocol":"onebot11","recall_scope":"output","recall_limit_seconds":120,
        "actions":["send_msg","send_private_msg","send_group_msg","send_forward_msg","send_private_forward_msg","send_group_forward_msg","get_forward_msg","delete_msg","schedule_delete_msg","get_login_info","get_friend_list","get_group_list","get_group_info","get_group_member_info","get_group_member_list","get_stranger_info","get_status","get_version_info","get_delivery_status"],
        "segments":{"text":"native","image":"native_separate_message","music":"native_type3_qq","record":"native_voice_4.2.0.12","node":"native_type19","at":"readable_text","reply":"trace_only"},
        "voice":{"native_profiles":["4.2.0.12"],"note_limit_seconds":60,"source_limit_seconds":600,"long_source":"split_into_native_notes_same_output","formats":["SILK","MP3","WAV","FLAC","AAC/M4A","Ogg/Vorbis"]},
        "unsupported_segments":["video","file","face","location","share","contact","poke","tts","non_music_json_xml","music_or_voice_forward_node"],
        "unsupported_actions":["get_msg","get_record","get_image","set_group_ban","set_group_kick","set_group_admin","set_essence_msg","set_msg_emoji_like","send_like","upload_group_file","set_friend_add_request","set_group_add_request"],
        "mappings":[
            {"operation":"文字 / 图片","result":"微信独立消息，同一次输出统一撤回","state":"支持"},
            {"operation":"音乐卡 music / QQ音乐JSON","result":"本次音频的原生音乐卡，直链发送前检查","state":"支持"},
            {"operation":"语音 record","result":"微信原生语音，单条60秒；长音频自动分段。目前4.2.0.12","state":"支持（指定版本）"},
            {"operation":"合并转发 node","result":"本人自聊中转的原生合并卡；音乐/语音节点暂不支持","state":"部分支持"},
            {"operation":"艾特 / 引用","result":"艾特显示正文，引用仅作来源追踪","state":"转译"},
            {"operation":"视频 / 文件 / 其他卡片","result":"尚无对应发送后端，明确返回不支持","state":"未支持"},
            {"operation":"QQ管理动作 / 消息媒体读取","result":"禁言、精华、管理员、get_msg等尚未对齐","state":"未支持"}
        ],
        "music_audio":"caller_supplied_anonymously_accessible_http_audio"})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn group_route_trace_echo_and_mixed_node_content_survive_translation() {
        let mut q = json!({"action":"send_msg","echo":"keep","params":{"group_id":"123","trigger_id":"456","source_message_id":456,"wechat_batch_id":"flow","message":[{"type":"text","data":{"text":"prefix"}},{"type":"node","data":{"content":"body"}},{"type":"image","data":{"file":"base64://fixture"}}]}});
        rewrite(&mut q).unwrap();
        assert_eq!(q["action"], "send_group_forward_msg");
        assert_eq!(q["echo"], "keep");
        assert_eq!(q["params"]["trigger_id"], "456");
        assert_eq!(q["params"]["messages"].as_array().unwrap().len(), 3);
        let original = q.clone();
        rewrite(&mut q).unwrap();
        assert_eq!(q, original);
    }
    #[test]
    fn auto_escape_does_not_promote_literal_cq_nodes_or_images() {
        let mut q = json!({"action":"send_msg","params":{"user_id":123,"auto_escape":true,"message":"[CQ:node,id=1][CQ:image,file=x]"}});
        rewrite(&mut q).unwrap();
        assert_eq!(q["action"], "send_private_msg");
        assert_eq!(q["params"]["message"][0]["type"], "text");
    }
    #[test]
    fn explicit_private_forward_works_without_guessing_an_active_target() {
        let mut q = json!({"action":"send_group_forward_msg","params":{"group_id":"","user_id":"123","messages":[]}});
        rewrite(&mut q).unwrap();
        assert_eq!(q["action"], "send_private_forward_msg");
        q["params"]["user_id"] = Value::Null;
        assert!(rewrite(&mut q).is_err());
    }
}
