//! Verify the native card contains every staged item, in order.
use anyhow::{Result, bail};
use serde::Deserialize;
#[derive(Deserialize)]
struct Envelope {
    appmsg: App,
}
#[derive(Deserialize)]
struct App {
    #[serde(rename = "type")]
    kind: u32,
    recorditem: String,
}
#[derive(Deserialize)]
struct Record {
    datalist: List,
}
#[derive(Deserialize)]
struct List {
    #[serde(rename = "dataitem", default)]
    items: Vec<Item>,
}
#[derive(Deserialize)]
struct Item {
    #[serde(rename = "@datatype")]
    kind: u32,
    #[serde(default)]
    datadesc: String,
    #[serde(rename = "srcMsgLocalid", default)]
    source_local: String,
    #[serde(rename = "fromnewmsgid", default)]
    source_server: String,
    #[serde(rename = "srcMsgCreateTime", default)]
    source_time: String,
}
pub fn verify(xml: &str, staged: &[crate::actions::Tracked]) -> Result<usize> {
    if xml.len() > 2 * 1024 * 1024 {
        bail!("原生合并卡超过完整性验证上限");
    }
    let outer: Envelope =
        quick_xml::de::from_str(xml).map_err(|_| anyhow::anyhow!("原生合并卡外层XML无法验证"))?;
    if outer.appmsg.kind != 19 {
        bail!("收到的不是原生聊天记录卡");
    }
    let record: Record = quick_xml::de::from_str(&outer.appmsg.recorditem)
        .map_err(|_| anyhow::anyhow!("原生合并卡的聊天记录XML无法验证"))?;
    if record.datalist.items.len() != staged.len() {
        bail!(
            "原生合并卡节点数不完整（{}/{}），不能标记完整发送成功",
            record.datalist.items.len(),
            staged.len()
        );
    }
    for (item, expected) in record.datalist.items.iter().zip(staged) {
        if item.source_local.parse::<i64>().ok() != Some(expected.row.local_id)
            || item.source_server.parse::<i64>().ok() != Some(expected.row.server_id)
            || item.source_time.parse::<i64>().ok() != Some(expected.row.create_time)
        {
            bail!("原生合并卡源消息身份与中转记录不一致，不能标记完整发送成功");
        }
        match expected.row.kind {
            1 => {
                if item.kind != 1
                    || item.datadesc.replace("\r\n", "\n")
                        != expected.row.content.replace("\r\n", "\n")
                {
                    bail!("原生合并卡中的中转文本或顺序不一致，不能标记完整发送成功");
                }
            }
            3 => {
                if item.kind != 2 {
                    bail!("原生合并卡缺少对应的中转图片，不能标记完整发送成功");
                }
            }
            _ => bail!("未知的中转消息类型，不能验证合并卡"),
        }
    }
    Ok(staged.len())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn tracked(text: &str) -> crate::actions::Tracked {
        crate::actions::Tracked {
            native_cgi: None,
            row: crate::database::Message {
                local_id: 1,
                server_id: 2,
                sort_seq: 1,
                create_time: 1,
                kind: 1,
                sender: "self".into(),
                content: text.into(),
                shard: "m".into(),
            },
            runtime_id: vec![],
            sent_at: 1.,
        }
    }
    #[test]
    fn validates_nested_cdata_and_rejects_missing_or_reordered_nodes() {
        let xml = "<msg><appmsg><type>19</type><recorditem><![CDATA[<recordinfo><datalist count=\"2\"><dataitem datatype=\"1\"><srcMsgLocalid>1</srcMsgLocalid><fromnewmsgid>2</fromnewmsgid><srcMsgCreateTime>1</srcMsgCreateTime><datadesc>A &amp; B</datadesc></dataitem><dataitem datatype=\"1\"><srcMsgLocalid>1</srcMsgLocalid><fromnewmsgid>2</fromnewmsgid><srcMsgCreateTime>1</srcMsgCreateTime><datadesc>magnet:?xt=urn:btih:AbC</datadesc></dataitem></datalist></recordinfo>]]></recorditem></appmsg></msg>";
        let expected = vec![tracked("A & B"), tracked("magnet:?xt=urn:btih:AbC")];
        assert_eq!(verify(xml, &expected).unwrap(), 2);
        assert!(verify(xml, &expected[..1]).is_err());
        assert!(verify(xml, &[expected[1].clone(), expected[0].clone()]).is_err());
        assert!(verify(&xml.replace("btih:AbC", "btih:abc"), &expected).is_err());
        assert!(
            verify(
                &xml.replace("<fromnewmsgid>2", "<fromnewmsgid>999"),
                &expected
            )
            .is_err()
        );
    }
}
