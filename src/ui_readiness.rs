//! Target preparation must not assume that a new account already has a chat
//! open. No message is submitted until the selected target and input verify.
use anyhow::{Context, Result};
/// Only constructed at a boundary before any physical message submission.
/// Queue handling also checks that no earlier part was already recorded.
#[derive(Debug)]
pub struct NotSubmitted(String);
impl std::fmt::Display for NotSubmitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for NotSubmitted {}
pub fn not_submitted(reason: impl std::fmt::Display) -> anyhow::Error {
    NotSubmitted(reason.to_string()).into()
}
pub fn prepare<T>(
    initial_title: Result<String>,
    select: impl FnOnce() -> Result<T>,
    verify: impl FnOnce() -> Result<()>,
) -> Result<T> {
    // A fresh login or a conversation list can legitimately have no current
    // title. Selecting the explicit target is what creates that chat view.
    let no_initial_chat = initial_title.is_err();
    let selected = select().with_context(|| {
        if no_initial_chat {
            "当前尚未打开会话，自动选择目标失败"
        } else {
            "选择目标会话失败"
        }
    })?;
    verify()?;
    Ok(selected)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    #[test]
    fn fresh_window_without_current_chat_can_select_and_verify_its_target() {
        let selected = Cell::new(false);
        let result = prepare(
            Err(anyhow::anyhow!("no chat selected")),
            || {
                selected.set(true);
                Ok(())
            },
            || {
                anyhow::ensure!(selected.get(), "No target selected");
                Ok(())
            },
        );
        assert!(
            result.is_ok(),
            "A missing initial title must not prevent selecting the requested chat: {result:?}"
        );
    }
    #[test]
    fn target_or_input_failure_never_passes_preparation() {
        let verify_called = Cell::new(false);
        assert!(
            prepare(
                Ok("old chat".into()),
                || Err::<(), _>(anyhow::anyhow!("ambiguous target")),
                || {
                    verify_called.set(true);
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!verify_called.get());
        assert!(
            prepare(
                Ok("old chat".into()),
                || Ok(()),
                || Err(anyhow::anyhow!("wrong target or missing input"))
            )
            .is_err()
        );
    }
}
