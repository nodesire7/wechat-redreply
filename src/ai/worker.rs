use super::{
    config::{self, Profile, Settings},
    store,
};
use crate::service::Shared;
use anyhow::{Context, Result, bail};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub fn run(shared: Arc<Shared>) -> Result<()> {
    store::recover(&shared.data)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(async {
        let mut jobs=tokio::task::JoinSet::new();
        let mut pruned=Instant::now();
        while !shared.stop.load(Ordering::Relaxed) {
            while jobs.try_join_next().is_some() {}
            let c=shared.config.read().unwrap().clone();
            let settings=Settings::read(&c.raw["ai"])?;
            for (id,receipt) in store::receipts(&shared.data)? {
                if let Some(result)=crate::outbox::status(&shared,receipt)? {
                    store::delivery(&shared.data,&id,result["delivery_status"].as_str().unwrap_or(""))?;
                }
            }
            let pending=store::pending(&shared.data)?;
            if settings.enabled {
                for id in pending {
                    if jobs.len()>=settings.concurrency { break; }
                    if store::claim(&shared.data,&id,&settings)? {
                        let shared=shared.clone();
                        jobs.spawn(async move {
                            let result=std::panic::AssertUnwindSafe(execute(&shared.data,&id,Some(&shared))).catch_unwind().await.unwrap_or_else(|_|Err(anyhow::anyhow!("AI 任务执行异常；未自动重试或提交发送")));
                            if let Err(e)=store::finish(&shared.data,&id,result) { shared.log(&format!("保存 AI 任务状态失败：{e}")); }
                        });
                    }
                }
            } else {
                store::open(&shared.data)?.execute("UPDATE tasks SET status='cancelled',error='AI 已关闭，未继续执行',finished=?1 WHERE kind='chat' AND status IN ('queued','running','generated')",[crate::config::now()])?;
            }
            if pruned.elapsed()>=Duration::from_secs(60) { store::prune(&shared.data,settings.context_minutes)?; pruned=Instant::now(); }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        jobs.abort_all();
        while jobs.join_next().await.is_some() {}
        store::recover(&shared.data)?;
        Ok(())
    })
}
/// A GUI test is a real, asynchronous model call, never a WeChat send. It also
/// works with the listener stopped and shares persisted limits with it.
pub fn launch_probe(c: &crate::config::Config, id: String) -> Result<()> {
    static PROBES: AtomicUsize = AtomicUsize::new(0);
    if PROBES
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < 8).then_some(n + 1)
        })
        .is_err()
    {
        store::open(&c.path.parent().unwrap().join("data"))?.execute("UPDATE tasks SET status='failed',finished=?2,error='已有8项模型测试，请等待或取消后再试；未调用模型' WHERE id=?1 AND status='queued'",rusqlite::params![id,crate::config::now()])?;
        bail!("已有8项未结束的模型测试，请等待或取消后再试");
    }
    struct ProbeGuard;
    impl Drop for ProbeGuard {
        fn drop(&mut self) {
            PROBES.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let guard = ProbeGuard;
    let data = c.path.parent().unwrap().join("data");
    let path = c.path.clone();
    std::thread::Builder::new().name("ai-model-test".into()).spawn(move || {
        let _guard=guard;
        let result=(|| -> Result<()> {
            let rt=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
            rt.block_on(async {
                loop {
                    let current=crate::config::Config::read(path.clone())?;
                    let settings=Settings::read(&current.raw["ai"])?;
                    let task=store::get(&data,&id)?;
                    if task["status"]!="queued" { return Ok(()); }
                    if crate::config::now()>=task["deadline"].as_f64().unwrap_or(0.) {
                        store::open(&data)?.execute("UPDATE tasks SET status='expired',error='测试等待已超时，未调用模型',finished=?2 WHERE id=?1 AND status='queued'",rusqlite::params![id,crate::config::now()])?;
                        return Ok(());
                    }
                    if store::claim(&data,&id,&settings)? {
                        let result=execute_at(&data,&id,None,Some(&path)).await;
                        store::finish(&data,&id,result)?;
                        return Ok(());
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            })
        })();
        if result.is_err() {
            // Do not print URLs, HTTP bodies, user prompts or credentials.
            let _=store::open(&data).and_then(|db| {db.execute("UPDATE tasks SET status='failed',error='测试模块无法执行，请检查配置和本地数据文件',finished=?2 WHERE id=?1 AND status IN ('queued','running')",rusqlite::params![id,crate::config::now()])?;Ok(())});
        }
    }).context("无法启动 AI 测试任务")?;
    Ok(())
}
async fn execute(data: &Path, id: &str, shared: Option<&Shared>) -> Result<(String, Value)> {
    execute_at(data, id, shared, None).await
}
async fn execute_at(
    data: &Path,
    id: &str,
    shared: Option<&Shared>,
    config_path: Option<&PathBuf>,
) -> Result<(String, Value)> {
    let input = store::input(data, id)?;
    let task = store::get(data, id)?;
    let remaining = task["deadline"].as_f64().unwrap_or(0.) - crate::config::now();
    if remaining <= 0. {
        bail!("AI 整轮时间已耗尽，未继续请求");
    }
    let messages = store::messages(data, id, &input)?;
    check_current(&input, shared, config_path)?;
    let key = config::key(data, &input.profile)?;
    let call = complete(
        &input.profile,
        &key,
        messages,
        input.settings.max_output_tokens,
        Duration::from_secs_f64(remaining),
    );
    tokio::pin!(call);
    let mut check = tokio::time::interval(Duration::from_millis(200));
    let deadline = tokio::time::sleep(Duration::from_secs_f64(remaining));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result=&mut call => return result,
            _=&mut deadline => bail!("AI 整轮超时；未自动重试或发送，模型可能已产生用量"),
            _=check.tick() => {
                let db=store::open(data)?;
                let status:String=db.query_row("SELECT status FROM tasks WHERE id=?1",[id],|r|r.get(0))?;
                if status!="running" { bail!("AI 任务已取消或结束"); }
                check_current(&input,shared,config_path)?;
            }
        }
    }
}
fn check_current(
    input: &store::Input,
    shared: Option<&Shared>,
    config_path: Option<&PathBuf>,
) -> Result<()> {
    let settings = if let Some(shared) = shared {
        if shared.stop.load(Ordering::SeqCst) {
            bail!("监听停止，AI 调用已中止；未自动重试");
        }
        let c = shared.config.read().unwrap().clone();
        if let Some(source) = &input.source {
            if shared.account_wxid.read().unwrap().as_deref() != Some(&source.account)
                || !c.allows(&source.name, source.group)
            {
                bail!("微信账号或白名单已变化，AI 任务已停止");
            }
        }
        let settings = Settings::read(&c.raw["ai"])?;
        if !settings.enabled {
            bail!("AI 已关闭，任务已停止");
        }
        Some(settings)
    } else if let Some(path) = config_path {
        Some(Settings::read(
            &crate::config::Config::read(path.clone())?.raw["ai"],
        )?)
    } else {
        None
    };
    if let Some(settings) = settings {
        if settings.profile(Some(&input.profile.id)).ok().as_ref() != Some(&input.profile) {
            bail!("模型配置已变化，旧任务未继续请求；请重新提交");
        }
    }
    Ok(())
}
pub(super) async fn complete(
    p: &Profile,
    key: &str,
    messages: Vec<Value>,
    max_output: usize,
    timeout: Duration,
) -> Result<(String, Value)> {
    let url = config::endpoint(p)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10).min(timeout))
        .timeout(timeout)
        .build()
        .map_err(|_| anyhow::anyhow!("无法初始化 AI HTTP 客户端"))?;
    let mut body = json!({"model":p.model,"messages":messages,"stream":false});
    body[&p.token_parameter] = max_output.into();
    if let Some(v) = p.temperature {
        body["temperature"] = json!(v);
    }
    let mut req = client
        .post(url)
        .header("Content-Type", "application/json")
        .body(body.to_string());
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let mut response = req.send().await.map_err(|e| {
        if e.is_timeout() {
            anyhow::anyhow!("模型连接或响应超时；未自动重试")
        } else {
            anyhow::anyhow!("模型网络或 TLS 连接失败；请检查地址、证书和网络")
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "模型 HTTP {}：{}",
            status.as_u16(),
            match status.as_u16() {
                301..=399 => "不跟随重定向，请填写最终接口地址，避免向其他地址转发密钥",
                401 | 403 => "认证或模型权限失败，请检查 Key 和账号权限",
                404 => "接口路径或模型不存在，请检查 base URL 和模型 ID",
                429 => "供应商限流或额度不足；未自动重试或切换供应商",
                400 | 422 => "请求参数或模型协议不兼容；可调整输出 token 字段或移除 temperature",
                _ => "供应商请求失败；未回传可能含凭据或正文的响应体",
            }
        );
    }
    const MAX: usize = 2 * 1024 * 1024;
    if response.content_length().is_some_and(|n| n > MAX as u64) {
        bail!("模型响应超过2MB上限");
    }
    let mut bytes = vec![];
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("模型响应中断；未发送部分文本"))?
    {
        if bytes.len() + chunk.len() > MAX {
            bail!("模型响应超过2MB上限");
        }
        bytes.extend_from_slice(&chunk);
    }
    decode(&bytes)
}
pub(super) fn decode(bytes: &[u8]) -> Result<(String, Value)> {
    let v: Value = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("模型未返回有效的 Chat Completions JSON；请检查接口类型"))?;
    let choice = &v["choices"][0];
    if choice["finish_reason"] == "length" {
        bail!("模型输出被 token 上限截断，未发送残缺回答；请增加输出上限");
    }
    if choice["message"]["tool_calls"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
        || choice["finish_reason"] == "tool_calls"
    {
        bail!("本期只支持文字回答，不执行模型工具调用");
    }
    let content = &choice["message"]["content"];
    let text = if let Some(s) = content.as_str() {
        s.to_owned()
    } else if let Some(a) = content.as_array() {
        a.iter()
            .filter(|x| x["type"] == "text")
            .filter_map(|x| x["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    let text = text.trim().to_owned();
    if text.is_empty() {
        bail!("模型未返回可见回答（仅 reasoning、拒答或空内容），未发送");
    }
    if text.len() > 64000 {
        bail!("模型回答超过64000字节，未发送");
    }
    let usage = &v["usage"];
    let clean = json!({"prompt_tokens":usage["prompt_tokens"].as_u64(),"completion_tokens":usage["completion_tokens"].as_u64(),"total_tokens":usage["total_tokens"].as_u64(),"cached_tokens":usage["prompt_tokens_details"]["cached_tokens"].as_u64().or_else(||usage["prompt_cache_hit_tokens"].as_u64()),"reasoning_tokens":usage["completion_tokens_details"]["reasoning_tokens"].as_u64()});
    Ok((text, clean))
}
