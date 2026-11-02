//! Narrow CGI222 wire codec. No account/session bytes are created or logged here.
#[derive(Debug, PartialEq)]
pub enum Field<'a> {
    Number(u32, u64),
    Bytes(u32, &'a [u8]),
    Fixed(u32),
}
pub fn fields(data: &[u8]) -> Result<Vec<Field<'_>>, String> {
    if data.len() > 1024 * 1024 {
        return Err("protobuf超出限制".into());
    }
    fn varint(d: &[u8], p: &mut usize) -> Result<u64, String> {
        let mut n = 0;
        for i in 0..10 {
            let b = *d.get(*p).ok_or("protobuf截断")?;
            *p += 1;
            if i == 9 && b > 1 {
                return Err("protobuf整数溢出".into());
            }
            n |= ((b & 127) as u64) << (i * 7);
            if b & 128 == 0 {
                return Ok(n);
            }
        }
        Err("protobuf整数无结束字节".into())
    }
    let (mut p, mut out) = (0, vec![]);
    while p < data.len() {
        if out.len() >= 128 {
            return Err("protobuf字段过多".into());
        }
        let tag = varint(data, &mut p)?;
        let number = u32::try_from(tag >> 3).map_err(|_| "protobuf字段溢出")?;
        if number == 0 {
            return Err("protobuf零字段号".into());
        }
        out.push(match tag & 7 {
            0 => Field::Number(number, varint(data, &mut p)?),
            2 => {
                let len = usize::try_from(varint(data, &mut p)?).map_err(|_| "protobuf长度溢出")?;
                let end = p
                    .checked_add(len)
                    .filter(|e| *e <= data.len())
                    .ok_or("protobuf字段截断")?;
                let v = &data[p..end];
                p = end;
                Field::Bytes(number, v)
            }
            1 | 5 => {
                p = p
                    .checked_add(if tag & 7 == 1 { 8 } else { 4 })
                    .filter(|e| *e <= data.len())
                    .ok_or("protobuf定长字段截断")?;
                Field::Fixed(number)
            }
            _ => return Err("protobuf未知wire类型".into()),
        });
    }
    Ok(out)
}
fn integer(n: u64, out: &mut Vec<u8>) {
    let mut n = n;
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7
    }
    out.push(n as u8)
}
fn num(tag: u32, n: u64, out: &mut Vec<u8>) {
    integer((tag as u64) << 3, out);
    integer(n, out)
}
fn bytes(tag: u32, value: &[u8], out: &mut Vec<u8>) {
    integer(((tag as u64) << 3) | 2, out);
    integer(value.len() as u64, out);
    out.extend_from_slice(value)
}
pub fn appmsg_field(
    from: &str,
    to: &str,
    xml: &str,
    client_id: &str,
    now: u32,
) -> Result<Vec<u8>, String> {
    if !from.starts_with("wxid_")
        || [from, to, client_id]
            .iter()
            .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
        || xml.is_empty()
        || xml.len() > 256 * 1024
        || xml.contains('\0')
    {
        return Err("原生音乐请求字段无效".into());
    }
    let mut m = vec![];
    bytes(1, from.as_bytes(), &mut m);
    num(3, 0, &mut m);
    bytes(4, to.as_bytes(), &mut m);
    num(5, 3, &mut m);
    bytes(6, xml.as_bytes(), &mut m);
    num(7, now as u64, &mut m);
    bytes(8, client_id.as_bytes(), &mut m);
    bytes(12, b"<msgsource><bizflag>0</bizflag></msgsource>", &mut m);
    let mut out = vec![];
    bytes(2, &m, &mut out);
    Ok(out)
}
/// Accept the native-generated BaseRequest without exposing its contents.
pub fn native_base_only(data: &[u8]) -> Result<(), String> {
    let f = fields(data)?;
    let mut base = 0;
    for v in f {
        match v {
            Field::Bytes(1, b) if !b.is_empty() => base += 1,
            Field::Bytes(2, b) if !b.is_empty() => {
                return Err("原生任务已含其他AppMsg，拒绝覆盖".into());
            }
            _ => (),
        }
    }
    if base != 1 {
        return Err("原生BaseRequest不完整或重复".into());
    }
    Ok(())
}
#[derive(Debug, PartialEq)]
pub struct Ack {
    pub server_id: u64,
}
fn tag(f: &Field<'_>) -> u32 {
    match f {
        Field::Number(n, _) | Field::Bytes(n, _) | Field::Fixed(n) => *n,
    }
}
fn text<'a>(fields: &'a [Field<'a>], tag: u32) -> Result<&'a str, String> {
    let values = fields
        .iter()
        .filter_map(|f| match f {
            Field::Bytes(n, b) if *n == tag => Some(*b),
            _ => None,
        })
        .collect::<Vec<_>>();
    if values.len() != 1 || fields.iter().filter(|f| self::tag(f) == tag).count() != 1 {
        return Err("原生响应身份缺失或重复".into());
    }
    std::str::from_utf8(values[0]).map_err(|_| "原生响应身份不是UTF8".into())
}
pub fn ack(data: &[u8], from: &str, to: &str, client_id: &str) -> Result<Ack, String> {
    server_ok(data)?;
    let f = fields(data)?;
    if text(&f, 3)? != from || text(&f, 4)? != to || text(&f, 6)? != client_id {
        return Err("原生音乐响应与本次任务不一致".into());
    }
    let ids = f.iter().filter(|f| tag(f) == 9).collect::<Vec<_>>();
    if ids.len() != 1 {
        return Err("原生响应缺少唯一服务器消息ID".into());
    }
    match ids[0] {
        Field::Number(9, n) if *n != 0 => Ok(Ack { server_id: *n }),
        _ => Err("原生响应缺少唯一服务器消息ID".into()),
    }
}
pub fn server_ok(data: &[u8]) -> Result<(), String> {
    let f = fields(data)?;
    let base = f
        .iter()
        .filter_map(|f| match f {
            Field::Bytes(1, b) => Some(*b),
            _ => None,
        })
        .collect::<Vec<_>>();
    if base.len() != 1 || f.iter().filter(|f| tag(f) == 1).count() != 1 {
        return Err("原生响应缺少唯一BaseResponse".into());
    }
    let b = fields(base[0])?;
    let ret = b
        .iter()
        .filter_map(|f| match f {
            Field::Number(1, n) => Some(*n),
            _ => None,
        })
        .collect::<Vec<_>>();
    if ret.len() != 1 || b.iter().filter(|f| tag(f) == 1).count() != 1 {
        return Err("原生响应ret缺失或重复，不能当作成功".into());
    }
    let n = ret[0];
    if n > u32::MAX as u64 && n != (n as i32 as i64 as u64) {
        return Err("微信响应ret溢出".into());
    }
    if n != 0 {
        return Err(format!("微信服务端拒绝原生任务，ret={}", n as i32));
    }
    Ok(())
}
pub fn revoke_fields(
    from: &str,
    to: &str,
    client: &str,
    local: u32,
    time: u32,
    server: u64,
) -> Result<Vec<u8>, String> {
    if local == 0
        || time == 0
        || server == 0
        || !from.starts_with("wxid_")
        || [from, to, client]
            .iter()
            .any(|s| s.is_empty() || s.len() > 512 || s.chars().any(char::is_control))
    {
        return Err("撤回来源标识无效".into());
    }
    let mut out = vec![];
    bytes(2, client.as_bytes(), &mut out);
    num(3, local as u64, &mut out);
    num(4, time as u64, &mut out);
    num(5, 0, &mut out);
    bytes(6, from.as_bytes(), &mut out);
    bytes(7, to.as_bytes(), &mut out);
    num(8, 0, &mut out);
    num(9, server, &mut out);
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revoke_preserves_own_source_and_strict_status() {
        let f = revoke_fields(
            "wxid_self",
            "wxid_peer",
            "wxid_peer_123_456",
            456,
            123,
            u64::MAX,
        )
        .unwrap();
        let f = fields(&f).unwrap();
        assert!(f.contains(&Field::Number(9, u64::MAX)));
        assert!(f.contains(&Field::Number(3, 456)));
        let mut base = vec![];
        num(1, 1u64 << 32, &mut base);
        let mut response = vec![];
        bytes(1, &base, &mut response);
        assert!(server_ok(&response).is_err());
        assert!(revoke_fields("wxid_self", "wxid_peer", "x", 0, 123, 1).is_err())
    }
    fn response() -> Vec<u8> {
        let mut b = vec![];
        let mut ret = vec![];
        num(1, 0, &mut ret);
        bytes(1, &ret, &mut b);
        bytes(3, b"wxid_self", &mut b);
        bytes(4, b"wxid_peer", &mut b);
        bytes(6, b"CaseId", &mut b);
        num(9, u64::MAX, &mut b);
        b
    }
    #[test]
    fn rejects_false_success_and_keeps_full_server_identity() {
        let b = response();
        assert_eq!(
            ack(&b, "wxid_self", "wxid_peer", "CaseId")
                .unwrap()
                .server_id,
            u64::MAX
        );
        for n in 0..b.len() {
            assert!(ack(&b[..n], "wxid_self", "wxid_peer", "CaseId").is_err())
        }
        assert!(ack(&b, "wxid_self", "wxid_peer", "caseid").is_err());
        assert!(ack(&b, "wxid_self", "wrong", "CaseId").is_err());
        assert!(ack(&[10, 0], "wxid_self", "wxid_peer", "CaseId").is_err());
        let mut d = b.clone();
        num(9, 1, &mut d);
        assert!(ack(&d, "wxid_self", "wxid_peer", "CaseId").is_err())
    }
    #[test]
    fn native_session_is_retained_and_existing_appmsg_refused() {
        let mut base = vec![];
        bytes(1, b"native-session-private", &mut base);
        assert!(native_base_only(&base).is_ok());
        let field = appmsg_field(
            "wxid_self",
            "wxid_peer",
            "<appmsg><type>3</type></appmsg>",
            "CaseId",
            1,
        )
        .unwrap();
        let app = fields(&field).unwrap();
        let Field::Bytes(2, body) = app[0] else {
            panic!()
        };
        assert!(fields(body).unwrap().contains(&Field::Number(5, 3)));
        base.extend(field);
        assert!(native_base_only(&base).is_err())
    }
}
