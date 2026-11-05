//! Local, bounded conversation and protocol logs. Authentication and image
//! bytes never enter the readable transcript.
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
pub fn timestamp() -> String {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(time: *mut windows::Win32::Foundation::SYSTEMTIME);
    }
    let mut t = windows::Win32::Foundation::SYSTEMTIME::default();
    unsafe {
        GetLocalTime(&mut t);
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, t.wMilliseconds
    )
}
pub fn redact(value: &Value, token: &str) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let secret = matches!(
                        key.to_lowercase().as_str(),
                        "access_token" | "authorization" | "password" | "web_password" | "cookie"
                    );
                    (
                        key.clone(),
                        if secret {
                            json!("<已隐藏>")
                        } else {
                            redact(value, token)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| redact(v, token)).collect()),
        Value::String(s) => {
            if s.starts_with("base64://") {
                return json!(format!("<图片 base64，{} 字符>", s.len() - 9));
            }
            let s = if token.is_empty() {
                s.clone()
            } else {
                s.replace(token, "<已隐藏>")
            };
            static AUTH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            static MEDIA: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
            let re = AUTH.get_or_init(|| {
                regex::Regex::new(r"(?i)(access_token|authorization|password)=([^&\s]+)").unwrap()
            });
            let s = re.replace_all(&s, "$1=<已隐藏>");
            let media =
                MEDIA.get_or_init(|| regex::Regex::new(r"base64://[A-Za-z0-9+/=]+").unwrap());
            json!(
                media
                    .replace_all(&s, "<图片 base64 已省略>")
                    .chars()
                    .take(32000)
                    .collect::<String>()
            )
        }
        _ => value.clone(),
    }
}
pub(crate) fn source_timestamp(seconds: f64) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn FileTimeToLocalFileTime(input: *const FILETIME, output: *mut FILETIME) -> i32;
        fn FileTimeToSystemTime(input: *const FILETIME, output: *mut SYSTEMTIME) -> i32;
    }
    let ticks = ((seconds + 11_644_473_600.) * 10_000_000.) as u64;
    let input = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut local = FILETIME::default();
    let mut time = SYSTEMTIME::default();
    if unsafe { FileTimeToLocalFileTime(&input, &mut local) } == 0
        || unsafe { FileTimeToSystemTime(&local, &mut time) } == 0
    {
        return timestamp();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
    )
}
pub fn append(root: &Path, kind: &str, detail: &Value, token: &str) {
    let _ = std::fs::create_dir_all(root);
    let path = root.join("events.jsonl");
    if path.metadata().is_ok_and(|m| m.len() > 8 * 1024 * 1024) {
        // Three bounded files keep history available without growing forever.
        let _ = std::fs::remove_file(root.join("events.2.jsonl"));
        let _ = std::fs::rename(root.join("events.1.jsonl"), root.join("events.2.jsonl"));
        let _ = std::fs::rename(&path, root.join("events.1.jsonl"));
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let event = json!({"at":timestamp(),"time":crate::config::now(),"kind":kind,"detail":redact(detail,token)});
        if let Ok(mut bytes) = serde_json::to_vec(&event) {
            bytes.push(b'\n');
            let _ = file.write_all(&bytes);
        }
    }
}
pub fn read(root: &Path, filter: i32) -> String {
    let mut lines = Vec::new();
    for name in ["events.1.jsonl", "events.jsonl"] {
        if let Ok(mut file) = File::open(root.join(name)) {
            let len = file.metadata().map(|m| m.len()).unwrap_or(0);
            let start = len.saturating_sub(1024 * 1024);
            let _ = file.seek(SeekFrom::Start(start));
            let mut bytes = vec![];
            let _ = file.read_to_end(&mut bytes);
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines() {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    let kind = v["kind"].as_str().unwrap_or("");
                    if filter == 312
                        && !matches!(kind, "incoming" | "history" | "outgoing" | "delivery")
                    {
                        continue;
                    }
                    if filter == 313 && !kind.starts_with("protocol") {
                        continue;
                    }
                    if filter == 314 && kind != "runtime" {
                        continue;
                    }
                    lines.push(v);
                }
            }
        }
    }
    let skip = lines.len().saturating_sub(500);
    lines
        .into_iter()
        .skip(skip)
        .map(|v| {
            let at = v["at"].as_str().unwrap_or("");
            let d = &v["detail"];
            let kind = v["kind"].as_str().unwrap_or("");
            if kind == "runtime" {
                return format!("[{at}] 状态　{}", d["text"].as_str().unwrap_or(""));
            }
            if matches!(kind, "incoming" | "history" | "outgoing" | "delivery") {
                let tag = match kind {
                    "incoming" => "收到",
                    "history" => "历史",
                    "outgoing" => "回复请求",
                    _ => "发送结果",
                };
                let at = if kind == "history" {
                    d["source_time"]
                        .as_f64()
                        .map(source_timestamp)
                        .unwrap_or_else(|| at.to_owned())
                } else {
                    at.to_owned()
                };
                let body = if let Some(text) = d["text"].as_str() {
                    text.to_owned()
                } else if let Some(parts) = d["content"].as_array() {
                    parts
                        .iter()
                        .map(|p| {
                            if p["type"] == "text" {
                                p["text"].as_str().unwrap_or("").to_owned()
                            } else if p["type"] == "music" {
                                format!(
                                    "[原生音乐] {} / {}",
                                    p["title"].as_str().unwrap_or(""),
                                    p["artist"].as_str().unwrap_or("")
                                )
                            } else {
                                if p["type"] == "record" {
                                    format!("[原生语音：{} 毫秒]", p["duration_ms"])
                                } else {
                                    format!("[图片：{} 字节]", p["bytes"])
                                }
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\r\n")
                } else if kind == "delivery" {
                    format!(
                        "{}　耗时 {}ms{}",
                        if d["success"] == true {
                            match d["confirmation"].as_str() {
                                Some("native_server") => "原生消息已由微信服务端确认",
                                Some("mixed") => "图文及原生消息已由微信确认",
                                Some("wechat") => "微信操作已确认",
                                _ => "已由微信数据库确认成功",
                            }
                        } else {
                            "失败"
                        },
                        d["elapsed_ms"],
                        d["error"]
                            .as_str()
                            .map(|s| format!("\r\n{s}"))
                            .unwrap_or_default()
                    )
                } else {
                    String::new()
                };
                let id = d
                    .get("message_id")
                    .or_else(|| d.get("source_id"))
                    .map(|v| format!("\r\n消息 ID：{v}"))
                    .unwrap_or_default();
                return format!(
                    "[{at}] {tag}　{}【{}】　{}\r\n{}{id}",
                    if d["group"] == true {
                        "群聊"
                    } else {
                        "私聊"
                    },
                    d["session"].as_str().unwrap_or(""),
                    d["sender"].as_str().unwrap_or(""),
                    body
                );
            }
            format!(
                "[{at}] {}\r\n{}",
                if kind == "protocol_request" {
                    "OB11 调用 →"
                } else {
                    "OB11 回包 ←"
                },
                serde_json::to_string_pretty(d).unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n\r\n")
        .replace('\n', "\r\n")
        .replace("\r\r\n", "\r\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn music_transcript_uses_title_and_server_confirmation() {
        let d = tempfile::tempdir().unwrap();
        append(
            d.path(),
            "outgoing",
            &json!({"session":"测试","content":[{"type":"music","title":"Youngblood","artist":"Artist"}]}),
            "",
        );
        append(
            d.path(),
            "delivery",
            &json!({"session":"测试","success":true,"confirmation":"native_server","elapsed_ms":20}),
            "",
        );
        let text = read(d.path(), 312);
        assert!(text.contains("Youngblood / Artist"));
        assert!(text.contains("原生消息已由微信服务端确认"));
        assert!(!text.contains("数据库确认"));
        assert!(!text.contains("图片：null"));
    }
    #[test]
    fn local_transcript_keeps_case_and_hides_credentials_and_image_bytes() {
        let value = redact(
            &json!({"text":"ccc1\nCCC1","access_token":"secret","file":"base64://AAAA","url":"ws://host?access_token=secret"}),
            "secret",
        );
        assert_eq!(value["text"], "ccc1\nCCC1");
        assert!(!value.to_string().contains("secret"));
        assert!(!value.to_string().contains("AAAA"));
        let d = tempfile::tempdir().unwrap();
        append(
            d.path(),
            "incoming",
            &json!({"session":"测试","text":"ccc1\nCCC1"}),
            "secret",
        );
        let text = read(d.path(), 312);
        assert!(text.contains("ccc1"));
        assert!(text.contains("CCC1"));
        assert!(read(d.path(), 313).is_empty());
    }
}
