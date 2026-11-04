//! Offline CGI127 (`uploadvoice`) codec. This does not submit network tasks.
//!
//! Contract source: thisnick/agent-wechat, tree
//! 32a35c29cd0c888172c0943cf751eaec46b90423,
//! packages/native-client-rust/crates/wechat-protocol/src/media_send.rs.
//! The field numbers/types are corroborated by Weixin.dll 4.2.0.12 SHA-256
//! 2b3677c3f932daaa4ef84c8b2acd0aee12cbe5a779ff9e740c370761e5c291ce:
//! request serializer RVA 0x2fd7c80, response serializer RVA 0x2fd9520.
//! These static observations and offline tests do not establish live delivery
//! or multi-chunk service behavior. SILK framing is not an audio decode.
use crate::music_wire::{Field, fields};

pub const CGI_ID: u32 = 127;
pub const CGI_PATH: &str = "/cgi-bin/micromsg-bin/uploadvoice";
pub const CHUNK_BYTES: usize = 32 * 1024;
pub const MAX_DURATION_MS: u32 = 60_000;
/// Local allocation bound, not a claim about the service's encoded-size limit.
pub const MAX_ENCODED_BYTES: usize = 512 * 1024;

fn integer(mut n: u64, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn num(tag: u32, n: u64, out: &mut Vec<u8>) {
    integer((tag as u64) << 3, out);
    integer(n, out);
}
fn bytes(tag: u32, b: &[u8], out: &mut Vec<u8>) {
    integer(((tag as u64) << 3) | 2, out);
    integer(b.len() as u64, out);
    out.extend_from_slice(b);
}
fn buffer(value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(value.len() + 12);
    num(1, value.len() as u64, &mut out);
    bytes(2, value, &mut out);
    out
}
fn tag(f: &Field<'_>) -> u32 {
    match f {
        Field::Number(n, _) | Field::Bytes(n, _) | Field::Fixed(n) => *n,
    }
}
fn parsed(data: &[u8]) -> Result<Vec<Field<'_>>, String> {
    let f = fields(data)?;
    if f.iter().any(|f| tag(f) > 0x1fff_ffff) {
        return Err("原生语音protobuf字段号超限".into());
    }
    Ok(f)
}
fn one<'f, 'a>(f: &'f [Field<'a>], n: u32) -> Result<Option<&'f Field<'a>>, String> {
    let mut found = f.iter().filter(|f| tag(f) == n);
    let value = found.next();
    if found.next().is_some() {
        return Err("原生语音响应字段重复".into());
    }
    Ok(value)
}
fn data<'a>(f: &[Field<'a>], n: u32) -> Result<&'a [u8], String> {
    match one(f, n)? {
        Some(Field::Bytes(_, b)) => Ok(b),
        _ => Err("原生语音响应字节字段缺失或类型错误".into()),
    }
}
fn number(f: &[Field<'_>], n: u32) -> Result<u64, String> {
    match one(f, n)? {
        Some(Field::Number(_, v)) => Ok(*v),
        _ => Err("原生语音响应整数缺失或类型错误".into()),
    }
}
fn optional_number(f: &[Field<'_>], n: u32) -> Result<Option<u64>, String> {
    match one(f, n)? {
        Some(Field::Number(_, v)) => Ok(Some(*v)),
        None => Ok(None),
        _ => Err("原生语音响应整数类型错误".into()),
    }
}
fn u32_number(f: &[Field<'_>], n: u32) -> Result<u32, String> {
    u32::try_from(number(f, n)?).map_err(|_| "原生语音响应整数溢出".into())
}
fn clean(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn valid_identity(from: &str, to: &str, client: &str) -> bool {
    from.starts_with("wxid_")
        && clean(from, 256)
        && clean(to, 512)
        && (16..=128).contains(&client.len())
        && client
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'@'))
}
fn buffer_data<'a>(encoded: &'a [u8]) -> Result<&'a [u8], String> {
    let f = parsed(encoded)?;
    if f.len() != 2 {
        return Err("原生语音SKBuffer字段不完整或含额外内容".into());
    }
    let b = data(&f, 2)?;
    if number(&f, 1)? != b.len() as u64 {
        return Err("原生语音SKBuffer长度与内容不一致".into());
    }
    Ok(b)
}

/// Validate one host-generated chunk before the helper allocates a native task.
/// This whitelist excludes BaseRequest, account material and unexpected fields;
/// the helper must add only its own native-generated BaseRequest at tag 10.
/// The host owns serial progress and must correlate the native task completion.
pub fn validate_request_fields(
    body: &[u8],
    from: &str,
    to: &str,
    client: &str,
) -> Result<(), String> {
    if !valid_identity(from, to, client) || body.len() > CHUNK_BYTES + 4096 {
        return Err("原生语音分片身份或请求长度无效".into());
    }
    let f = parsed(body)?;
    for value in &f {
        match (tag(value), value) {
            (1 | 2 | 5 | 8 | 18 | 21, Field::Bytes(_, _))
            | (3 | 4 | 6 | 7 | 9 | 11 | 13 | 15 | 16 | 17, Field::Number(_, _)) => (),
            _ => return Err("原生语音分片含未授权字段或错误类型".into()),
        }
        one(&f, tag(value))?;
    }
    if data(&f, 1)? != from.as_bytes()
        || data(&f, 2)? != to.as_bytes()
        || data(&f, 5)? != client.as_bytes()
    {
        return Err("原生语音分片身份与任务不一致".into());
    }
    let offset = u32_number(&f, 3)? as usize;
    let length = u32_number(&f, 4)? as usize;
    let chunk = buffer_data(data(&f, 8)?)?;
    let end = offset.checked_add(length).ok_or("原生语音分片范围溢出")?;
    let final_flag = u32_number(&f, 9)?;
    if offset >= MAX_ENCODED_BYTES
        || offset % CHUNK_BYTES != 0
        || length == 0
        || length > CHUNK_BYTES
        || chunk.len() != length
        || end > MAX_ENCODED_BYTES
        || final_flag > 1
        || (final_flag == 0 && length != CHUNK_BYTES)
        || !(1..=MAX_DURATION_MS).contains(&u32_number(&f, 7)?)
        || number(&f, 11)? != 0
        || number(&f, 13)? != 4
        || number(&f, 15)? != 0
        || number(&f, 16)? != 0
        || u32_number(&f, 17)? == 0
        || !data(&f, 21)?.is_empty()
    {
        return Err("原生语音分片范围、格式、时间或控制标识无效".into());
    }
    if let Some(id) = optional_number(&f, 6)? {
        if id == 0 || id > u32::MAX as u64 {
            return Err("原生语音分片旧消息ID无效".into());
        }
    }
    if offset == 0 {
        if !chunk.starts_with(b"\x02#!SILK_V3") || buffer_data(data(&f, 18)?)? != client.as_bytes()
        {
            return Err("原生语音首片SILK头或VoiceId不一致".into());
        }
        if final_flag == 1 {
            validate_silk(chunk)?;
        }
    } else if one(&f, 18)?.is_some() {
        return Err("原生语音后续分片重复携带VoiceId".into());
    }
    Ok(())
}

/// Check the Tencent SILK container, including every packet length. Duration
/// must come from the caller's actual decode/encode result, not a filename or
/// this packet count: SILK packets can hold different audio durations.
pub fn validate_silk(encoded: &[u8]) -> Result<(), String> {
    if encoded.len() < 13
        || encoded.len() > MAX_ENCODED_BYTES
        || !encoded.starts_with(b"\x02#!SILK_V3")
    {
        return Err("原生语音要求有界的腾讯SILK_V3编码".into());
    }
    let (mut p, mut packets) = (10usize, 0usize);
    while p < encoded.len() {
        let prefix = encoded.get(p..p + 2).ok_or("SILK包长度截断")?;
        let size = i16::from_le_bytes([prefix[0], prefix[1]]);
        if !(1..=5120).contains(&size) {
            return Err("SILK包长度无效".into());
        }
        p = p
            .checked_add(2 + size as usize)
            .filter(|p| *p <= encoded.len())
            .ok_or("SILK包内容截断")?;
        packets += 1;
        if packets > 3000 {
            return Err("SILK包数超出单条语音限制".into());
        }
    }
    Ok(())
}

/// Accept only the voice task's own native-generated BaseRequest at tag 10.
/// No session subfields are interpreted, created or returned as diagnostics.
pub fn native_base_only(native: &[u8]) -> Result<(), String> {
    match parsed(native)?.as_slice() {
        [Field::Bytes(10, b)] if !b.is_empty() => Ok(()),
        _ => Err("原生语音BaseRequest缺失、重复或已有其他负载".into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ack {
    pub server_id: u64,
    pub legacy_server_id: u32,
    pub created_at: u32,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadReply {
    More,
    Complete(Ack),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Ready,
    Awaiting,
    Finished,
    Failed,
}

/// One immutable note and its serial upload progress. No automatic retry is
/// provided: once a request has been issued, a missing/invalid reply poisons
/// this object and the owner must retain the uncertain outcome for review.
/// Do not log this object: it owns the caller's voice bytes and identities.
pub struct VoiceUpload {
    from: String,
    to: String,
    client: String,
    created_at: u32,
    duration_ms: u32,
    encoded: Vec<u8>,
    offset: usize,
    legacy_id: Option<u32>,
    server_id: Option<u64>,
    phase: Phase,
}
impl VoiceUpload {
    pub fn new(
        from: &str,
        to: &str,
        client: &str,
        created_at: u32,
        duration_ms: u32,
        encoded: &[u8],
    ) -> Result<Self, String> {
        if !valid_identity(from, to, client)
            || created_at == 0
            || !(1..=MAX_DURATION_MS).contains(&duration_ms)
        {
            return Err("原生语音任务身份、时间或时长无效".into());
        }
        validate_silk(encoded)?;
        Ok(Self {
            from: from.into(),
            to: to.into(),
            client: client.into(),
            created_at,
            duration_ms,
            encoded: encoded.to_vec(),
            offset: 0,
            legacy_id: None,
            server_id: None,
            phase: Phase::Ready,
        })
    }
    pub fn request_count(&self) -> usize {
        self.encoded.len().div_ceil(CHUNK_BYTES)
    }
    fn end(&self) -> usize {
        (self.offset + CHUNK_BYTES).min(self.encoded.len())
    }
    fn final_range(&self) -> bool {
        self.end() == self.encoded.len()
    }
    /// Return exactly one chunk's fields, excluding native BaseRequest tag 10.
    /// Store these bytes in its owned native task; do not call again until that
    /// task has a verified reply and completed its native completion callback.
    pub fn next_fields(&mut self) -> Result<Vec<u8>, String> {
        if self.phase != Phase::Ready {
            return Err("原生语音当前分片已提交或任务已结束，拒绝重发".into());
        }
        let chunk = &self.encoded[self.offset..self.end()];
        let mut out = Vec::with_capacity(chunk.len() + 512);
        bytes(1, self.from.as_bytes(), &mut out);
        bytes(2, self.to.as_bytes(), &mut out);
        num(3, self.offset as u64, &mut out);
        num(4, chunk.len() as u64, &mut out);
        bytes(5, self.client.as_bytes(), &mut out);
        if let Some(id) = self.legacy_id {
            num(6, id.into(), &mut out);
        }
        num(7, self.duration_ms.into(), &mut out);
        bytes(8, &buffer(chunk), &mut out);
        num(9, self.final_range().into(), &mut out);
        num(11, 0, &mut out);
        num(13, 4, &mut out);
        num(15, 0, &mut out);
        num(16, 0, &mut out);
        num(17, self.created_at.into(), &mut out);
        if self.offset == 0 {
            bytes(18, &buffer(self.client.as_bytes()), &mut out);
        }
        // Opaque empty extension emitted by the cited primary implementation.
        bytes(21, &[], &mut out);
        self.phase = Phase::Awaiting;
        Ok(out)
    }
    /// Treat any transport error/missing response as an uncertain terminal
    /// outcome. This deliberately exposes no reset/retry operation.
    pub fn fail(&mut self) {
        self.phase = Phase::Failed;
    }
    pub fn accept_reply(&mut self, raw: &[u8]) -> Result<UploadReply, String> {
        if self.phase != Phase::Awaiting {
            return Err("原生语音响应没有对应的待完成分片".into());
        }
        // Only a fully checked response may permit the next request.
        self.phase = Phase::Failed;
        let f = parsed(raw)?;
        for n in 1..=13 {
            if let Some(value) = one(&f, n)? {
                if !matches!(
                    (n, value),
                    (1 | 2 | 6 | 10, Field::Bytes(_, _))
                        | (3 | 4 | 5 | 7 | 8 | 9 | 11 | 12 | 13, Field::Number(_, _))
                ) {
                    return Err("原生语音响应字段类型错误".into());
                }
            }
        }
        let base = parsed(data(&f, 10)?)?;
        let ret = number(&base, 1)?;
        if ret > u32::MAX as u64 && ret != (ret as i32 as i64 as u64) {
            return Err("原生语音响应ret溢出".into());
        }
        if ret != 0 {
            return Err(format!("微信服务端拒绝原生语音，ret={}", ret as i32));
        }
        if data(&f, 1)? != self.from.as_bytes()
            || data(&f, 2)? != self.to.as_bytes()
            || data(&f, 6)? != self.client.as_bytes()
        {
            return Err("原生语音响应与本次发送身份不一致".into());
        }
        // Generated schemas identify these uint32 fields, but do not establish
        // whether Offset/Length are echoed or cumulative; do not guess equality.
        for n in [3, 4, 5, 7, 8, 9] {
            u32_number(&f, n)?;
        }
        // Request EndFlag=1 means this is our last chunk. The current native
        // CGI127 server's confirmed completed response uses EndFlag=2 instead
        // of echoing that request flag. Intermediate flag 0 is the bounded
        // upstream contract; other values have no established meaning here.
        let expected_response_end = if self.final_range() { 2 } else { 0 };
        if u32_number(&f, 8)? != self.duration_ms
            || u32_number(&f, 9)? != expected_response_end
            || optional_number(&f, 11)?.unwrap_or(0) != 0
        {
            return Err(format!(
                "原生语音响应标识不一致（时长{}，期望{}；结束{}，期望{}；取消{}）",
                u32_number(&f, 8)?,
                self.duration_ms,
                u32_number(&f, 9)?,
                expected_response_end,
                optional_number(&f, 11)?.unwrap_or(0)
            ));
        }
        let legacy = u32_number(&f, 7)?;
        if legacy != 0 && self.legacy_id.is_some_and(|id| id != legacy) {
            return Err("原生语音响应旧消息ID变更".into());
        }
        let server = optional_number(&f, 12)?.filter(|id| *id != 0);
        if self
            .server_id
            .is_some_and(|known| server.is_some_and(|id| id != known))
        {
            return Err("原生语音响应服务器消息ID变更".into());
        }
        if self.final_range() {
            let server_id = server.ok_or("原生语音最终响应缺少服务器消息ID")?;
            let created_at = u32_number(&f, 5)?;
            if created_at == 0 {
                return Err("原生语音最终响应创建时间为空".into());
            }
            self.phase = Phase::Finished;
            Ok(UploadReply::Complete(Ack {
                server_id,
                legacy_server_id: self.legacy_id.unwrap_or(legacy),
                created_at,
            }))
        } else {
            self.legacy_id = (legacy != 0).then_some(legacy).or(self.legacy_id);
            self.server_id = server.or(self.server_id);
            self.offset = self.end();
            self.phase = Phase::Ready;
            Ok(UploadReply::More)
        }
    }
}

/// Bounded numeric diagnostics only: never retain account names, client IDs,
/// native session material, audio, or an unparsed native response in the log.
pub fn response_metadata(raw: &[u8]) -> serde_json::Value {
    let Ok(f) = parsed(raw) else {
        return serde_json::json!({"parse_error":true});
    };
    let mut out = serde_json::Map::new();
    for n in [3, 4, 5, 7, 8, 9, 11, 12, 13] {
        if let Ok(Some(v)) = optional_number(&f, n) {
            out.insert(format!("field_{n}"), v.into());
        }
    }
    if let Ok(b) = data(&f, 10).and_then(parsed) {
        if let Ok(ret) = number(&b, 1) {
            out.insert("ret".into(), (ret as i32).into());
        }
    }
    serde_json::Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    const CLIENT: &str = "voice_SyntheticCaseId";
    #[test]
    fn group_voice_preserves_native_chatroom_target_and_client_identity() {
        let mut u = VoiceUpload::new(
            "wxid_self",
            "12345@chatroom",
            "12345@chatroom_1234_42",
            1234,
            1000,
            &silk(10, 10),
        )
        .unwrap();
        let body = u.next_fields().unwrap();
        validate_request_fields(
            &body,
            "wxid_self",
            "12345@chatroom",
            "12345@chatroom_1234_42",
        )
        .unwrap();
    }
    fn silk(packets: usize, size: usize) -> Vec<u8> {
        let mut b = b"\x02#!SILK_V3".to_vec();
        for _ in 0..packets {
            b.extend_from_slice(&(size as i16).to_le_bytes());
            b.extend(std::iter::repeat_n(0x55, size));
        }
        b
    }
    fn upload(b: &[u8]) -> VoiceUpload {
        VoiceUpload::new("wxid_self", "wxid_peer", CLIENT, 1234, 1000, b).unwrap()
    }
    fn reply(final_range: bool, server: u64) -> Vec<u8> {
        let mut b = vec![];
        bytes(1, b"wxid_self", &mut b);
        bytes(2, b"wxid_peer", &mut b);
        num(3, 0, &mut b);
        num(4, 100, &mut b);
        num(5, 1235, &mut b);
        bytes(6, CLIENT.as_bytes(), &mut b);
        num(7, 123, &mut b);
        num(8, 1000, &mut b);
        num(9, if final_range { 2 } else { 0 }, &mut b);
        bytes(10, &[8, 0], &mut b);
        num(11, 0, &mut b);
        num(12, server, &mut b);
        b
    }
    #[test]
    fn confirmed_native_terminal_response_uses_two_not_request_end_flag() {
        // Numeric fields from the isolated 1-second native response; the user
        // confirmed an earlier note from the same native profile was audible.
        // Account, destination, client ID and audio are synthetic fixtures.
        let mut response = vec![];
        bytes(1, b"wxid_self", &mut response);
        bytes(2, b"wxid_peer", &mut response);
        for (tag, value) in [(3, 0), (4, 2891), (5, 1_793_804_924)] {
            num(tag, value, &mut response);
        }
        bytes(6, CLIENT.as_bytes(), &mut response);
        for (tag, value) in [(7, 607_407_271), (8, 1000), (9, 2)] {
            num(tag, value, &mut response);
        }
        bytes(10, &[8, 0], &mut response);
        for (tag, value) in [(11, 0), (12, 2_655_813_048_745_716_639), (13, 0)] {
            num(tag, value, &mut response);
        }
        let mut upload = upload(&silk(1, 2879));
        let request = upload.next_fields().unwrap();
        assert_eq!(number(&parsed(&request).unwrap(), 9).unwrap(), 1);
        assert_eq!(
            upload.accept_reply(&response).unwrap(),
            UploadReply::Complete(Ack {
                server_id: 2_655_813_048_745_716_639,
                legacy_server_id: 607_407_271,
                created_at: 1_793_804_924,
            })
        );
    }
    #[test]
    fn response_flags_are_exact_and_never_treated_as_an_arbitrary_bitmask() {
        for (final_range, flags) in [
            (true, vec![0, 1, 3, 4, u32::MAX as u64]),
            (false, vec![1, 2, 3, 4, u32::MAX as u64]),
        ] {
            for flag in flags {
                let response = reply(final_range, 99);
                let mut bad = vec![];
                for field in parsed(&response).unwrap() {
                    match field {
                        Field::Number(tag, v) => {
                            num(tag, if tag == 9 { flag } else { v }, &mut bad)
                        }
                        Field::Bytes(tag, v) => bytes(tag, v, &mut bad),
                        Field::Fixed(_) => unreachable!(),
                    }
                }
                let note = if final_range {
                    silk(1, 10)
                } else {
                    silk(400, 100)
                };
                let mut u = upload(&note);
                u.next_fields().unwrap();
                let error = u.accept_reply(&bad).unwrap_err();
                assert!(error.contains("响应标识不一致"), "{error}");
                assert!(u.next_fields().is_err());
                assert!(u.accept_reply(&response).is_err());
            }
        }
    }
    #[test]
    fn native_session_requires_voice_tag_and_refuses_existing_payload() {
        let mut original = vec![];
        bytes(1, b"synthetic-private-native-base", &mut original);
        assert!(native_base_only(&original).is_err());
        let mut voice = vec![];
        bytes(10, b"synthetic-private-native-base", &mut voice);
        native_base_only(&voice).unwrap();
        assert_eq!(
            data(&parsed(&voice).unwrap(), 10).unwrap(),
            b"synthetic-private-native-base"
        );
        let mut duplicate = voice.clone();
        duplicate.extend_from_slice(&voice);
        assert!(native_base_only(&duplicate).is_err());
        bytes(8, b"existing-voice", &mut voice);
        assert!(native_base_only(&voice).is_err());
    }
    #[test]
    fn chunks_reassemble_exact_source_and_preserve_one_identity() {
        let encoded = silk(400, 100);
        let mut u = upload(&encoded);
        assert_eq!(u.request_count(), 2);
        let first = u.next_fields().unwrap();
        validate_request_fields(&first, "wxid_self", "wxid_peer", CLIENT).unwrap();
        assert!(u.next_fields().is_err());
        let first = parsed(&first).unwrap();
        assert!(one(&first, 10).unwrap().is_none());
        assert_eq!(number(&first, 3).unwrap(), 0);
        assert_eq!(number(&first, 9).unwrap(), 0);
        assert_eq!(number(&first, 13).unwrap(), 4);
        let a = parsed(data(&first, 8).unwrap()).unwrap();
        assert_eq!(number(&a, 1).unwrap(), CHUNK_BYTES as u64);
        let mut reassembled = data(&a, 2).unwrap().to_vec();
        assert_eq!(
            u.accept_reply(&reply(false, u64::MAX)).unwrap(),
            UploadReply::More
        );
        let second = u.next_fields().unwrap();
        validate_request_fields(&second, "wxid_self", "wxid_peer", CLIENT).unwrap();
        let second = parsed(&second).unwrap();
        assert_eq!(data(&second, 5).unwrap(), CLIENT.as_bytes());
        assert_eq!(number(&second, 3).unwrap(), CHUNK_BYTES as u64);
        assert_eq!(number(&second, 6).unwrap(), 123);
        assert_eq!(number(&second, 9).unwrap(), 1);
        assert!(one(&second, 18).unwrap().is_none());
        let a = parsed(data(&second, 8).unwrap()).unwrap();
        reassembled.extend_from_slice(data(&a, 2).unwrap());
        assert_eq!(reassembled, encoded);
        assert_eq!(
            u.accept_reply(&reply(true, u64::MAX)).unwrap(),
            UploadReply::Complete(Ack {
                server_id: u64::MAX,
                legacy_server_id: 123,
                created_at: 1235
            })
        );
        assert!(u.next_fields().is_err());
        assert!(u.accept_reply(&reply(true, u64::MAX)).is_err());
    }
    #[test]
    fn malformed_or_mismatched_response_never_becomes_success_or_retry() {
        let valid = reply(true, u64::MAX);
        for end in 0..valid.len() {
            let mut u = upload(&silk(10, 10));
            u.next_fields().unwrap();
            assert!(u.accept_reply(&valid[..end]).is_err());
            assert!(u.next_fields().is_err());
        }
        for (n, value) in [
            (1, b"wrong".as_slice()),
            (2, b"wrong"),
            (6, b"voice_syntheticcaseid"),
        ] {
            let mut bad = vec![];
            for field in parsed(&valid).unwrap() {
                match field {
                    Field::Number(tag, v) => num(tag, v, &mut bad),
                    Field::Bytes(tag, v) => bytes(tag, if tag == n { value } else { v }, &mut bad),
                    Field::Fixed(_) => unreachable!(),
                }
            }
            let mut u = upload(&silk(10, 10));
            u.next_fields().unwrap();
            assert!(u.accept_reply(&bad).is_err());
        }
        for (n, value) in [(8, 1001), (9, 0), (11, 1), (12, 0)] {
            // Rebuild with that field replaced, avoiding a duplicate-field-only test.
            let mut bad = vec![];
            for field in parsed(&valid).unwrap() {
                match field {
                    Field::Number(tag, v) => num(tag, if tag == n { value } else { v }, &mut bad),
                    Field::Bytes(tag, v) => bytes(tag, v, &mut bad),
                    Field::Fixed(_) => unreachable!(),
                }
            }
            let mut u = upload(&silk(10, 10));
            u.next_fields().unwrap();
            assert!(u.accept_reply(&bad).is_err());
            assert!(u.next_fields().is_err());
        }
    }
    #[test]
    fn duplicate_wrong_wire_and_missing_status_are_refused() {
        let valid = reply(true, 99);
        for n in 1..=13 {
            let mut bad = valid.clone();
            if [1, 2, 6, 10].contains(&n) {
                bytes(n, b"duplicate", &mut bad);
            } else {
                num(n, 0, &mut bad);
            }
            let mut u = upload(&silk(1, 10));
            u.next_fields().unwrap();
            // Field13 was absent: a single well-typed unknown scalar is valid.
            if n != 13 {
                assert!(u.accept_reply(&bad).is_err());
            }
            let mut wrong_type = vec![];
            for f in parsed(&valid).unwrap() {
                if tag(&f) == n {
                    match f {
                        Field::Number(_, _) => bytes(n, b"wrong-wire", &mut wrong_type),
                        _ => num(n, 0, &mut wrong_type),
                    }
                } else {
                    match f {
                        Field::Number(tag, v) => num(tag, v, &mut wrong_type),
                        Field::Bytes(tag, v) => bytes(tag, v, &mut wrong_type),
                        Field::Fixed(_) => unreachable!(),
                    }
                }
            }
            let mut u = upload(&silk(1, 10));
            u.next_fields().unwrap();
            if n != 13 {
                assert!(u.accept_reply(&wrong_type).is_err());
            }
        }
        for base in [vec![], vec![8, 0, 8, 0], vec![10, 0]] {
            let mut bad = vec![];
            bytes(10, &base, &mut bad);
            let mut u = upload(&silk(1, 10));
            u.next_fields().unwrap();
            assert!(u.accept_reply(&bad).is_err());
        }
    }
    #[test]
    fn helper_preflight_rejects_injected_session_and_inconsistent_chunk_metadata() {
        let mut u = upload(&silk(10, 10));
        let valid = u.next_fields().unwrap();
        validate_request_fields(&valid, "wxid_self", "wxid_peer", CLIENT).unwrap();
        assert!(validate_request_fields(&valid, "wxid_self", "wrong", CLIENT).is_err());
        for tag in [1, 5, 8, 10, 13, 18, 21, 24] {
            let mut bad = valid.clone();
            bytes(tag, b"injected", &mut bad);
            assert!(validate_request_fields(&bad, "wxid_self", "wxid_peer", CLIENT).is_err());
        }
        for (n, value) in [
            (3, 1),
            (4, 1),
            (7, 0),
            (9, 2),
            (11, 1),
            (13, 3),
            (15, 1),
            (16, 1),
            (17, 0),
        ] {
            let mut bad = vec![];
            for field in parsed(&valid).unwrap() {
                match field {
                    Field::Number(tag, v) => num(tag, if tag == n { value } else { v }, &mut bad),
                    Field::Bytes(tag, v) => bytes(tag, v, &mut bad),
                    Field::Fixed(_) => unreachable!(),
                }
            }
            assert!(validate_request_fields(&bad, "wxid_self", "wxid_peer", CLIENT).is_err());
        }
        let mut bad = vec![];
        for field in parsed(&valid).unwrap() {
            match field {
                Field::Number(tag, v) => num(tag, v, &mut bad),
                Field::Bytes(8, _) => bytes(8, &[8, 2, 18, 1, 0], &mut bad),
                Field::Bytes(tag, v) => bytes(tag, v, &mut bad),
                Field::Fixed(_) => unreachable!(),
            }
        }
        assert!(validate_request_fields(&bad, "wxid_self", "wxid_peer", CLIENT).is_err());
    }
    #[test]
    fn changed_server_id_and_nonzero_ret_poison_upload() {
        let mut u = upload(&silk(400, 100));
        u.next_fields().unwrap();
        u.accept_reply(&reply(false, 99)).unwrap();
        u.next_fields().unwrap();
        assert!(u.accept_reply(&reply(true, 100)).is_err());
        assert!(u.next_fields().is_err());
        for ret in [1, u32::MAX as u64, (-1i64) as u64, 1u64 << 32] {
            let mut b = vec![];
            let mut base = vec![];
            num(1, ret, &mut base);
            bytes(10, &base, &mut b);
            let mut u = upload(&silk(1, 10));
            u.next_fields().unwrap();
            assert!(u.accept_reply(&b).is_err());
            assert!(u.next_fields().is_err());
        }
    }
    #[test]
    fn rejects_container_truncation_and_duration_limits_before_submission() {
        let good = silk(2, 10);
        validate_silk(&good).unwrap();
        for bad in [
            vec![],
            b"ID3audio-file".to_vec(),
            b"\x02#!SILK_V3\0\0".to_vec(),
            b"\x02#!SILK_V3\xff\xff".to_vec(),
            good[..good.len() - 1].to_vec(),
        ] {
            assert!(validate_silk(&bad).is_err());
        }
        for duration in [0, MAX_DURATION_MS + 1] {
            assert!(
                VoiceUpload::new("wxid_self", "wxid_peer", CLIENT, 1234, duration, &good).is_err()
            );
        }
        let mut u = upload(&good);
        u.next_fields().unwrap();
        u.fail();
        assert!(u.next_fields().is_err());
        assert!(u.accept_reply(&reply(true, 99)).is_err());
    }
}
