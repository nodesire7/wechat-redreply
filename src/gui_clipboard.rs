//! Clipboard access exists only for explicit Copy buttons in the settings GUI.
use crate::config;
use anyhow::{Result, bail};
use std::time::{Duration, Instant};
use windows::Win32::{
    Foundation::*,
    System::{DataExchange::*, Memory::*},
};
pub fn copy_text(text: &str) -> Result<()> {
    let raw: Vec<u8> = config::wide(text)
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    write_clipboard(13, &raw)
}
fn write_clipboard(format: u32, raw: &[u8]) -> Result<()> {
    unsafe {
        let mut opened = false;
        let until = Instant::now() + Duration::from_secs(2);
        let mut attempt = 0;
        while Instant::now() < until {
            if OpenClipboard(None).is_ok() {
                opened = true;
                break;
            }
            attempt += 1;
            std::thread::sleep(Duration::from_millis((10 * attempt).min(80)));
        }
        if !opened {
            bail!("剪贴板暂时被其他程序占用");
        }
        let result = (|| -> Result<()> {
            let mem = GlobalAlloc(GMEM_MOVEABLE, raw.len())?;
            let ptr = GlobalLock(mem);
            if ptr.is_null() {
                let _ = GlobalFree(Some(mem));
                bail!("剪贴板分配失败");
            }
            std::ptr::copy_nonoverlapping(raw.as_ptr(), ptr as *mut u8, raw.len());
            let _ = GlobalUnlock(mem);
            EmptyClipboard()?;
            if SetClipboardData(format, Some(HANDLE(mem.0))).is_err() {
                let _ = GlobalFree(Some(mem));
                bail!("剪贴板写入失败");
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}
