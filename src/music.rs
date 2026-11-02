//! Music format parsing and a bounded cache of verified native references.
//! Parse QQ/OneBot metadata and build native type3 payloads with the observed provider identity.
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::fmt;

#[derive(Clone, Debug)]
pub struct NativeCard {
    pub card: MusicCard,
    pub peer: crate::database::Peer,
    pub source_key: String,
}
#[derive(Clone, Debug)]
pub struct OutgoingMusic {
    pub card: MusicCard,
    pub app_id: String,
}
pub fn outgoing(request: &MusicRequest, shared: &crate::service::Shared) -> Result<OutgoingMusic> {
    let card = match request {
        MusicRequest::Card(c) => c.clone(),
        MusicRequest::Catalog { .. } => resolve_cached(request, shared)?.card,
    };
    if card.provider != Provider::Qq || card.audio_url.is_none() {
        bail!("QQ音乐转换需要曲目标题、歌手、页面和播放地址")
    }
    // Public provider identifier observed in the supplied reference, not an
    // account credential. No source audio/CDN keys or lyric fields are embedded.
    let app_id = include_str!("../assets/qq-music-app-id.txt")
        .trim()
        .to_owned();
    native_appmsg(&card, &app_id)?;
    Ok(OutgoingMusic { card, app_id })
}

pub(crate) fn native_app_id(xml: &str) -> Option<String> {
    use quick_xml::{Reader, events::Event};
    let mut r = Reader::from_str(xml);
    let mut depth = 0;
    for _ in 0..10000 {
        match r.read_event().ok()? {
            Event::Start(e) => {
                depth += 1;
                if depth == 2 && e.name().as_ref() == b"appmsg" {
                    for a in e.attributes() {
                        let a = a.ok()?;
                        if a.key.as_ref() == b"appid" {
                            let s = a.decode_and_unescape_value(r.decoder()).ok()?.into_owned();
                            return (s.len() == 18
                                && s.starts_with("wx")
                                && s[2..].bytes().all(|b| b.is_ascii_hexdigit()))
                            .then_some(s);
                        }
                    }
                    return None;
                }
            }
            Event::End(_) => depth -= 1,
            Event::Eof => return None,
            _ => (),
        }
    }
    None
}

/// Create the actual type3 payload. Media and identity belong to this request;
/// signed thumbnail keys/lyrics/source-account fields from an old song are never copied.
pub fn native_appmsg(card: &MusicCard, app_id: &str) -> Result<String> {
    if card.provider != Provider::Qq
        || app_id.len() != 18
        || !app_id.starts_with("wx")
        || !app_id[2..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        bail!("尚无该音乐来源的已验证微信应用标识")
    }
    let audio = card
        .audio_url
        .as_deref()
        .context("音乐请求缺少真实播放地址")?;
    use quick_xml::escape::escape;
    let mut out = format!("<appmsg appid=\"{app_id}\" sdkver=\"0\">");
    for (name, text) in [
        ("title", card.title.as_str()),
        ("des", card.artist.as_str()),
        ("action", "view"),
        ("type", "3"),
        ("showtype", "0"),
        ("content", ""),
        ("url", card.page_url.as_str()),
        ("dataurl", audio),
        ("lowurl", ""),
        ("lowdataurl", ""),
        ("thumburl", card.image_url.as_deref().unwrap_or("")),
        ("songalbumurl", card.image_url.as_deref().unwrap_or("")),
    ] {
        out.push_str(&format!("<{name}>{}</{name}>", escape(text)));
    }
    out.push_str("<appattach/><extinfo/></appmsg>");
    Ok(out)
}

/// Parse the outer appmsg only. Quoted cards inside type57 and merged type19
/// records must never be mistaken for a forwardable native music message.
pub fn parse_native(xml: &str) -> Result<Option<MusicCard>> {
    use quick_xml::{Reader, events::Event};
    if xml.len() > MAX_CARD_BYTES {
        bail!("微信音乐XML过大");
    }
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut values = std::collections::HashMap::<String, String>::new();
    let mut started = std::collections::HashSet::new();
    let mut roots = 0;
    for _ in 0..10000 {
        let text = match reader.read_event()? {
            Event::Start(e) => {
                if stack.is_empty() {
                    roots += 1;
                }
                stack.push(std::str::from_utf8(e.name().as_ref())?.to_owned());
                if stack.len() > 32 {
                    bail!("微信音乐XML嵌套过深");
                }
                let path = stack.join("/");
                if matches!(
                    path.as_str(),
                    "msg/appmsg"
                        | "msg/appmsg/type"
                        | "msg/appmsg/title"
                        | "msg/appmsg/des"
                        | "msg/appmsg/url"
                        | "msg/appmsg/dataurl"
                        | "msg/appmsg/songalbumurl"
                ) && !started.insert(path)
                {
                    bail!("微信音乐XML字段不唯一");
                }
                continue;
            }
            Event::Empty(_) => continue,
            Event::End(_) => {
                stack.pop();
                continue;
            }
            Event::Text(e) => e.xml_content()?.into_owned(),
            Event::CData(e) => e.xml_content()?.into_owned(),
            Event::GeneralRef(e) => {
                quick_xml::escape::unescape(&format!("&{};", e.decode()?))?.into_owned()
            }
            Event::DocType(_) => bail!("微信音乐XML不允许DTD"),
            Event::Eof => {
                if roots != 1 || !stack.is_empty() {
                    bail!("微信音乐XML结构无效");
                }
                if values.get("msg/appmsg/type").map(String::as_str) != Some("3") {
                    return Ok(None);
                }
                let field = |s: &str| {
                    values
                        .get(&format!("msg/appmsg/{s}"))
                        .cloned()
                        .unwrap_or_default()
                };
                return MusicCard::new(
                    Provider::Unknown,
                    field("title"),
                    field("des"),
                    field("url"),
                    field("dataurl"),
                    field("songalbumurl"),
                )
                .map(Some);
            }
            _ => continue,
        };
        values.entry(stack.join("/")).or_default().push_str(&text);
    }
    bail!("微信音乐XML节点过多")
}

pub fn remember(
    shared: &crate::service::Shared,
    peer: &crate::database::Peer,
    row: &crate::database::Message,
) {
    if row.kind != 49 || row.server_id <= 0 {
        return;
    }
    let Ok(Some(card)) = parse_native(&row.content) else {
        return;
    };
    let mut cache = shared.music_cache.write().unwrap();
    let key = row.key();
    if cache
        .iter()
        .any(|c| c.peer.wxid == peer.wxid && c.source_key == key)
    {
        return;
    }
    cache.push(NativeCard {
        card,
        peer: peer.clone(),
        source_key: key,
    });
    if cache.len() > 512 {
        cache.remove(0);
    }
}

pub fn resolve_cached(
    request: &MusicRequest,
    shared: &crate::service::Shared,
) -> Result<NativeCard> {
    let cache = shared.music_cache.read().unwrap();
    let found = cache
        .iter()
        .rev()
        .find(|c| request.matches_cached(&c.card))
        .ok_or_else(|| {
            anyhow::anyhow!("仅歌曲ID尚无对应的完整元数据；请提供url、audio、title等音乐卡字段")
        })?;
    // UIA omits the source message ID. Refuse a visually identical card when
    // that source conversation contains a different recording with this label.
    if cache.iter().any(|c| {
        c.peer.wxid == found.peer.wxid
            && c.card.title == found.card.title
            && c.card.artist == found.card.artist
            && !request.matches_cached(&c.card)
    }) {
        bail!("源会话存在同名不同版本音乐卡，无法可靠定位，未转发");
    }
    Ok(found.clone())
}

const MAX_CARD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Qq,
    Netease,
    Xiami,
    Unknown,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MusicCard {
    pub provider: Provider,
    pub title: String,
    pub artist: String,
    /// Retain case, escaping and signed query parameters exactly as supplied.
    pub page_url: String,
    pub audio_url: Option<String>,
    pub image_url: Option<String>,
}

// Signed URLs must not appear merely because a caller logs a parsed request.
impl fmt::Debug for MusicCard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MusicCard")
            .field("provider", &self.provider)
            .field("title", &self.title)
            .field("artist", &self.artist)
            .field("has_audio", &self.audio_url.is_some())
            .field("has_image", &self.image_url.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MusicRequest {
    Catalog { provider: Provider, id: String },
    Card(MusicCard),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrackKey {
    QqSongMid(String),
    QqSongId(String),
    NeteaseSongId(String),
}

fn string(value: &Value, key: &str) -> Result<String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        _ => bail!("音乐卡片字段类型错误: {key}"),
    }
}

fn first_string(value: &Value, keys: &[&str]) -> String {
    for key in keys {
        if let Some(Value::String(s)) = value.get(*key)
            && !s.is_empty()
        {
            return s.clone();
        }
    }
    String::new()
}

fn checked_url(raw: String, required: bool) -> Result<Option<String>> {
    if raw.is_empty() && !required {
        return Ok(None);
    }
    if raw.len() > 16 * 1024 || raw.trim() != raw || raw.chars().any(char::is_control) {
        bail!("音乐卡片URL无效");
    }
    let parsed = url::Url::parse(&raw).map_err(|_| anyhow::anyhow!("音乐卡片URL无效"))?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        bail!("音乐卡片URL须为无用户凭据的HTTP(S)地址");
    }
    Ok(Some(raw))
}

fn provider_name(name: &str) -> Option<Provider> {
    match name {
        "qq" => Some(Provider::Qq),
        "163" => Some(Provider::Netease),
        "xm" => Some(Provider::Xiami),
        _ => None,
    }
}

fn page_provider(page: &str) -> Provider {
    let Ok(url) = url::Url::parse(page) else {
        return Provider::Unknown;
    };
    match url.host_str().unwrap_or_default() {
        "y.qq.com" | "i.y.qq.com" | "c.y.qq.com" => Provider::Qq,
        "music.163.com" | "y.music.163.com" => Provider::Netease,
        _ => Provider::Unknown,
    }
}

impl MusicCard {
    pub fn new(
        provider: Provider,
        title: String,
        artist: String,
        page_url: String,
        audio_url: String,
        image_url: String,
    ) -> Result<Self> {
        if title.trim().is_empty() || title.len() > 4096 || artist.len() > 4096 {
            bail!("音乐标题为空或音乐说明过长");
        }
        let page_url = checked_url(page_url, true)?.unwrap();
        let inferred = page_provider(&page_url);
        if provider != Provider::Unknown && inferred != Provider::Unknown && provider != inferred {
            bail!("音乐来源与页面地址不一致");
        }
        Ok(Self {
            provider: if provider == Provider::Unknown {
                inferred
            } else {
                provider
            },
            title,
            artist,
            page_url,
            audio_url: checked_url(audio_url, false)?,
            image_url: checked_url(image_url, false)?,
        })
    }

    /// Identity for matching an existing native source card. Song IDs and
    /// songmids are distinct; matching titles or artists alone is insufficient.
    pub fn track_key(&self) -> Option<TrackKey> {
        let u = url::Url::parse(&self.page_url).ok()?;
        let pairs: Vec<_> = u.query_pairs().collect();
        for key in ["songmid", "songid", "id"] {
            if pairs.iter().filter(|(k, _)| k == key).count() > 1 {
                return None;
            }
        }
        let get = |name: &str| {
            let values: Vec<_> = pairs
                .iter()
                .filter(|(k, v)| k == name && !v.is_empty())
                .collect();
            (values.len() == 1).then(|| values[0].1.to_string())
        };
        match (self.provider, u.host_str()?) {
            (Provider::Qq, "y.qq.com" | "i.y.qq.com" | "c.y.qq.com") => {
                if let Some(mid) = get("songmid") {
                    return Some(TrackKey::QqSongMid(mid));
                }
                if let Some(id) = get("songid") {
                    return Some(TrackKey::QqSongId(id));
                }
                let path = u.path();
                path.strip_prefix("/n/ryqq/songDetail/")
                    .or_else(|| {
                        path.strip_prefix("/n/yqq/song/")
                            .and_then(|s| s.strip_suffix(".html"))
                    })
                    .filter(|s| !s.is_empty() && !s.contains('/'))
                    .map(|s| TrackKey::QqSongMid(s.to_owned()))
            }
            (Provider::Netease, "music.163.com" | "y.music.163.com") => {
                get("id").map(TrackKey::NeteaseSongId)
            }
            _ => None,
        }
    }
}

impl MusicRequest {
    /// A cache hit permits considering a native forward of that exact card.
    /// It is not evidence that arbitrary music can be created in WeChat.
    pub fn matches_cached(&self, cached: &MusicCard) -> bool {
        match self {
            Self::Catalog {
                provider: Provider::Qq,
                id,
            } => cached.track_key() == Some(TrackKey::QqSongId(id.clone())),
            Self::Catalog {
                provider: Provider::Netease,
                id,
            } => cached.track_key() == Some(TrackKey::NeteaseSongId(id.clone())),
            Self::Catalog { .. } => false,
            Self::Card(card) => {
                // A native forward preserves the source card's metadata. Do not
                // silently replace a custom title, artist or audio with it.
                if card.title != cached.title
                    || (!card.artist.is_empty() && card.artist != cached.artist)
                    || (card.audio_url.is_some() && card.audio_url != cached.audio_url)
                    || (card.image_url.is_some() && card.image_url != cached.image_url)
                {
                    return false;
                }
                match (card.track_key(), cached.track_key()) {
                    (Some(a), Some(b)) => a == b,
                    _ => card == cached,
                }
            }
        }
    }
}

/// Parse the official OneBot music segment or an explicit QQ music JSON card.
/// Unknown segment/card types return None; malformed recognized music errors.
pub fn parse_segment(segment: &Value) -> Result<Option<MusicRequest>> {
    match segment.get("type").and_then(Value::as_str) {
        Some("xml") => {
            let raw = segment["data"]["data"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("XML卡片data无效"))?;
            Ok(parse_native(raw)?.map(MusicRequest::Card))
        }
        Some("music") => {
            let d = &segment["data"];
            let kind = string(d, "type")?;
            let custom = kind == "custom"
                || (kind.is_empty()
                    && (d.get("audio").is_some()
                        || d.get("musicUrl").is_some()
                        || d.get("url").is_some()
                        || d.get("jumpUrl").is_some()));
            if custom {
                let page = first_string(d, &["url", "jumpUrl"]);
                let mut audio = first_string(d, &["audio", "musicUrl", "voice"]);
                if audio.is_empty() {
                    audio = first_string(d, &["voice"]);
                }
                if audio.is_empty() {
                    bail!("自定义音乐缺少audio地址");
                }
                let mut artist = first_string(d, &["content", "singer", "desc"]);
                if artist.is_empty() {
                    artist = first_string(d, &["singer"]);
                }
                let card = MusicCard::new(
                    provider_name(&string(d, "subtype")?).unwrap_or(Provider::Unknown),
                    first_string(d, &["title"]),
                    artist,
                    page,
                    audio,
                    first_string(d, &["image", "preview", "img"]),
                )?;
                return Ok(Some(MusicRequest::Card(card)));
            }
            let provider =
                provider_name(&kind).ok_or_else(|| anyhow::anyhow!("不支持的音乐平台"))?;
            let id = match &d["id"] {
                Value::String(s) => s.clone(),
                Value::Number(n) if n.is_u64() => n.to_string(),
                _ => bail!("音乐歌曲ID无效"),
            };
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                bail!("音乐歌曲ID无效");
            }
            // Some platforms attach full metadata alongside the catalog ID. Use
            // that request, never last session's cached card for the same ID.
            let title = first_string(d, &["title", "name"]);
            let page = first_string(d, &["url", "jumpUrl"]);
            let audio = first_string(d, &["audio", "musicUrl"]);
            if !title.trim().is_empty() && !page.is_empty() && !audio.is_empty() {
                let card = MusicCard::new(
                    provider,
                    title,
                    first_string(d, &["content", "singer", "desc", "artist"]),
                    page,
                    audio,
                    first_string(d, &["image", "preview", "img"]),
                )?;
                return Ok(Some(MusicRequest::Card(card)));
            }
            Ok(Some(MusicRequest::Catalog { provider, id }))
        }
        Some("json") => {
            let raw = &segment["data"]["data"];
            let value = if let Some(s) = raw.as_str() {
                if s.len() > MAX_CARD_BYTES {
                    bail!("音乐JSON卡片过大");
                }
                serde_json::from_str(s).map_err(|_| anyhow::anyhow!("JSON卡片格式错误"))?
            } else if raw.is_object() {
                if raw.to_string().len() > MAX_CARD_BYTES {
                    bail!("音乐JSON卡片过大");
                }
                raw.clone()
            } else {
                bail!("JSON卡片data无效");
            };
            Ok(parse_qq_json(&value)?.map(MusicRequest::Card))
        }
        _ => Ok(None),
    }
}

pub fn parse_qq_json(value: &Value) -> Result<Option<MusicCard>> {
    if !matches!(
        value["app"].as_str(),
        Some("com.tencent.structmsg" | "com.tencent.music.lua")
    ) || value["view"] != "music"
    {
        return Ok(None);
    }
    if value.to_string().len() > MAX_CARD_BYTES {
        bail!("音乐JSON卡片过大");
    }
    let music = &value["meta"]["music"];
    if !music.is_object() {
        bail!("QQ音乐JSON缺少meta.music");
    }
    MusicCard::new(
        Provider::Unknown,
        string(music, "title")?,
        string(music, "desc")?,
        string(music, "jumpUrl")?,
        string(music, "musicUrl")?,
        string(music, "preview")?,
    )
    .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_song_payload_uses_requested_identity_not_the_reference_song() {
        let card = MusicCard::new(
            Provider::Qq,
            "New ccc1 / CCC1 & <新曲>".into(),
            "Artist & B".into(),
            "https://i.y.qq.com/playsong?songmid=NewSong".into(),
            "https://audio.example/New?sig=KeepCase&x=1".into(),
            "https://image.example/New.jpg".into(),
        )
        .unwrap();
        let app = native_appmsg(&card, "wx0123456789abcdef").unwrap();
        let parsed = parse_native(&format!("<msg>{app}</msg>")).unwrap().unwrap();
        assert_eq!(parsed, card);
        assert!(!app.contains("STAY"));
        assert!(!app.contains("cdnthumbaeskey"));
        assert_eq!(
            native_app_id(&format!("<msg>{app}</msg>")).as_deref(),
            Some("wx0123456789abcdef")
        );
    }

    #[test]
    #[ignore = "set WECHAT_MUSIC_XML_FIXTURE to a private native reference XML"]
    fn supplied_native_reference_is_parsed_without_losing_track_identity() {
        let p = std::env::var("WECHAT_MUSIC_XML_FIXTURE").unwrap();
        let xml = std::fs::read_to_string(p).unwrap();
        let card = parse_native(&xml)
            .expect("native XML parser failed")
            .expect("not native type3");
        assert!(
            card.track_key().is_some(),
            "reference has no parsed track identity"
        );
    }

    #[test]
    fn native_outer_type_is_distinct_from_web_forward_and_quoted_music() {
        let card = r#"<msg><appmsg><type>3</type><title><![CDATA[STAY (Clean)]]></title><des>A &amp; B</des><url><![CDATA[https://i.y.qq.com/v8/playsong.html?songmid=KeepCase&uin=Private]]></url><dataurl>https://audio.example/Keep?sig=Case&amp;x=1</dataurl></appmsg></msg>"#;
        let parsed = parse_native(card).unwrap().unwrap();
        assert_eq!(parsed.title, "STAY (Clean)");
        assert_eq!(parsed.artist, "A & B");
        assert_eq!(
            parsed.audio_url.as_deref(),
            Some("https://audio.example/Keep?sig=Case&x=1")
        );
        for typ in ["5", "19", "57"] {
            assert!(
                parse_native(&card.replace("<type>3</type>", &format!("<type>{typ}</type>")))
                    .unwrap()
                    .is_none()
            );
        }
        assert!(
            parse_native(&format!(
                "<msg><appmsg><type>57</type><refermsg>{card}</refermsg></appmsg></msg>"
            ))
            .unwrap()
            .is_none()
        );
        assert!(
            parse_native(&card.replace("<type>3</type>", "<type>3</type><type>3</type>")).is_err()
        );
        assert!(parse_native(&format!("<!DOCTYPE msg>{card}")).is_err());
    }

    #[test]
    fn adapter_preparation_creates_new_song_and_only_reuses_provider_id() {
        let dir = tempfile::tempdir().unwrap();
        let (shared, _) = crate::service::Shared::new(crate::config::Config::default(
            dir.path().join("config.json"),
        ));
        let peer = crate::database::Peer {
            name: "测试好友".into(),
            group: false,
            wxid: "test-source".into(),
        };
        let mut card = qq("https://i.y.qq.com/v8/playsong.html?songmid=KeepCase");
        card.audio_url = Some("https://audio.example/Reference.mp3".into());
        shared.music_cache.write().unwrap().push(NativeCard {
            card: card.clone(),
            peer,
            source_key: "test:1".into(),
        });
        let qq_json = json!({"type":"json","data":{"data":{"app":"com.tencent.structmsg","view":"music","meta":{"music":{"title":card.title,"desc":card.artist,"jumpUrl":card.page_url,"musicUrl":card.audio_url}}}}});
        let parts = crate::delivery::prepare(&json!([qq_json.clone()]), false, &shared).unwrap();
        assert!(matches!(&parts[0], crate::delivery::Part::Music(_)));
        let mut other = qq_json;
        other["data"]["data"]["meta"]["music"]["jumpUrl"] =
            "https://i.y.qq.com/v8/playsong.html?songmid=Different".into();
        other["data"]["data"]["meta"]["music"]["title"] = "Different Song".into();
        other["data"]["data"]["meta"]["music"]["musicUrl"] =
            "https://audio.example/Different.mp3".into();
        let parts = crate::delivery::prepare(&json!([other]), false, &shared).unwrap();
        let crate::delivery::Part::Music(music) = &parts[0] else {
            panic!()
        };
        assert_eq!(music.card.title, "Different Song");
        assert_eq!(
            music.card.track_key(),
            Some(TrackKey::QqSongMid("Different".into()))
        );
        assert_eq!(
            music.card.audio_url.as_deref(),
            Some("https://audio.example/Different.mp3")
        );
    }

    fn qq(url: &str) -> MusicCard {
        MusicCard::new(
            Provider::Qq,
            "Case KEEP".into(),
            "Artist".into(),
            url.into(),
            String::new(),
            String::new(),
        )
        .unwrap()
    }

    #[test]
    fn custom_preserves_signed_urls_and_redacts_debug() {
        let page = "https://i.y.qq.com/v8/playsong.html?songmid=AbC123&hosteuin=PrivateCase";
        let audio = "https://audio.example/AbC.m4a?vkey=SECRETCase%2FA&uin=123";
        let parsed = parse_segment(&json!({"type":"music","data":{"type":"custom","title":"Keep My CASE","url":page,"audio":audio}})).unwrap().unwrap();
        let MusicRequest::Card(card) = &parsed else {
            panic!()
        };
        assert_eq!(card.page_url, page);
        assert_eq!(card.audio_url.as_deref(), Some(audio));
        assert_eq!(card.title, "Keep My CASE");
        assert!(!format!("{parsed:?}").contains("SECRETCase"));
        assert!(!format!("{parsed:?}").contains("PrivateCase"));
    }

    #[test]
    fn catalog_request_with_current_metadata_never_reuses_a_cached_old_card() {
        let parsed = parse_segment(&json!({"type":"music","data":{
            "type":"qq","id":"123",
            "title":"当前新歌","singer":"当前歌手",
            "url":"https://y.qq.com/n/ryqq/songDetail/NewSongMid",
            "audio":"https://audio.example/NewSong.mp3",
            "image":"https://image.example/NewSong.jpg"
        }}))
        .unwrap()
        .unwrap();
        let MusicRequest::Card(card) = parsed else {
            panic!("catalog id with full metadata must be a new card request")
        };
        assert_eq!(card.title, "当前新歌");
        assert_eq!(card.artist, "当前歌手");
        assert_eq!(
            card.audio_url.as_deref(),
            Some("https://audio.example/NewSong.mp3")
        );
        assert_eq!(
            card.track_key(),
            Some(TrackKey::QqSongMid("NewSongMid".into()))
        );
        assert_ne!(card.title, "STAY (Clean)");
    }

    #[test]
    fn supports_both_qq_json_envelopes_without_retaining_tokens() {
        for app in ["com.tencent.structmsg", "com.tencent.music.lua"] {
            let card = json!({"app":app,"view":"music","config":{"token":"not-for-logs"},"meta":{"music":{"title":"Song","desc":"Artist","jumpUrl":"https://i.y.qq.com/v8/playsong.html?songmid=AbC","musicUrl":"https://audio.example/Track?Sig=Keep","preview":"https://image.example/Cover.JPG"}}});
            for data in [card.clone(), Value::String(card.to_string())] {
                let parsed = parse_segment(&json!({"type":"json","data":{"data":data}}))
                    .unwrap()
                    .unwrap();
                let MusicRequest::Card(parsed) = parsed else {
                    panic!()
                };
                assert_eq!(parsed.track_key(), Some(TrackKey::QqSongMid("AbC".into())));
                assert_eq!(
                    parsed.audio_url.as_deref(),
                    Some("https://audio.example/Track?Sig=Keep")
                );
            }
        }
    }

    #[test]
    fn catalog_id_is_not_a_songmid_and_title_does_not_match() {
        let request = parse_segment(&json!({"type":"music","data":{"type":"qq","id":"123"}}))
            .unwrap()
            .unwrap();
        assert!(!request.matches_cached(&qq("https://i.y.qq.com/v8/playsong.html?songmid=123")));
        assert!(request.matches_cached(&qq("https://i.y.qq.com/v8/playsong.html?songid=123")));
        let request = MusicRequest::Card(qq("https://i.y.qq.com/v8/playsong.html?songmid=CaseA"));
        assert!(!request.matches_cached(&qq("https://i.y.qq.com/v8/playsong.html?songmid=casea")));
        assert!(request.matches_cached(&qq(
            "https://i.y.qq.com/v8/playsong.html?songmid=CaseA&uin=Different"
        )));
    }

    #[test]
    fn rejects_ambiguous_ids_bad_urls_and_non_music_json() {
        assert_eq!(
            qq("https://i.y.qq.com/v8/playsong.html?songmid=A&songmid=B").track_key(),
            None
        );
        for audio in [
            "file:///C:/private.mp3",
            "https://user:password@example.com/song",
            "https://example.com/song\n",
        ] {
            assert!(parse_segment(&json!({"type":"music","data":{"type":"custom","title":"Song","url":"https://example.com/song","audio":audio}})).is_err());
        }
        assert!(
            parse_qq_json(
                &json!({"app":"com.tencent.structmsg","view":"news","meta":{"music":{}}})
            )
            .unwrap()
            .is_none()
        );
        assert!(parse_segment(&json!({"type":"music","data":{"type":"custom","title":"Song","url":"https://example.com/song"}})).is_err());
    }

    #[test]
    fn native_reuse_must_not_change_requested_metadata_or_audio() {
        let cached = qq("https://i.y.qq.com/v8/playsong.html?songmid=SameSong");
        let mut changed = cached.clone();
        changed.title = "Custom different title".into();
        assert!(!MusicRequest::Card(changed).matches_cached(&cached));
        let mut changed = cached.clone();
        changed.audio_url = Some("https://audio.example/DifferentRecording.mp3".into());
        assert!(!MusicRequest::Card(changed).matches_cached(&cached));
    }
}
