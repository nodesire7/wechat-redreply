# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Operators of an existing RedReply installation who want its private and group chat plugins to handle selected conversations in one logged-in Windows WeChat account. The interface uses Chinese operational language.

## Product Purpose

Bind the intended WeChat account, choose the conversations it may handle, connect a local or remote RedReply instance through OneBot11, and inspect actual sending and recall outcomes in one Windows control window.

## Operating Context

The shipped interface is a local React web UI inside a Windows Rust host using the system WebView2 Runtime. Its bundled HTML, JavaScript, CSS and image assets are served over an authenticated loopback bridge; privileged operations remain in Rust. It does not load a hosted website or require a Node.js/pnpm runtime. WebView2 is a system prerequisite; if the web shell cannot initialize, the host falls back to the older native interface. That fallback is a separate surface and does not define the current React design system.

Windows, a logged-in compatible WeChat client and an existing RedReply service are external prerequisites. Configuration and local data live beside the executable. Building from source requires the documented Windows Rust/C++ tools, Node.js 22 and pnpm 10. The ordinary window opens at 1160 × 780 logical pixels with a 960 × 680 minimum.

GUI layout verification uses an isolated unbound backend and does not authorize personal account binding or listener startup. Separate explicitly requested recipient tests are native-delivery evidence, not GUI-layout evidence. A remote production RedReply service exists separately; its existence alone does not establish a GUI connection or send result.

## Capabilities and Constraints

- Five working pages expose account/access, private/group scope, durable sending queue, logs/conversations and advanced settings. All displayed operational values come from the Rust bridge, saved configuration or the durable queue.
- Account binding targets one selected logged-in WeChat window. Binding and automated operation remain account scoped; account changes invalidate the previously read contact list.
- Exact private and group names control separate whitelists. Reading/searching contacts and manual name entry are supported. Compact applied-name rows expose immediate saved removal; contact/manual addition also saves immediately. Bulk multiline editing is behind a collapsed disclosure and requires saving. Saved scope is distinct from unsaved edits; empty saved lists handle no conversations of that type.
- Connection configuration covers listen/connect addresses, port, robot ID, a masked token, optional management URL and optional WSS certificate/key paths. Validation and saving precede copying the OneBot address or starting the listener. Binding and contact addition save their resulting settings. Connection changes can restart an already running listener; scope and polling settings apply without that connection restart.
- Polling preserves configuration drafts and reports stale or unavailable state. Queue totals show an unavailable marker when no successful statistics snapshot exists. A queue item is not a confirmed send: queued, preparing, sending, confirmed, failed, uncertain, cancelled and recall states remain distinct.
- Queue cancellation/recall requests use the real durable backend. A request stays visible until its result is known. Recall is limited to explicit adapter outputs and the smaller of the configured limit and WeChat's 120-second ceiling; a GUI acknowledgment alone does not confirm native recall.
- Logs support category selection, pausing automatic refresh and explicit copying. Changing category fetches that category even while paused; stale responses cannot relabel the displayed records. Copying is an explicit user action and may contain sensitive data.
- Advanced settings cover database polling, recall limit, optional self-chat staging override, music-card link mode and automatic friend approval with chat-only permissions. An expandable compatibility table reads the actual Rust capability mappings and unsupported actions. New friends do not automatically join the whitelist. Security challenges remain manual in WeChat.
- Existing backend paths include text/images, native music cards and merged forwarding, with version and content restrictions. Mentions appear as text. Video, files and QQ management actions remain unsupported. Native music uses checked module profiles for WeChat 4.2.0.9 and 4.2.0.12; client upgrades require revalidation. Complete metadata and usable audio authorization are still required; no universal song-ID lookup or unrestricted music source support is implied. Music preserves separate current-song page and audio URLs and fills the native playback identifier only from a verified current QQ songmid. The GUI does not expose an unverified click-mode switch; current recipient playback repair remains unverified.
- Voice `record` must produce a genuine native WeChat voice bubble. The implemented CGI127 path is limited to the exact checked WeChat 4.2.0.12 profile and never falls back to a file message. Preparation decodes supported audio into SILK, bounds input to 12 MiB and 10 minutes, and splits long sources into notes of at most 60 seconds within the same output. Current tests confirm 1-second and 12-second native sends and subsequent native recalls on that profile, including the original RedLang → remote OneBot WebSocket → durable queue → native voice → explicit RedLang recall path. Recipient playback is independently confirmed only for the 1-second bubble. Other OS/client versions and integrated over-60-second note segmentation remain unverified.
- Automated messaging does not use the global clipboard, mouse or foreground focus. Offscreen operation stays enabled. Preserve existing configuration bytes during deployment and exclude private configuration, tokens, signed audio URLs and chat history from deliverables.

## Brand Commitments

The user retained the current red/white/green identity and both light and dark themes, and requested more reactbits.dev styling. The selected B task composition remains account first: bind the account above connection setup, keep five stable navigation destinations, and integrate saving into dependent actions to reduce steps. The subsequent density refinement removes excessive empty space through a smaller default window, tighter task spacing, compact applied-scope rows and disclosure of bulk/compatibility detail.

The delivered React UI uses actual SpotlightCard, MagicBento, StarBorder and CountUp components. They frame real account binding, database readiness, saved scope, principal actions and queue counts. Their effects support readable Chinese controls and truthful feedback; they must not introduce sample telemetry, fabricated progress or success implied by an animation.

## Evidence on Hand

Current implementation: `design/gui-react/src/App.jsx`, `design/gui-react/src/model.js`, `design/gui-react/src/app.css`, their local React Bits components, and the Rust bridge in `src/gui_web.rs`. The built React page is embedded at `assets/gui/next.html`. Durable visual rules are recorded in `DESIGN.md` and its extension sidecar; the delivery-specific contract is `.impeccable/surfaces/react-delivery.md`.

The current named captures in `.impeccable/review/delivery` show all five pages at 1160 × 780 in both themes and at the 960 × 680 minimum in dark theme. The independent final review (`final-review.md`, disposition `ship`) opened all 15 individual page captures plus `logs-paused-filter.jpg` and found the six previous material issues resolved. These captures replaced the earlier larger-window files. The older `logical` captures remain historical native evidence and do not validate the current React layout. Isolated GUI captures and explicitly labeled synthetic queue rows exercise the actual UI/SQLite/API path without establishing live WeChat sending, recall or receiver-side music playback.

Separate native voice tests on the checked WeChat 4.2.0.12 profile produced server-confirmed 1-second and 12-second sends, followed by confirmed native CGI594 recall after 30 seconds. The 12-second note used two upload chunks for one voice bubble; this does not establish over-60-second note segmentation. The recipient independently confirmed that the first 1-second bubble was audible; 12-second recipient playback was not independently confirmed. The host's final-response flag handling was corrected and no response-field mismatch remains in these tests. Clipboard, cursor, focus and foreground state stayed unchanged. An offline 61-second split test passes; broader OS/client coverage and live long-source segmentation remain separate validation work.

The current candidate portable EXE (SHA256 `DAF4EEF40DA5F01D3ED7BA5D8430A54A09060C93EE38972EBDB1E48E53C7756B`) also passed the complete original RedLang output-stream voice flow through the remote production server's OneBot WebSocket, durable queue and native backend during an isolated candidate-service window. A 12-second WAV received a queue acknowledgment in 93 ms, then a server-confirmed native type-34 send; an explicit RedLang recall of that output ID reached confirmed recall. The acknowledgment is enqueue evidence, separate from the later send result. The previously installed listener was restored ready and connected in cleanup, with production configuration bytes unchanged. A native type-49 music card sent through the actual RedLang output stream was also server confirmed. It had no recall marker or explicit delete request and was retained for recipient playback checking; that playback confirmation remains pending.

The earlier candidate portable package passed installation, relocation, token and configuration/data-preservation checks with 73 Rust tests passed, two external-fixture tests ignored, and four frontend tests passed. These results do not validate a newer distribution package.

The November 5 server hotfix uses native CGI594 for verified own text, image and card receipts, processes the oldest physical part first, and persists submission/confirmation independently of database changes. Two real RedLang/OneBot runs confirmed selective recall of a progress message in approximately 24–27 seconds and all three physical parts of an at/image/text output in approximately 98–102 seconds. A normal sibling output remained unscheduled until explicit test cleanup. The latest deployed EXE SHA256 is `6F8EF47862A199DB8935C416B44791451E07784D5A94CF3E4E1A9C12BBC93A91`; exactly one settings process and listener are running, the listener is ready and connected, and production configuration bytes were preserved. The current workspace checks pass 81 host and 13 helper tests, with two external-fixture host tests skipped. The current binary has not been published as a Release, and recipient music playback repair is still unverified.

Historical native indigo mockups and `.impeccable/build/state.json` describe an earlier surface. They are not a current React pixel comparison authority, and this refinement does not close or bypass their historical acceptance gate. No newly approved React pixel comp is claimed.

## Product Principles

- Show the account, current saved scope and next available action clearly.
- Save and validate before an action that depends on edited settings.
- Treat unknown, stale, queued and uncertain results explicitly.
- Confirm sending and recall through backend evidence, never a visual effect.
- Keep local controls keyboard accessible and both themes readable.
- Deliver requested voice as native bubbles; fail explicitly rather than substitute files.
