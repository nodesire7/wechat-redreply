#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod actions;
mod attachments;
mod bridge;
mod config;
mod database;
mod delivery;
mod diagnostics;
mod friends;
mod gui;
mod gui_clipboard;
mod gui_web;
mod lifecycle;
mod maintenance;
mod message_view;
mod music;
mod music_backend;
mod music_ui;
#[cfg(test)]
mod music_wire;
mod native;
mod outbox;
mod popup;
mod record_card;
mod service;
mod telemetry;
mod ui_readiness;

fn main() {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    if let Err(error) = service::entry() {
        eprintln!("{error:#}");
        if std::env::args().len() == 1 {
            let text = config::wide(format!("{error:#}"));
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
                    None,
                    windows::core::PCWSTR(text.as_ptr()),
                    windows::core::w!("微信 · 红色问答"),
                    windows::Win32::UI::WindowsAndMessaging::MB_OK
                        | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
                );
            }
        }
        std::process::exit(1);
    }
}
