//! OneBot record -> bounded decode -> canonical SILK -> genuine CGI127 voice.
//! No file-message fallback, external transcoder, clipboard or UI input.
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    time::Duration,
};
const RATE: usize = 16_000;
const MAX_SECONDS: usize = 600;
const MAX_INPUT: usize = 12 * 1024 * 1024;

#[derive(Clone)]
pub struct Note {
    pub data: Vec<u8>,
    pub duration_ms: u32,
}
impl std::fmt::Debug for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoiceNote")
            .field("bytes", &self.data.len())
            .field("duration_ms", &self.duration_ms)
            .finish()
    }
}
pub fn prepare(data: &Value) -> Result<Vec<Note>> {
    let file = data["file"]
        .as_str()
        .filter(|s| !s.is_empty())
        .or_else(|| data["url"].as_str())
        .context("语音缺少file或url")?;
    let raw = load(file)?;
    let pcm = if raw.starts_with(b"\x02#!SILK_V3") || raw.starts_with(b"#!SILK_V3") {
        decode_silk(&raw)?
    } else {
        decode_audio(raw)?
    };
    encode_notes(&pcm)
}
fn load(file: &str) -> Result<Vec<u8>> {
    if let Some(encoded) = file.strip_prefix("base64://") {
        if encoded.len() > MAX_INPUT * 4 / 3 + 4 {
            bail!("语音源文件超过12MiB");
        }
        let raw = STANDARD.decode(encoded).context("语音base64无效")?;
        if raw.len() > MAX_INPUT {
            bail!("语音源文件超过12MiB");
        }
        return Ok(raw);
    }
    if file.starts_with("http://") || file.starts_with("https://") {
        let url = url::Url::parse(file).map_err(|_| anyhow::anyhow!("语音地址无效"))?;
        if !url.username().is_empty() || url.password().is_some() {
            bail!("语音地址不能包含用户凭据");
        }
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(3))
            .build()?;
        let response = client
            .get(url)
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|_| anyhow::anyhow!("语音下载失败，请检查音源是否过期或需要登录"))?;
        let mut raw = Vec::new();
        response
            .take(MAX_INPUT as u64 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| anyhow::anyhow!("语音下载中断"))?;
        if raw.len() > MAX_INPUT {
            bail!("语音源文件超过12MiB");
        }
        return Ok(raw);
    }
    let path = if file.starts_with("file://") {
        url::Url::parse(file)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .context("语音文件地址无效")?
    } else {
        std::path::PathBuf::from(file)
    };
    if !path.is_absolute() {
        bail!("语音本地路径须为微信机器上的绝对路径；远程RedReply请传base64或HTTP地址");
    }
    let source = std::fs::File::open(path).map_err(|_| {
        anyhow::anyhow!("微信机器无法读取该语音文件；远程RedReply请传base64或HTTP地址")
    })?;
    if source.metadata()?.len() > MAX_INPUT as u64 {
        bail!("语音源文件超过12MiB");
    }
    let mut raw = Vec::new();
    source.take(MAX_INPUT as u64 + 1).read_to_end(&mut raw)?;
    if raw.len() > MAX_INPUT {
        bail!("语音源文件超过12MiB");
    }
    Ok(raw)
}

// This ABI is the pinned SDK's SKP_SILK_SDK_DecControlStruct, retained in the
// bundled source headers. Drain every internal frame; the crate's convenience
// decoder does not, so packet counts are never treated as authoritative time.
#[repr(C)]
struct DecodeControl {
    api_rate: i32,
    frame_size: i32,
    frames_per_packet: i32,
    more_frames: i32,
    fec_offset: i32,
}
unsafe extern "C" {
    fn SKP_Silk_SDK_Get_Decoder_Size(size: *mut i32) -> i32;
    fn SKP_Silk_SDK_InitDecoder(state: *mut std::ffi::c_void) -> i32;
    fn SKP_Silk_SDK_Decode(
        state: *mut std::ffi::c_void,
        control: *mut DecodeControl,
        lost: i32,
        input: *const u8,
        bytes: i32,
        output: *mut i16,
        samples: *mut i16,
    ) -> i32;
}
fn decode_silk(raw: &[u8]) -> Result<Vec<i16>> {
    let mut at = if raw.starts_with(b"\x02#!SILK_V3") {
        10
    } else if raw.starts_with(b"#!SILK_V3") {
        9
    } else {
        bail!("SILK头无效")
    };
    let mut size = 0;
    unsafe {
        if SKP_Silk_SDK_Get_Decoder_Size(&mut size) != 0 || !(1..=1024 * 1024).contains(&size) {
            bail!("SILK解码器尺寸无效");
        }
    }
    let mut state = vec![0u64; (size as usize).div_ceil(8)];
    unsafe {
        if SKP_Silk_SDK_InitDecoder(state.as_mut_ptr().cast()) != 0 {
            bail!("SILK解码器初始化失败");
        }
    }
    let mut control = DecodeControl {
        api_rate: RATE as i32,
        frame_size: 0,
        frames_per_packet: 0,
        more_frames: 0,
        fec_offset: 0,
    };
    let mut output = Vec::new();
    while at < raw.len() {
        let prefix = raw.get(at..at + 2).context("SILK包长度截断")?;
        let len = i16::from_le_bytes([prefix[0], prefix[1]]);
        at += 2;
        if len == -1 && at == raw.len() {
            break;
        }
        if !(1..=5120).contains(&len) {
            bail!("SILK包长度无效");
        }
        let packet = raw.get(at..at + len as usize).context("SILK包内容截断")?;
        at += len as usize;
        let mut frames = 0;
        loop {
            let mut frame = [0i16; 960];
            let mut samples = frame.len() as i16;
            let code = unsafe {
                SKP_Silk_SDK_Decode(
                    state.as_mut_ptr().cast(),
                    &mut control,
                    0,
                    packet.as_ptr(),
                    packet.len() as i32,
                    frame.as_mut_ptr(),
                    &mut samples,
                )
            };
            if code != 0 || samples <= 0 || samples as usize > frame.len() {
                bail!("SILK音频解码失败，未发送损坏语音");
            }
            output.extend_from_slice(&frame[..samples as usize]);
            frames += 1;
            if output.len() > RATE * MAX_SECONDS {
                bail!("单次语音源超过10分钟，未截断或改为文件发送");
            }
            if control.more_frames == 0 {
                break;
            }
            if frames >= 5 {
                bail!("SILK内部帧数无效");
            }
        }
    }
    if output.is_empty() {
        bail!("语音没有可播放音频");
    }
    Ok(output)
}
fn decode_audio(raw: Vec<u8>) -> Result<Vec<i16>> {
    use symphonia::core::{
        audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
        meta::MetadataOptions, probe::Hint,
    };
    let stream = MediaSourceStream::new(Box::new(Cursor::new(raw)), Default::default());
    let mut format = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| {
            anyhow::anyhow!("语音格式无法解码；支持SILK、MP3、WAV、FLAC、AAC/M4A、Ogg/Vorbis")
        })?
        .format;
    let track = format.default_track().context("语音没有音轨")?;
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("语音音轨编码不受支持")?;
    let mut mono = Vec::<f32>::new();
    let mut rate = 0usize;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(_) => bail!("语音容器损坏或读取失败"),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = decoder
            .decode(&packet)
            .map_err(|_| anyhow::anyhow!("语音解码失败"))?;
        let spec = *decoded.spec();
        let channels = spec.channels.count();
        if !(8000..=192000).contains(&spec.rate)
            || !(1..=8).contains(&channels)
            || (rate != 0 && rate != spec.rate as usize)
        {
            bail!("语音采样参数不受支持");
        }
        rate = spec.rate as usize;
        let mut samples = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        samples.copy_interleaved_ref(decoded);
        for frame in samples.samples().chunks_exact(channels) {
            mono.push(frame.iter().sum::<f32>() / channels as f32);
        }
        if mono.len() > rate * MAX_SECONDS {
            bail!("单次语音源超过10分钟，未截断或改为文件发送");
        }
    }
    if mono.is_empty() || rate == 0 {
        bail!("语音没有可播放音频");
    }
    let count = (mono.len() * RATE).div_ceil(rate);
    let mut pcm = Vec::with_capacity(count);
    for n in 0..count {
        let pos = n as f64 * rate as f64 / RATE as f64;
        let a = pos as usize;
        let t = (pos - a as f64) as f32;
        let v = mono[a.min(mono.len() - 1)] * (1. - t) + mono[(a + 1).min(mono.len() - 1)] * t;
        pcm.push((v.clamp(-1., 1.) * i16::MAX as f32).round() as i16);
    }
    Ok(pcm)
}
fn encode_notes(pcm: &[i16]) -> Result<Vec<Note>> {
    if pcm.is_empty() || pcm.len() > RATE * MAX_SECONDS {
        bail!("语音时长须在0–10分钟内");
    }
    let mut result = vec![];
    for part in pcm.chunks(RATE * 60) {
        let padded = part.len().div_ceil(320) * 320;
        let mut bytes = Vec::with_capacity(padded * 2);
        for sample in part {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        bytes.resize(padded * 2, 0);
        let data = silk_rs_no_llvm::encode_silk(&bytes, RATE as i32, 40000, true)
            .map_err(|_| anyhow::anyhow!("SILK编码失败"))?;
        crate::voice_wire::validate_silk(&data).map_err(anyhow::Error::msg)?;
        result.push(Note {
            data,
            duration_ms: (padded * 1000 / RATE) as u32,
        });
    }
    Ok(result)
}

pub fn inspect(c: &crate::config::Config) -> Result<()> {
    if crate::music_backend::inspect(c)?.version != "4.2.0.12" {
        bail!("原生语音目前只包含已核对的微信4.2.0.12后端");
    }
    Ok(())
}
pub fn deliver(
    shared: &crate::service::Shared,
    job: &crate::bridge::Job,
    note: &Note,
    index: usize,
) -> Result<crate::actions::Tracked> {
    let c = shared.config.read().unwrap().clone();
    inspect(&c)?;
    job.check_submission(shared)?;
    let from = shared
        .staging
        .read()
        .unwrap()
        .as_ref()
        .context("本账号尚未核验")?
        .wxid
        .clone();
    let to = shared
        .peers
        .read()
        .unwrap()
        .iter()
        .find(|p| p.name == job.name && p.group == job.group)
        .context("语音目标身份尚未核验")?
        .wxid
        .clone();
    let local_id = u32::from_le_bytes(
        Sha256::digest(format!("voice:{}:{index}:{}", job.id, c.binding()?.created_at).as_bytes())
            [..4]
            .try_into()?,
    )
    .max(1);
    let sent_at = crate::config::now();
    let created = sent_at as u32;
    let client = format!("{to}_{created}_{local_id}");
    let mut upload = crate::voice_wire::VoiceUpload::new(
        &from,
        &to,
        &client,
        created,
        note.duration_ms,
        &note.data,
    )
    .map_err(anyhow::Error::msg)?;
    let path = shared
        .data
        .join("native-voice")
        .join(format!("{}-{index}.json", job.id));
    crate::config::atomic(
        &path,
        &json!({"state":"admitted","message_id":job.id,"duration_ms":note.duration_ms,"chunks":upload.request_count()}),
    )?;
    let ack = loop {
        job.check_submission(shared)?;
        let fields = upload.next_fields().map_err(anyhow::Error::msg)?;
        let response = crate::music_backend::rpc(
            &c,
            json!({"mode":"voice_chunk","from":from,"to":to,"client_id":client,"fields_b64":STANDARD.encode(fields)}),
        )?;
        if response["native_task_completed"] != true {
            bail!("语音原生任务没有确认完成，不自动重发");
        }
        let bytes = STANDARD
            .decode(
                response["response_b64"]
                    .as_str()
                    .context("语音缺少原生回执")?,
            )
            .context("语音回执格式无效")?;
        crate::config::atomic(
            &path,
            &json!({
                "state":"native_response_received", "message_id":job.id,
                "duration_ms":note.duration_ms,
                "response":crate::voice_wire::response_metadata(&bytes)
            }),
        )?;
        match upload.accept_reply(&bytes).map_err(anyhow::Error::msg)? {
            crate::voice_wire::UploadReply::More => {}
            crate::voice_wire::UploadReply::Complete(ack) => break ack,
        }
    };
    let receipt = crate::music_backend::Receipt {
        from: from.clone(),
        to,
        client_id: client,
        local_id,
        create_time: created,
        server_id: ack.server_id.to_string(),
        recalled: false,
    };
    crate::config::atomic(
        &path,
        &json!({"state":"server_confirmed","receipt":receipt,"duration_ms":note.duration_ms,"chunks":upload.request_count()}),
    )?;
    shared.event("delivery",&json!({"session":job.name,"group":job.group,"message_id":job.id,"success":true,"source":"wechat_native_voice_cgi127","duration_ms":note.duration_ms,"server_id":receipt.server_id,"text":format!("[原生语音] {:.2}秒；服务端已确认",note.duration_ms as f64/1000.)}));
    Ok(crate::actions::Tracked {
        row: crate::database::Message {
            local_id: local_id as i64,
            server_id: ack.server_id as i64,
            sort_seq: job.id as i64,
            create_time: created as i64,
            kind: 34,
            sender: from,
            content: "native-voice".into(),
            shard: "native-cgi127".into(),
        },
        runtime_id: vec![],
        sent_at,
        native_cgi: Some(receipt),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_silk_roundtrip_preserves_authoritative_duration_and_audio() {
        let pcm = (0..RATE)
            .map(|n| ((n as f32 * 440. * std::f32::consts::TAU / RATE as f32).sin() * 5000.) as i16)
            .collect::<Vec<_>>();
        let notes = encode_notes(&pcm).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].duration_ms, 1000);
        let decoded = decode_silk(&notes[0].data).unwrap();
        assert_eq!(decoded.len(), RATE);
        assert!(decoded.iter().any(|n| n.abs() > 100));
    }
    #[test]
    fn long_audio_is_complete_native_notes_not_a_file_or_silent_truncation() {
        let pcm = vec![0i16; RATE * 61 + 1];
        let notes = encode_notes(&pcm).unwrap();
        assert_eq!(
            notes.iter().map(|n| n.duration_ms).collect::<Vec<_>>(),
            [60000, 1020]
        );
        assert!(notes.iter().all(|n| n.data.starts_with(b"\x02#!SILK_V3")));
        assert!(decode_silk(b"\x02#!SILK_V3\x10\x00broken").is_err());
    }
}
