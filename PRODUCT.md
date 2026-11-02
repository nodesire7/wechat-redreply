# Product

<!-- impeccable:product-schema 1 -->

## Platform

Windows desktop, native Rust / Win32. This existing platform is retained; this is not a web, iOS or Android application.

## Users

People running an existing RedReply who want its private and group chat plugins to handle selected WeChat conversations. The current operator prioritizes account and connection setup as the opening page.

## Product Purpose

Bind one logged-in Windows WeChat account, configure allowed conversations, connect local or remote RedReply over OneBot11, and observe actual delivery results.

## Operating Context

The portable Rust EXE includes its helper and runtime components. Windows, a logged-in WeChat client and RedReply are external prerequisites. Current native features were validated on WeChat4.2.0.9. Configuration and local data live beside the executable.

## Capabilities and Constraints

Preserve account binding, private/group whitelists, connection/token/WSS settings, offscreen operation, start/stop, logs, durable queue, native music/merged forwarding, trigger-scoped recall limited to120 seconds and friend approvals with chat-only permissions. Unknown send outcomes must not look successful. Never expose tokens or use global clipboard, mouse or foreground focus for automated messaging. Copy buttons act only on user request. Preserve current configuration bytes when deploying a GUI update. Listening and the bound robot WeChat are currently stopped by explicit user request; do not restart them during design work.

## Brand Commitments

The user requested a modern GUI referencing agalwood/Motrix. Keep the product's Chinese operational language and existing functionality. Default opening surface is account and connection configuration. The user chose visual mockups before implementation.

## Evidence on Hand

Existing implementation src/gui.rs and underlying config/queue/service modules; original native UI screenshots in the private sibling development tree; Motrix official screenshots referenced from its public repository. Mockups use clearly representative loopback addresses and stopped/unbound state, never private account data.

## Product Principles

- Configuration should explain the next available action.
- A queued response is not a confirmed send.
- One explicit bound account and whitelist control all automation.
- Local operator controls remain predictable and keyboard accessible.

