use crate::{
    config::{self, Config},
    delivery::{self, Part},
    service::{Client, Shared},
};
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpListener,
    sync::{Semaphore, mpsc},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        self,
        handshake::server::{ErrorResponse, Request, Response},
        protocol::WebSocketConfig,
    },
};

pub struct Job {
    pub name: String,
    pub group: bool,
    pub owner: u64,
    pub id: u64,
    pub parts: Vec<Part>,
    pub operation: crate::actions::Operation,
    pub deadline: Instant,
    pub state: Arc<AtomicU8>,
    pub reply: std::sync::mpsc::SyncSender<std::result::Result<(), String>>,
}
impl Job {
    pub(crate) fn check_submission(&self, shared: &Shared) -> Result<()> {
        if Instant::now() >= self.deadline
            || shared.stop.load(Ordering::SeqCst)
            || !shared.owner(self.owner)
            || !shared.config.read().unwrap().allows(&self.name, self.group)
        {
            bail!("发送提交前已过期、停止、断线或目标已移出白名单；未提交");
        }
        if crate::outbox::cancelled(shared, self.id)? {
            bail!("该次输出已请求撤回或到期，未继续提交");
        }
        Ok(())
    }
}
pub fn failure(echo: Value, code: i64, message: &str) -> Value {
    json!({"status":"failed","retcode":code,"data":null,"echo":echo,"message":message})
}
fn success(echo: Value, data: Value) -> Value {
    json!({"status":"ok","retcode":0,"data":data,"echo":echo})
}
fn same_secret(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    let a = Sha256::digest(a.as_bytes());
    let b = Sha256::digest(b.as_bytes());
    a.iter().zip(b).fold(0u8, |v, (a, b)| v | (a ^ b)) == 0
}
fn auth(req: &Request, c: &Config) -> bool {
    if req.uri().path() != "/onebot/v11" {
        return false;
    }
    let bearer = req
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));
    let query = req.uri().query().and_then(|q| {
        url::form_urlencoded::parse(q.as_bytes())
            .find(|(k, _)| k == "access_token")
            .map(|(_, v)| v.into_owned())
    });
    bearer
        .or(query.as_deref())
        .is_some_and(|s| same_secret(s, c.token()))
}
pub fn serve(shared: Arc<Shared>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let c = shared.config.read().unwrap().clone();
        let listener = TcpListener::bind((c.host(), c.port())).await?;
        let cert = c.r()["tls_certfile"].as_str().unwrap_or("");
        let tls = if cert.is_empty() {
            None
        } else {
            let root = c.path.parent().unwrap();
            let mut certificate = std::io::BufReader::new(std::fs::File::open(root.join(cert))?);
            let certs =
                rustls_pemfile::certs(&mut certificate).collect::<std::io::Result<Vec<_>>>()?;
            let mut private = std::io::BufReader::new(std::fs::File::open(
                root.join(c.r()["tls_keyfile"].as_str().unwrap()),
            )?);
            let key = rustls_pemfile::private_key(&mut private)?
                .ok_or_else(|| anyhow::anyhow!("WSS 私钥不存在"))?;
            let tls = tokio_rustls::rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)?;
            Some(tokio_rustls::TlsAcceptor::from(Arc::new(tls)))
        };
        loop {
            if shared.stop.load(Ordering::Relaxed) {
                break;
            }
            let result = tokio::time::timeout(Duration::from_millis(250), listener.accept()).await;
            if let Ok(Ok((stream, _))) = result {
                let shared = shared.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    let _ = stream.set_nodelay(true);
                    if let Some(tls) = tls {
                        if let Ok(Ok(stream)) =
                            tokio::time::timeout(Duration::from_secs(5), tls.accept(stream)).await
                        {
                            let _ = connection(stream, shared).await;
                        }
                    } else {
                        let _ = connection(stream, shared).await;
                    }
                });
            }
        }
        Ok(())
    })
}
async fn connection<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    shared: Arc<Shared>,
) -> Result<()> {
    let c = shared.config.read().unwrap().clone();
    let id = rand::random::<u64>().max(1);
    struct Lease(Arc<Shared>, u64);
    impl Drop for Lease {
        fn drop(&mut self) {
            let mut client = self.0.client.lock().unwrap();
            if client.as_ref().is_some_and(|c| c.id == self.1) {
                *client = None;
            }
        }
    }
    let _lease = Lease(shared.clone(), id);
    let (tx, mut rx) = mpsc::channel::<Value>(1024);
    let claim = shared.clone();
    let sender = tx.clone();
    let callback =
        move |req: &Request, response: Response| -> std::result::Result<Response, ErrorResponse> {
            if !auth(req, &c) {
                return Err(tungstenite::http::Response::builder()
                    .status(401)
                    .body(Some("Unauthorized".into()))
                    .unwrap());
            }
            let mut client = claim.client.lock().unwrap();
            if client.is_some() {
                return Err(tungstenite::http::Response::builder()
                    .status(409)
                    .body(Some("One client already connected".into()))
                    .unwrap());
            }
            *client = Some(Client { id, tx: sender });
            Ok(response)
        };
    let mut options = WebSocketConfig::default();
    options.max_message_size = Some(16 * 1024 * 1024);
    options.max_frame_size = Some(16 * 1024 * 1024);
    let ws = tokio::time::timeout(
        Duration::from_secs(5),
        accept_hdr_async_with_config(stream, callback, Some(options)),
    )
    .await;
    let ws = match ws {
        Ok(Ok(ws)) => ws,
        _ => {
            let mut client = shared.client.lock().unwrap();
            if client.as_ref().is_some_and(|c| c.id == id) {
                *client = None;
            }
            return Ok(());
        }
    };
    shared.log("RedReply OneBot11 已连接");
    let (mut output, mut input) = ws.split();
    let bot = shared.config.read().unwrap().bot();
    output.send(tungstenite::Message::Text(json!({"time":config::now() as i64,"self_id":bot,"post_type":"meta_event","meta_event_type":"lifecycle","sub_type":"connect"}).to_string().into())).await?;
    // Native WeChat operations share one window. Await a bounded FIFO lane
    // asynchronously instead of rejecting the ninth request or occupying eight
    // blocking threads waiting for that same window. Control APIs stay separate.
    let native_pending = Arc::new(Semaphore::new(128));
    let (native_tx, mut native_rx) =
        mpsc::channel::<(Value, tokio::sync::OwnedSemaphorePermit, Instant)>(128);
    let native_shared = shared.clone();
    let native_responses = tx.clone();
    tokio::spawn(async move {
        while let Some((request, _permit, deadline)) = native_rx.recv().await {
            let echo = request["echo"].clone();
            native_shared.event("runtime",&json!({"text":format!("原生回复出队，已等待 {}ms，后续 {} 项",28000u128.saturating_sub(deadline.saturating_duration_since(Instant::now()).as_millis()),native_rx.len()),"event":"action_dequeued","echo":echo,"queue_depth":native_rx.len()}));
            let result = if !native_shared.owner(id) || Instant::now() >= deadline {
                native_shared.event("protocol_request", &request);
                let result = failure(
                    echo,
                    1408,
                    "Native queue deadline elapsed or connection closed; request was not executed",
                );
                native_shared.event("protocol_response", &result);
                result
            } else {
                let shared = native_shared.clone();
                match tokio::task::spawn_blocking(move || {
                    api_until(&shared, request, id, Some(deadline))
                })
                .await
                {
                    Ok(result) => result,
                    Err(e) => {
                        let result = failure(echo, 1500, &format!("Native API worker failed: {e}"));
                        native_shared.event("protocol_response", &result);
                        result
                    }
                }
            };
            if native_responses.send(result).await.is_err() {
                break;
            }
        }
    });
    let control_pending = Arc::new(Semaphore::new(32));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    heartbeat.tick().await;
    loop {
        tokio::select! {
            result=input.next()=>{match result{
                Some(Ok(tungstenite::Message::Text(text)))=>{let request:Value=match serde_json::from_str(&text){Ok(v)=>v,Err(_)=>{let _=tx.try_send(failure(Value::Null,1400,"Invalid JSON"));continue;}};
                    let action=request["action"].as_str().unwrap_or("");
                    let native=(action.starts_with("send_") && action!="send_ai_task") || action=="delete_msg";
                    let pending=if native {native_pending.clone()} else {control_pending.clone()};
                    let permit=match pending.try_acquire_owned(){Ok(p)=>p,Err(_)=>{
                        let result=failure(request["echo"].clone(),1408,"Bounded request backlog is full; request was not executed");
                        shared.event("protocol_request",&request);shared.event("protocol_response",&result);
                        if tx.try_send(result).is_err() {shared.log("协议回包队列已满；关闭连接，未静默丢弃错误");break;}continue;
                    }};
                    let deadline=Instant::now()+Duration::from_secs(28);
                    if native {
                        shared.event("runtime",&json!({"text":format!("回复进入有界队列，等待或执行中 {} 项",128-native_pending.available_permits()),"event":"action_queued","echo":request["echo"]}));
                        if native_tx.try_send((request,permit,deadline)).is_err() {shared.log("原生任务队列不可用；关闭连接");break;}
                    } else {
                        let response=tx.clone();let shared=shared.clone();
                        tokio::spawn(async move {
                            let _pending=permit;
                            if let Ok(result)=tokio::task::spawn_blocking(move||api_until(&shared,request,id,Some(deadline))).await {
                                let _=response.send(result).await;
                            }
                        });
                    }
                },Some(Ok(tungstenite::Message::Ping(bytes)))=>{if output.send(tungstenite::Message::Pong(bytes)).await.is_err(){break;}},Some(Ok(tungstenite::Message::Close(_)))|None|Some(Err(_))=>break,_=>{}}
            },
            message=rx.recv()=>{let Some(message)=message else{break};if output.send(tungstenite::Message::Text(message.to_string().into())).await.is_err(){break;}},
            _=heartbeat.tick()=>{if shared.stop.load(Ordering::Relaxed){break;}let ready=shared.ready.load(Ordering::SeqCst);if output.send(tungstenite::Message::Text(json!({"time":config::now() as i64,"self_id":bot,"post_type":"meta_event","meta_event_type":"heartbeat","status":{"online":ready,"good":ready},"interval":5000}).to_string().into())).await.is_err(){break;}}
        }
    }
    let mut client = shared.client.lock().unwrap();
    if client.as_ref().is_some_and(|c| c.id == id) {
        *client = None;
    }
    shared.log("RedReply OneBot11 已断开；未推送的数据库消息保留游标");
    Ok(())
}
#[cfg(test)]
pub fn api(shared: &Arc<Shared>, request: Value, owner: u64) -> Value {
    api_until(shared, request, owner, None)
}
fn api_until(
    shared: &Arc<Shared>,
    request: Value,
    owner: u64,
    request_deadline: Option<Instant>,
) -> Value {
    shared.event("protocol_request", &request);
    let mut request = request;
    let translated = crate::compat::rewrite(&mut request);
    let echo = request["echo"].clone();
    let c = shared.config.read().unwrap().clone();
    let deadline = (Instant::now() + Duration::from_secs_f64(c.timeout())).min(
        request_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs_f64(c.timeout())),
    );
    let params = &request["params"];
    let action = request["action"].as_str().unwrap_or("");
    let result = (|| -> Result<Value> {
        translated?;
        match action {
            "submit_ai_task" | "get_ai_task" | "cancel_ai_task" | "send_ai_task"
            | "clear_ai_context" => {
                if !shared.owner(owner) {
                    bail!("Connection closed");
                }
                crate::ai::api(shared, action, params)
            }
            "inject_message" => {
                let text = params["text"].as_str().context("Missing text")?.to_owned();
                let name = params["name"].as_str().unwrap_or("阿飞").to_owned();
                let uid = params["user_id"].as_u64().unwrap_or(197589500722561);
                let id = (config::now() * 1000.) as u64;
                let peer = crate::database::Peer {
                    name: name.clone(),
                    group: false,
                    wxid: String::new(),
                };
                let row = crate::database::Message {
                    local_id: 0,
                    server_id: id as i64,
                    sort_seq: 0,
                    create_time: config::now() as i64,
                    kind: 1,
                    sender: name.clone(),
                    content: text,
                    shard: "inject".into(),
                };
                let published = shared.publish(&peer, &name, uid, &row, id);
                Ok(json!({"published":published,"message_id":id}))
            }
            "get_version_info" => Ok(
                json!({"app_name":"wechat-redreply-rust","app_version":env!("CARGO_PKG_VERSION"),"protocol_version":"v11"}),
            ),
            "get_login_info" => Ok(
                json!({"user_id":c.bot(),"nickname":c.r()["nickname"].as_str().unwrap_or("微信桥接")}),
            ),
            "get_capabilities" => Ok(crate::compat::capabilities()),
            "can_send_image" => Ok(json!({"yes":true})),
            "can_send_record" => Ok(json!({"yes":crate::voice::inspect(&c).is_ok()})),
            "get_stranger_info" => {
                let id = numeric(&params["user_id"]);
                let name = if id == c.bot() {
                    c.r()["nickname"].as_str().unwrap_or("微信桥接").to_owned()
                } else if let Some((name, _)) = c
                    .scope()
                    .into_iter()
                    .find(|(n, g)| !g && config::contact_id(n) == id)
                {
                    name
                } else {
                    shared
                        .members
                        .lock()
                        .unwrap()
                        .values()
                        .find_map(|m| {
                            m.get(&id)
                                .and_then(|v| v["nickname"].as_str())
                                .map(str::to_owned)
                        })
                        .context("未知的微信用户ID")?
                };
                Ok(json!({"user_id":id,"nickname":name,"sex":"unknown","age":0}))
            }
            "get_status" => Ok(
                json!({"online":shared.connected()&&shared.ready.load(Ordering::SeqCst),"good":shared.ready.load(Ordering::SeqCst),"outbox":crate::outbox::summary(shared)?}),
            ),
            "get_delivery_status" => crate::outbox::status(shared, numeric(&params["message_id"]))?
                .context("Unknown durable receipt"),
            "get_friend_list" => Ok(Value::Array(
                c.scope()
                    .iter()
                    .filter(|(_, g)| !*g)
                    .map(|(n, _)| json!({"user_id":config::contact_id(n),"nickname":n,"remark":n}))
                    .collect(),
            )),
            "get_group_list" => Ok(Value::Array(
                c.scope()
                    .iter()
                    .filter(|(_, g)| *g)
                    .map(|(n, _)| json!({"group_id":config::group_id(n),"group_name":n}))
                    .collect(),
            )),
            "get_group_info" | "get_group_member_info" | "get_group_member_list" => {
                let gid = numeric(&params["group_id"]);
                let group = c
                    .scope()
                    .into_iter()
                    .find(|(n, g)| *g && config::group_id(n) == gid)
                    .ok_or_else(|| anyhow::anyhow!("Unknown or non-whitelisted group"))?;
                if action == "get_group_info" {
                    return Ok(
                        json!({"group_id":gid,"group_name":group.0,"member_count":shared.members.lock().unwrap().get(&gid).map_or(0,|m|m.len()),"max_member_count":0}),
                    );
                }
                let members = shared.members.lock().unwrap();
                let list = members.get(&gid);
                if action == "get_group_member_list" {
                    Ok(Value::Array(
                        list.map_or(vec![], |m| m.values().cloned().collect()),
                    ))
                } else {
                    Ok(list.and_then(|m|m.get(&numeric(&params["user_id"]))).cloned().unwrap_or_else(||json!({"group_id":gid,"user_id":numeric(&params["user_id"]),"nickname":"","role":"member"})))
                }
            }
            "begin_response_batch" | "finish_response_batch" => {
                crate::outbox::workflow(shared, &c, &params, action == "begin_response_batch")
            }
            "schedule_delete_msg" => {
                let id = numeric(&params["message_id"]);
                let delay = params["delay_ms"]
                    .as_f64()
                    .or_else(|| params["delay_ms"].as_str().and_then(|s| s.parse().ok()))
                    .unwrap_or(30000.);
                if let Some(v) = crate::outbox::request_recall(shared, id, Some(delay))? {
                    return Ok(v);
                }
                let due = crate::actions::schedule(shared, id, delay)?;
                Ok(
                    json!({"message_id":id,"scheduled_at":due,"limit_seconds":crate::actions::recall_limit(&c.raw)}),
                )
            }
            "delete_msg" => {
                let id = numeric(&params["message_id"]);
                if let Some(v) = crate::outbox::request_recall(shared, id, None)? {
                    return Ok(v);
                }
                let record = crate::actions::ensure_recall(shared, id)?;
                if !record.recalled {
                    queue_action(
                        shared,
                        &c,
                        owner,
                        record.name,
                        record.group,
                        vec![],
                        crate::actions::Operation::Recall(id),
                        deadline,
                    )?;
                }
                Ok(Value::Null)
            }
            "get_forward_msg" => {
                let value = params
                    .get("id")
                    .or_else(|| params.get("message_id"))
                    .unwrap_or(&Value::Null);
                let id = value
                    .as_u64()
                    .or_else(|| {
                        value
                            .as_str()
                            .and_then(|s| s.trim_start_matches("rust-fwd-").parse().ok())
                    })
                    .unwrap_or(0);
                let ledger = shared.ledger.lock().unwrap();
                let nodes = ledger
                    .sent
                    .get(&id)
                    .and_then(|r| r.nodes.clone())
                    .context("Forward message is unavailable")?;
                Ok(json!({"message":nodes,"messages":nodes}))
            }
            "send_private_forward_msg" | "send_group_forward_msg" | "send_forward_msg" => {
                if crate::outbox::enabled(&c) {
                    if !shared.owner(owner) {
                        bail!("Connection closed");
                    }
                    return crate::outbox::enqueue(shared, &c, &request);
                }
                if !shared.ready.load(Ordering::SeqCst) || !shared.owner(owner) {
                    bail!("Wechat backend is not ready");
                }
                let group = (action == "send_group_forward_msg" || action == "send_forward_msg")
                    && (numeric(&params["group_id"]) > 0 || params["message_type"] == "group");
                let uid = numeric(&params[if group { "group_id" } else { "user_id" }]);
                let (name, _) = c
                    .scope()
                    .into_iter()
                    .find(|(n, g)| {
                        *g == group
                            && if group {
                                config::group_id(n)
                            } else {
                                config::contact_id(n)
                            } == uid
                    })
                    .context("Unknown or non-whitelisted forward destination")?;
                let nodes = crate::actions::nodes(
                    params
                        .get("messages")
                        .or_else(|| params.get("message"))
                        .context("Missing forward nodes")?,
                    shared,
                )?;
                let parts = crate::actions::forward_parts(&nodes, shared)?;
                let id = queue_action(
                    shared,
                    &c,
                    owner,
                    name,
                    group,
                    parts,
                    crate::actions::Operation::Forward(nodes),
                    deadline,
                )?;
                Ok(json!({"message_id":id,"forward_id":format!("rust-fwd-{id}")}))
            }
            "send_private_msg" | "send_group_msg" | "send_msg" => {
                if crate::outbox::enabled(&c) {
                    if !shared.owner(owner) {
                        bail!("Connection closed");
                    }
                    return crate::outbox::enqueue(shared, &c, &request);
                }
                if !shared.ready.load(Ordering::SeqCst) || !shared.owner(owner) {
                    bail!("Wechat backend is not ready");
                }
                let group = action == "send_group_msg"
                    || (action == "send_msg"
                        && (params["message_type"] == "group" || numeric(&params["group_id"]) > 0));
                if action == "send_private_msg" && numeric(&params["group_id"]) > 0 {
                    bail!("Temporary group private messages are unsupported");
                }
                let uid = numeric(&params[if group { "group_id" } else { "user_id" }]);
                let (name, _) = c
                    .scope()
                    .into_iter()
                    .find(|(n, g)| {
                        *g == group
                            && if group {
                                config::group_id(n)
                            } else {
                                config::contact_id(n)
                            } == uid
                    })
                    .ok_or_else(|| anyhow::anyhow!("Unknown or non-whitelisted destination"))?;
                let parts =
                    delivery::prepare(&params["message"], params["auto_escape"] == true, shared)?;
                let current = shared.config.read().unwrap();
                if !current.allows(&name, group) {
                    bail!("Destination was removed from whitelist");
                }
                drop(current);
                let id = queue_action(
                    shared,
                    &c,
                    owner,
                    name,
                    group,
                    parts,
                    crate::actions::Operation::Send,
                    deadline,
                )?;
                Ok(json!({"message_id":id}))
            }
            _ => bail!("Unsupported action"),
        }
    })();
    let response = match result {
        Ok(data) => success(echo, data),
        Err(e) => failure(
            echo,
            if action == "delete_msg" {
                1404
            } else if action.starts_with("send_") {
                1500
            } else {
                1400
            },
            &format!("{e}"),
        ),
    };
    shared.event("protocol_response", &response);
    response
}
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
fn next_id() -> u64 {
    NEXT_ID.fetch_max((config::now() * 1000.) as u64, Ordering::SeqCst);
    NEXT_ID.fetch_add(1, Ordering::SeqCst)
}
fn queue_action(
    shared: &Arc<Shared>,
    _c: &Config,
    owner: u64,
    name: String,
    group: bool,
    parts: Vec<Part>,
    operation: crate::actions::Operation,
    deadline: Instant,
) -> Result<u64> {
    if Instant::now() >= deadline {
        bail!("Native request deadline elapsed before execution; request was not sent");
    }
    if !shared.owner(owner) || !shared.ready.load(Ordering::SeqCst) {
        bail!("Wechat backend is not ready");
    }
    let id = next_id();
    let state = Arc::new(AtomicU8::new(0));
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    shared
        .queue
        .try_send(Job {
            name,
            group,
            owner,
            id,
            parts,
            operation,
            deadline,
            state: state.clone(),
            reply: tx,
        })
        .map_err(|_| anyhow::anyhow!("Delivery queue is full"))?;
    match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(())) => Ok(id),
        Ok(Err(e)) => bail!("{e}"),
        Err(_) => {
            let _ = state.compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst);
            bail!("Native action timed out; an action already started may have been attempted");
        }
    }
}
use std::sync::atomic::AtomicU64;
fn numeric(v: &Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_and_scope_are_preserved() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Config::default(d.path().join("config.json"));
        c.raw["whitelist"] = json!(["测试好友"]);
        let (s, _) = Shared::new(c);
        let a = api(&s, json!({"action":"get_friend_list","echo":"abc"}), 1);
        assert_eq!(a["echo"], "abc");
        assert_eq!(a["data"][0]["user_id"], 279515997009546u64);
        assert_eq!(api(&s, json!({"action":"delete_msg"}), 1)["retcode"], 1404);
    }
    #[test]
    fn header_and_query_auth() {
        let c = Config::default(PathBuf::from("config.json"));
        let request = Request::builder()
            .uri(format!("/onebot/v11?access_token={}", c.token()))
            .body(())
            .unwrap();
        assert!(auth(&request, &c));
        assert!(!auth(
            &Request::builder()
                .uri("/onebot/v11?access_token=bad")
                .body(())
                .unwrap(),
            &c
        ));
    }
    #[test]
    fn timed_out_queued_action_cannot_start_later() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Config::default(d.path().join("config.json"));
        c.raw["redreply"]["delivery_timeout_seconds"] = json!(1);
        let (shared, rx) = Shared::new(c.clone());
        shared.ready.store(true, Ordering::SeqCst);
        let result = queue_action(
            &shared,
            &c,
            0,
            "测试".into(),
            false,
            vec![Part::Text("不会发送".into())],
            crate::actions::Operation::Send,
            Instant::now() + Duration::from_secs_f64(c.timeout()),
        );
        assert!(result.is_err());
        let job = rx.try_recv().unwrap();
        assert_eq!(job.state.load(Ordering::SeqCst), 2);
        assert!(
            job.state
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        );
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn real_websocket_auth_echo_concurrency_and_reconnect() {
        use tokio_tungstenite::connect_async;
        let d = tempfile::tempdir().unwrap();
        let c = Config::default(d.path().join("config.json"));
        let token = c.token().to_owned();
        let (shared, _) = Shared::new(c);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_shared = shared.clone();
        let acceptor = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let s = server_shared.clone();
                tokio::spawn(async move {
                    connection(stream, s).await.unwrap();
                });
            }
        });
        assert!(
            connect_async(format!("ws://{addr}/onebot/v11?access_token=bad"))
                .await
                .is_err()
        );
        let url = format!("ws://{addr}/onebot/v11?access_token={token}");
        let (mut ws, _) = connect_async(&url).await.unwrap();
        let first = ws.next().await.unwrap().unwrap().into_text().unwrap();
        assert!(first.contains("lifecycle"));
        assert!(connect_async(&url).await.is_err());
        for id in 0..6 {
            ws.send(tungstenite::Message::Text(
                json!({"action":"get_version_info","echo":{"id":id}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        }
        let mut ids = std::collections::BTreeSet::new();
        while ids.len() < 6 {
            let frame = tokio::time::timeout(Duration::from_secs(3), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let tungstenite::Message::Text(body) = frame {
                let value: Value = serde_json::from_str(&body).unwrap();
                if let Some(id) = value["echo"]["id"].as_u64() {
                    assert_eq!(value["retcode"], 0);
                    ids.insert(id);
                }
            }
        }
        ws.close(None).await.unwrap();
        for _ in 0..100 {
            if !shared.connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!shared.connected());
        let (mut again, _) = connect_async(&url).await.unwrap();
        again.close(None).await.unwrap();
        acceptor.abort();
    }
    use std::path::PathBuf;
    #[test]
    fn durable_receipt_survives_restart_without_a_ready_wechat_window() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Config::default(d.path().join("config.json"));
        c.raw["whitelist"] = json!(["测试好友"]);
        c.raw["durable_queue"] = true.into();
        c.raw["wechat_binding"] = json!({"pid":1,"created_at":1.,"hwnd":1});
        std::fs::create_dir_all(d.path().join("data/db/0123456789abcdef01234567")).unwrap();
        let (s, _) = Shared::new(c.clone());
        let response = api(
            &s,
            json!({"action":"send_private_msg","params":{"user_id":config::contact_id("测试好友"),"message":[{"type":"reply","data":{"id":"500"}},{"type":"text","data":{"text":"持久化回复"}}]},"echo":"persist"}),
            0,
        );
        assert_eq!(
            response["retcode"], 0,
            "durable acceptance must not wait for native sending: {response}"
        );
        assert_eq!(response["data"]["delivery_status"], "queued");
        let id = response["data"]["message_id"].as_u64().unwrap();
        drop(s);
        let db = rusqlite::Connection::open(d.path().join("data/outbox.sqlite3")).unwrap();
        let status: String = db
            .query_row("SELECT status FROM jobs WHERE id=?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "queued");
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn burst_replies_are_queued_and_metadata_is_not_starved() {
        use tokio_tungstenite::connect_async;
        let d = tempfile::tempdir().unwrap();
        let mut c = Config::default(d.path().join("config.json"));
        c.raw["whitelist"] = json!(["测试好友"]);
        c.raw["durable_queue"] = false.into();
        let token = c.token().to_owned();
        let uid = config::contact_id("测试好友");
        let (shared, jobs) = Shared::new(c);
        shared.ready.store(true, Ordering::SeqCst);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let s = shared.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            connection(stream, s).await.unwrap();
        });
        let (mut ws, _) = connect_async(format!("ws://{addr}/onebot/v11?access_token={token}"))
            .await
            .unwrap();
        ws.next().await.unwrap().unwrap();
        let executor = std::thread::spawn(move || {
            // Hold the first native operation while more than eight calls arrive.
            let first = jobs.recv_timeout(Duration::from_secs(3)).unwrap();
            let body = |job: &Job| {
                job.parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<String>()
            };
            let mut order = vec![body(&first)];
            std::thread::sleep(Duration::from_millis(250));
            let _ = first.reply.send(Ok(()));
            for _ in 1..24 {
                let Ok(job) = jobs.recv_timeout(Duration::from_secs(2)) else {
                    break;
                };
                order.push(body(&job));
                std::thread::sleep(Duration::from_millis(5));
                let _ = job.reply.send(Ok(()));
            }
            order
        });
        for id in 0..24 {
            ws.send(tungstenite::Message::Text(json!({"action":"send_private_msg","params":{"user_id":uid,"message":format!("burst{id}")},"echo":id}).to_string().into())).await.unwrap();
        }
        ws.send(tungstenite::Message::Text(
            json!({"action":"get_status","echo":"health"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
        let mut ids = std::collections::BTreeSet::new();
        let mut health = false;
        while ids.len() < 24 || !health {
            let frame = tokio::time::timeout(Duration::from_secs(4), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let tungstenite::Message::Text(body) = frame {
                let v: Value = serde_json::from_str(&body).unwrap();
                if v.get("echo").is_some() {
                    assert_eq!(v["retcode"], 0, "burst request rejected: {}", v);
                    if v["echo"] == "health" {
                        health = true;
                        assert!(ids.len() < 24, "metadata waited for every delivery");
                    } else {
                        ids.insert(v["echo"].as_u64().unwrap());
                    }
                }
            }
        }
        ws.close(None).await.unwrap();
        assert_eq!(
            executor.join().unwrap(),
            (0..24).map(|i| format!("burst{i}")).collect::<Vec<_>>()
        );
        server.await.unwrap();
    }
    #[test]
    fn expired_request_does_not_enter_native_queue() {
        let d = tempfile::tempdir().unwrap();
        let mut c = Config::default(d.path().join("config.json"));
        c.raw["whitelist"] = json!(["测试好友"]);
        c.raw["durable_queue"] = false.into();
        let (shared, jobs) = Shared::new(c);
        shared.ready.store(true, Ordering::SeqCst);
        let v = api_until(
            &shared,
            json!({"action":"send_private_msg","params":{"user_id":config::contact_id("测试好友"),"message":"expired"},"echo":"expired"}),
            0,
            Some(Instant::now() - Duration::from_millis(1)),
        );
        assert_eq!(v["status"], "failed");
        assert!(v["message"].as_str().unwrap().contains("deadline"));
        assert!(jobs.is_empty());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "set REDREPLY_TEST_EXE to an isolated official RedReply executable"]
    async fn official_redreply_private_rules_and_rich_reply() {
        use std::os::windows::process::CommandExt;
        let d = tempfile::tempdir().unwrap();
        let exe = d.path().join("redlang.exe");
        std::fs::copy(
            std::env::var("REDREPLY_TEST_EXE").expect("REDREPLY_TEST_EXE"),
            &exe,
        )
        .unwrap();
        let package = d.path().join("plus_dir/pkg_dir/ExistingRules");
        std::fs::create_dir_all(&package).unwrap();
        let scripts = json!([
            {"name":"已有私聊","content":{"关键词":"ccc1","触发方式":"私聊触发","匹配方式":"完全匹配","code":"小写命中"}},
            {"name":"已有大写","content":{"关键词":"CCC1","触发方式":"私聊触发","匹配方式":"完全匹配","code":"大写命中"}},
            {"name":"已有多输出","content":{"关键词":"多条","触发方式":"私聊触发","匹配方式":"完全匹配","code":"【隐藏【输出流@[CQ:reply,id=【消息ID】]第一条】】[CQ:reply,id=【消息ID】]第二条"}},
            {"name":"已有图文","content":{"关键词":"图文","触发方式":"群、私聊触发","匹配方式":"完全匹配","code":"[CQ:reply,id=1]【艾特】[CQ:image,file=base64://iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=]图文正文"}}
        ]);
        config::atomic(&package.join("script.json"), &scripts).unwrap();
        let mut c = Config::default(d.path().join("adapter.json"));
        c.raw["whitelist"] = json!(["模拟微信好友"]);
        let durable = std::env::var("REDREPLY_TEST_DURABLE").is_ok_and(|s| s == "1");
        c.raw["durable_queue"] = durable.into();
        if durable {
            c.raw["wechat_binding"] = json!({"pid":1,"created_at":1.,"hwnd":1});
            std::fs::create_dir_all(d.path().join("data/db/0123456789abcdef01234567")).unwrap();
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let web = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = web.local_addr().unwrap().port();
        drop(web);
        config::atomic(&d.path().join("plus_dir/config.json"),&json!({"web_port":port,"web_host":"127.0.0.1","web_password":"","readonly_web_password":"","not_open_browser":true,"ws_urls":[format!("ws://{addr}/onebot/v11?access_token={}",c.token())]})).unwrap();
        let (shared, rx) = Shared::new(c);
        shared.ready.store(true, Ordering::SeqCst);
        let server_shared = shared.clone();
        let acceptor = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let shared = server_shared.clone();
                tokio::spawn(async move {
                    let _ = connection(stream, shared).await;
                });
            }
        });
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let _child = Child(
            std::process::Command::new(exe)
                .current_dir(d.path())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x08000000)
                .spawn()
                .unwrap(),
        );
        for _ in 0..600 {
            if shared.connected() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(shared.connected(), "official RedReply did not connect");
        tokio::time::sleep(Duration::from_millis(400)).await;
        let peer = crate::database::Peer {
            name: "模拟微信好友".into(),
            group: false,
            wxid: "fixture".into(),
        };
        for (index, (command, expected)) in [
            ("ccc1", vec!["小写命中"]),
            ("CCC1", vec!["大写命中"]),
            ("多条", vec!["第一条", "第二条"]),
            ("图文", vec!["图文正文"]),
        ]
        .into_iter()
        .enumerate()
        {
            let row = crate::database::Message {
                local_id: index as i64 + 1,
                server_id: index as i64 + 1,
                sort_seq: index as i64 + 1,
                create_time: config::now() as i64,
                kind: 1,
                sender: "friend".into(),
                content: command.into(),
                shard: "fixture".into(),
            };
            assert!(shared.publish(
                &peer,
                &peer.name,
                config::contact_id(&peer.name),
                &row,
                index as u64 + 100
            ));
            for text in expected {
                let parts = if durable {
                    let until = Instant::now() + Duration::from_secs(12);
                    let mut captured = None;
                    loop {
                        if crate::outbox::process_one_with(&shared, |job| {
                            captured = Some(job.parts.clone());
                            Ok(())
                        })
                        .unwrap()
                        {
                            break;
                        }
                        assert!(Instant::now() < until, "official durable output missing");
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    captured.unwrap()
                } else {
                    let receiver = rx.clone();
                    let job = tokio::task::spawn_blocking(move || {
                        receiver.recv_timeout(Duration::from_secs(12))
                    })
                    .await
                    .unwrap()
                    .unwrap();
                    job.reply.send(Ok(())).unwrap();
                    job.parts
                };
                let body = parts
                    .iter()
                    .filter_map(|p| {
                        if let Part::Text(t) = p {
                            Some(t.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<String>();
                assert!(body.contains(text), "unexpected existing rule output");
                if command == "图文" {
                    assert!(parts.iter().any(|p| matches!(p, Part::Image(_))));
                }
            }
            if durable && command == "多条" {
                let db = rusqlite::Connection::open(d.path().join("data/outbox.sqlite3")).unwrap();
                let count:i64=db.query_row("SELECT count(*) FROM jobs j JOIN batches b ON b.batch_id=j.batch_id WHERE b.source='102'",[],|r|r.get(0)).unwrap();
                assert_eq!(count, 2);
            }
        }
        acceptor.abort();
    }
}
