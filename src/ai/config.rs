use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub allow_http: bool,
    pub require_key: bool,
    pub token_parameter: String,
    pub temperature: Option<f64>,
    pub system_prompt: String,
}
impl Default for Profile {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), provider: "custom".into(),
            base_url: String::new(), model: String::new(), allow_http: false,
            require_key: true, token_parameter: "max_tokens".into(), temperature: None,
            system_prompt: "你是一个可靠的中文助手。根据用户问题回答；不确定时明确说明，不声称已执行未执行的操作。".into() }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub enabled: bool,
    pub active_profile: String,
    pub profiles: Vec<Profile>,
    pub concurrency: usize,
    pub timeout_seconds: u64,
    pub context_turns: usize,
    pub context_minutes: u64,
    pub input_budget: usize,
    pub max_output_tokens: usize,
    pub requests_per_minute: usize,
    pub daily_token_budget: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            active_profile: String::new(),
            profiles: vec![],
            concurrency: 2,
            timeout_seconds: 90,
            context_turns: 6,
            context_minutes: 30,
            input_budget: 8192,
            max_output_tokens: 1024,
            requests_per_minute: 30,
            daily_token_budget: 0,
        }
    }
}
impl Settings {
    pub fn read(raw: &Value) -> Result<Self> {
        let s: Self = if raw.is_null() {
            Self::default()
        } else {
            serde_json::from_value(raw.clone()).context("AI 配置格式无效")?
        };
        s.validate()?;
        Ok(s)
    }
    pub fn profile(&self, id: Option<&str>) -> Result<Profile> {
        let id = id.filter(|v| !v.is_empty()).unwrap_or(&self.active_profile);
        self.profiles
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .context("请选择已保存的 AI 模型配置")
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=8).contains(&self.concurrency)
            || !(5..=180).contains(&self.timeout_seconds)
            || self.context_turns > 30
            || !(1..=1440).contains(&self.context_minutes)
            || !(512..=65536).contains(&self.input_budget)
            || !(64..=8192).contains(&self.max_output_tokens)
            || !(1..=600).contains(&self.requests_per_minute)
            || self.profiles.len() > 20
            || self.daily_token_budget > 1_000_000_000
        {
            bail!(
                "AI 参数超出范围：并发1–8，超时5–180秒，上下文0–30轮，输入预算512–65536，输出64–8192，频率1–600次/分钟"
            );
        }
        let mut ids = HashSet::new();
        for p in &self.profiles {
            if p.id.is_empty()
                || p.id.len() > 64
                || !p
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || !ids.insert(&p.id)
            {
                bail!("AI 配置 ID 必须唯一，且只含字母、数字、横线或下划线");
            }
            if p.name.trim().is_empty()
                || p.name.len() > 120
                || p.model.trim().is_empty()
                || p.model.len() > 200
                || p.system_prompt.len() > 16000
            {
                bail!("请填写模型配置名称、模型 ID；系统提示词最多16000字节");
            }
            endpoint(p)?;
            if !matches!(
                p.token_parameter.as_str(),
                "max_tokens" | "max_completion_tokens"
            ) || p
                .temperature
                .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
            {
                bail!("AI token 字段或 temperature 无效");
            }
            if p.system_prompt.len() + 128 > self.input_budget {
                bail!("系统提示词已超过输入预算，请增加预算或缩短提示词");
            }
        }
        if self.enabled {
            self.profile(None)?;
        }
        if self.daily_token_budget > 0
            && self.daily_token_budget < self.input_budget + self.max_output_tokens
        {
            bail!("每日 token 预算至少应容纳一次输入预算＋输出上限的预留");
        }
        Ok(())
    }
}
pub fn endpoint(p: &Profile) -> Result<url::Url> {
    let mut u = url::Url::parse(p.base_url.trim()).context("模型地址必须是完整的 HTTP(S) URL")?;
    if u.host_str().is_none()
        || !matches!(u.scheme(), "http" | "https")
        || !u.username().is_empty()
        || u.password().is_some()
        || u.query().is_some()
        || u.fragment().is_some()
    {
        bail!("模型地址只允许 HTTP(S)，不能包含账号、密码、查询参数或片段");
    }
    if u.scheme() == "http" && !p.allow_http {
        bail!("HTTP 模型地址需要显式允许明文连接（本地/内网模型）");
    }
    let path = u.path().trim_end_matches('/').to_owned();
    if !path.ends_with("/chat/completions") {
        u.set_path(&format!("{path}/chat/completions"));
    } else {
        u.set_path(&path);
    }
    Ok(u)
}
pub fn keys(data: &Path) -> Result<Value> {
    let path = data.join("ai-keys.json");
    if !path.exists() {
        return Ok(json!({}));
    }
    let v = crate::config::load(&path).context("AI 密钥文件无法读取；请在 AI 设置重新保存")?;
    if !v.is_object() {
        bail!("AI 密钥文件格式无效");
    }
    Ok(v)
}
pub fn key(data: &Path, p: &Profile) -> Result<String> {
    let k = keys(data)?[&p.id].as_str().unwrap_or("").to_owned();
    if p.require_key && k.trim().is_empty() {
        bail!("此模型尚未设置 API Key");
    }
    Ok(k)
}
pub fn public(c: &crate::config::Config) -> Result<Value> {
    let s = Settings::read(&c.raw["ai"])?;
    let k = keys(&c.path.parent().unwrap().join("data"))?;
    let mut v = serde_json::to_value(s)?;
    for p in v["profiles"].as_array_mut().unwrap() {
        p["has_key"] = json!(
            k[p["id"].as_str().unwrap_or("")]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
    }
    Ok(v)
}
/// Keys are write-only through the GUI. Blank means preserve; deletion is explicit.
pub fn prepare_save(c: &crate::config::Config, value: &Value) -> Result<(Settings, Value)> {
    let data = c.path.parent().unwrap().join("data");
    let mut k = keys(&data)?;
    let mut clean = value.clone();
    for p in clean["profiles"]
        .as_array_mut()
        .context("缺少模型配置列表")?
    {
        let id = p["id"].as_str().context("缺少模型配置 ID")?.to_owned();
        let map = p.as_object_mut().context("模型配置格式无效")?;
        map.remove("has_key");
        if map.remove("clear_key").and_then(|v| v.as_bool()) == Some(true) {
            k.as_object_mut().unwrap().remove(&id);
        }
        if let Some(v) = map.remove("api_key") {
            let text = v.as_str().context("API Key 格式无效")?.trim();
            if text.len() > 8192 || text.contains(['\r', '\n']) {
                bail!("API Key 格式无效");
            }
            if !text.is_empty() {
                k[&id] = text.into();
            }
        }
    }
    let s = Settings::read(&clean)?;
    k.as_object_mut()
        .unwrap()
        .retain(|id, _| s.profiles.iter().any(|p| &p.id == id));
    if s.enabled {
        let p = s.profile(None)?;
        if p.require_key && k[&p.id].as_str().unwrap_or("").is_empty() {
            bail!("当前模型需要 API Key");
        }
    }
    Ok((s, k))
}
