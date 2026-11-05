//! Attachment path with no clipboard, global input, or foreground activation.
use crate::{config, delivery::Automation, native, service::Shared};
use anyhow::{Context, Result, bail};
use std::time::{Duration, Instant};
use windows::{
    Win32::{
        Foundation::*,
        UI::{Accessibility::*, WindowsAndMessaging::*},
    },
    core::BSTR,
};
fn native_image(raw: &[u8]) -> Result<(std::borrow::Cow<'_, [u8]>, &'static str)> {
    let format = image::guess_format(raw)?;
    if format == image::ImageFormat::Jpeg {
        return Ok((std::borrow::Cow::Borrowed(raw), "jpg"));
    }
    if format == image::ImageFormat::Png {
        return Ok((std::borrow::Cow::Borrowed(raw), "png"));
    }
    // The old DIB route decoded the first frame. Normalize other formats to
    // PNG so the file picker cannot reinterpret GIF/WebP/BMP as a document.
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::load_from_memory(raw)?.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok((std::borrow::Cow::Owned(encoded.into_inner()), "png"))
}
pub(crate) fn image_fingerprint(raw: &[u8]) -> Result<String> {
    use md5::{Digest, Md5};
    Ok(hex::encode(Md5::digest(native_image(raw)?.0.as_ref())))
}
impl Automation {
    pub(crate) fn attach_image(&self, shared: &Shared, h: HWND, raw: &[u8]) -> Result<()> {
        use sha2::{Digest, Sha256};
        let (prepared, extension) = native_image(raw)?;
        let raw = prepared.as_ref();
        let folder = shared.data.join("outbox");
        std::fs::create_dir_all(&folder)?;
        let file = folder.join(format!(
            "{}.{}",
            hex::encode(Sha256::digest(raw)),
            extension
        ));
        if !file.exists() {
            std::fs::write(&file, raw)?;
        }
        let path = file
            .canonicalize()?
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_owned();
        let root = self.root(h)?;
        self.click_native(&self.find(&root, UIA_NamePropertyId, "发送文件")?, h)?;
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut selected = None;
        while Instant::now() < deadline {
            for popup in native::thread_windows(h) {
                let Ok(root) = self.root(popup) else { continue };
                if unsafe { root.CurrentClassName()?.to_string() } != "#32770" {
                    continue;
                }
                unsafe {
                    let condition = self.automation.CreatePropertyCondition(
                        UIA_AutomationIdPropertyId,
                        &windows::Win32::System::Variant::VARIANT::from("1148"),
                    )?;
                    let nodes = root.FindAll(TreeScope_Descendants, &condition)?;
                    for index in 0..nodes.Length()? {
                        let node = nodes.GetElement(index)?;
                        if node.CurrentAutomationId()?.to_string() == "1148"
                            && node.CurrentControlType()? == UIA_EditControlTypeId
                        {
                            selected = Some((popup, node));
                            break;
                        }
                    }
                }
            }
            if selected.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        let (dialog, edit) = selected.context("原生文件名控件未出现；未使用剪贴板替代")?;
        unsafe {
            let pattern: IUIAutomationValuePattern =
                edit.GetCurrentPatternAs(UIA_ValuePatternId)?;
            let quoted = format!("\"{path}\"");
            pattern.SetValue(&BSTR::from(&quoted))?;
            if pattern.CurrentValue()?.to_string() != quoted {
                bail!("原生图片文件路径未完整写入");
            }
            // Directly deliver the dialog's documented IDOK command. A native
            // Button HWND receives no global mouse movement or keyboard input.
            let root = self.root(dialog)?;
            let open = self.find(&root, UIA_AutomationIdPropertyId, "1")?;
            let button = open.CurrentNativeWindowHandle()?;
            PostMessageW(
                Some(dialog),
                WM_COMMAND,
                WPARAM(1),
                LPARAM(button.0 as isize),
            )?;
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            let root = self.root(h)?;
            let edit = self.find(&root, UIA_AutomationIdPropertyId, "chat_input_field")?;
            let value = unsafe {
                edit.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)?
                    .CurrentValue()?
                    .to_string()
            };
            if value.contains('\u{fffc}') {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(80));
        }
        let mut windows = vec![];
        for popup in native::thread_windows(h) {
            let Ok(root) = self.root(popup) else { continue };
            let mut controls = vec![];
            for node in self.descendants(&root)? {
                unsafe {
                    controls.push(serde_json::json!({"class":node.CurrentClassName()?.to_string(),"name":node.CurrentName()?.to_string(),"aid":node.CurrentAutomationId()?.to_string(),"role":node.CurrentControlType()?.0}));
                }
            }
            windows.push(serde_json::json!({"class":unsafe{root.CurrentClassName()?.to_string()},"controls":controls}));
        }
        config::atomic(
            &shared.data.join("attachment-stage-probe.json"),
            &serde_json::json!({"windows":windows}),
        )?;
        bail!("图片选择已尝试，但未观察到输入框附件；未盲目发送")
    }
    pub fn file_picker_probe(&self, c: &crate::config::Config) -> Result<serde_json::Value> {
        let before = native::input_snapshot();
        let h = native::validate_window(&c.binding()?)?;
        let data = c.path.parent().unwrap().join("data");
        let _guard = crate::popup::Guard::install(h, &data)?;
        let root = self.root(h)?;
        let button = self.find(&root, UIA_NamePropertyId, "发送文件")?;
        self.click_native(&button, h)?;
        let end = Instant::now() + Duration::from_secs(4);
        let result = (|| -> Result<serde_json::Value> {
            while Instant::now() < end {
                let mut dialogs = vec![];
                for popup in native::thread_windows(h) {
                    let root = self.root(popup)?;
                    let mut controls = vec![];
                    for node in self.descendants(&root)? {
                        unsafe {
                            let role = node.CurrentControlType()?;
                            if role == UIA_ButtonControlTypeId
                                || role == UIA_EditControlTypeId
                                || role == UIA_ComboBoxControlTypeId
                            {
                                controls.push(serde_json::json!({"role":role.0,"class":node.CurrentClassName()?.to_string(),"name":node.CurrentName()?.to_string(),"aid":node.CurrentAutomationId()?.to_string()}));
                            }
                        }
                    }
                    let bounds = unsafe { root.CurrentBoundingRectangle()? };
                    dialogs.push(serde_json::json!({"class":unsafe{root.CurrentClassName()?.to_string()},"offscreen":bounds.right<0&&bounds.bottom<0,"controls":controls}));
                }
                if !dialogs.is_empty() {
                    let value = serde_json::json!({"dialogs":dialogs});
                    config::atomic(&data.join("file-picker-probe.json"), &value)?;
                    return Ok(value);
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            bail!("文件选择器未出现；未改用剪贴板")
        })();
        self.cancel_actions(h, &data);
        let after = native::input_snapshot();
        result.map(|mut v| {
            v["foreground_unchanged"] = (before.foreground == after.foreground).into();
            v["focus_unchanged"] = (before.focus == after.focus).into();
            v["cursor_unchanged"] = (before.cursor == after.cursor).into();
            v["clipboard_unchanged"] = (before.clipboard == after.clipboard).into();
            v
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn other_image_formats_become_png_before_native_file_selection() {
        let picture = image::DynamicImage::new_rgba8(3, 2);
        let mut gif = std::io::Cursor::new(Vec::new());
        picture.write_to(&mut gif, image::ImageFormat::Gif).unwrap();
        let (bytes, extension) = native_image(gif.get_ref()).unwrap();
        assert_eq!(extension, "png");
        assert_eq!(
            image::guess_format(&bytes).unwrap(),
            image::ImageFormat::Png
        );
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
    }
}
