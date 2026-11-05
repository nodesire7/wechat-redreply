//! Version profiles for the native music CGI channel. The profile is selected
//! by the caller (which already hashed Weixin.dll) and every native RVA is
//! looked up from it, so a new WeChat build only needs a JSON profile.
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

const P4209: &str = include_str!("../../assets/music-cgi-4.2.0.9.json");
const P4212: &str = include_str!("../../assets/music-cgi-4.2.0.12.json");
const VOICE4212: &str = include_str!("../../assets/voice-cgi-4.2.0.12.json");

pub struct VoiceProfile {
    functions: HashMap<String, (usize, [u8; 32])>,
    pub task_vtable: usize,
    pub task_size: usize,
}

pub struct Profile {
    pub version: String,
    functions: HashMap<String, (usize, [u8; 32])>,
    pub manager_anchor: usize,
    pub task_vtable: usize,
    pub voice: Option<VoiceProfile>,
}

fn parse_hex(s: &str) -> Result<usize, String> {
    usize::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| "音乐版本地址无效".into())
}

fn hex32(s: &str) -> Result<[u8; 32], String> {
    if s.len() < 64 {
        return Err("音乐版本字节校验无效".into());
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).map_err(|_| "音乐版本字节校验无效")?;
    }
    Ok(out)
}

pub fn load(version: &str) -> Result<Profile, String> {
    let raw = match version {
        "4.2.0.9" => P4209,
        "4.2.0.12" => P4212,
        _ => return Err("当前微信版本不在音乐原生后端的已核对版本中".into()),
    };
    let v: Value = serde_json::from_str(raw).map_err(|_| "音乐版本资料无效")?;
    let mut functions = HashMap::new();
    for (name, item) in v["functions"].as_object().ok_or("音乐版本函数表无效")? {
        let rva = parse_hex(item["rva"].as_str().ok_or("音乐版本函数地址缺失")?)?;
        let bytes = hex32(
            item["first_32_bytes"]
                .as_str()
                .ok_or("音乐版本函数校验缺失")?,
        )?;
        functions.insert(name.clone(), (rva, bytes));
    }
    Ok(Profile {
        version: version.to_owned(),
        functions,
        manager_anchor: parse_hex(v["manager_anchor"].as_str().ok_or("管理器锚点缺失")?)?,
        task_vtable: parse_hex(v["task_vtable"].as_str().ok_or("任务虚表缺失")?)?,
        voice: if version == "4.2.0.12" {
            Some(load_voice()?)
        } else {
            None
        },
    })
}

fn load_voice() -> Result<VoiceProfile, String> {
    let v: Value = serde_json::from_str(VOICE4212).map_err(|_| "语音版本资料无效")?;
    let mut functions = HashMap::new();
    for (name, item) in v["functions"].as_object().ok_or("语音版本函数表无效")? {
        functions.insert(
            name.clone(),
            (
                parse_hex(item["rva"].as_str().ok_or("语音函数地址缺失")?)?,
                hex32(item["first_32_bytes"].as_str().ok_or("语音函数校验缺失")?)?,
            ),
        );
    }
    Ok(VoiceProfile {
        functions,
        task_vtable: parse_hex(v["task_vtable"].as_str().ok_or("语音任务虚表缺失")?)?,
        task_size: parse_hex(v["task_size"].as_str().ok_or("语音任务大小缺失")?)?,
    })
}

pub fn voice(p: &Profile) -> Result<&VoiceProfile, String> {
    p.voice
        .as_ref()
        .ok_or_else(|| "当前微信版本尚无已核对的原生语音任务".into())
}

pub fn voice_functions(p: &VoiceProfile) -> impl Iterator<Item = (&String, &(usize, [u8; 32]))> {
    p.functions.iter()
}

pub fn voice_rva(p: &VoiceProfile, name: &str) -> Result<usize, String> {
    p.functions
        .get(name)
        .map(|v| v.0)
        .ok_or_else(|| format!("语音版本函数缺失：{name}"))
}

pub fn functions(p: &Profile) -> impl Iterator<Item = (&String, &(usize, [u8; 32]))> {
    p.functions.iter()
}

pub fn rva(p: &Profile, name: &str) -> Result<usize, String> {
    p.functions
        .get(name)
        .map(|v| v.0)
        .ok_or_else(|| format!("音乐版本函数缺失：{name}"))
}

static CURRENT: OnceLock<Profile> = OnceLock::new();

pub fn install(p: Profile) {
    let _ = CURRENT.set(p);
}

pub fn current() -> Result<&'static Profile, String> {
    CURRENT
        .get()
        .ok_or_else(|| "音乐版本资料尚未校验".to_string())
}
