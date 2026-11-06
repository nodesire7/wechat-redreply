# React delivery console

Mode: Operate. Target: design/gui-react/src/App.jsx, src/gui_web.rs, assets/gui/next.html.

The user rejected the November 4 console as cluttered and not a deliverable: navigation, displayed metrics and actual behavior did not agree. They explicitly chose the CURRENT RED/WHITE/GREEN palette and retained light/dark themes, then asked for MORE reactbits.dev styling. Keep the previously selected B task composition: account binding above connection configuration, account first, minimal steps. This is a functional refinement of the existing React console with that palette, not a claim that the old native indigo comp has passed its historical pixel gate.

Use real React Bits components: SpotlightCard for the working account strip, MagicBento for actual binding/database/scope state, StarBorder for principal actions, CountUp for actual queue counts. No fake trends, decorative 100s/120s progress, invented telemetry, demo rows in production, or animation that pretends a recall succeeded. No Shredder removal of a still-pending real message.

All five routes must work within this GUI: account/access, scope, queue, logs, advanced. Separate and search private contacts/groups; preserve exact names and existing token/IDs. Copy and start save current settings first; failed validation prevents the dependent action. Background polling must not erase drafts/errors. Queued/uncertain/recalling/failed/recalled are distinct. Final recall needs native confirmation; GUI writes a request and keeps the row visible.

Quality bar: completed operational Windows tool, truthful state and feedback, coherent restrained React Bits effects, keyboard names, readable Chinese, both themes at 1160x780 and the minimum 960x680 host. Smaller responsive behavior may collapse labels but must retain accessible names. User-input errors, offline, empty queue, failed/uncertain records and long names stay usable. No personal account binding or local listener startup during QA. The user requires native voice bubbles; audio files are not an acceptable fallback. Compact scope rows add/remove immediately, while bulk editing stays collapsed until requested.

Verification uses an isolated unbound Rust GUI and real loopback backend. Synthetic queue rows explicitly labeled as QA data exercise the actual SQLite/API path; they do not establish live WeChat delivery. Production native send/music verification is separate. No private config, tokens, business plugin code, signed audio URLs or chat history may ship.

## Direction contract — optional AI extension

THESIS: Make saved model configuration and real asynchronous work understandable in the existing console. This is an extension of the chosen B layout, not a new visual identity.

OWN-WORLD: Preserve the committed warm red/white/green tokens, both themes, Chinese system typography, compact form rhythm, sidebar and existing React Bits primary action treatment.

STORY: Choose or add a model profile, enter endpoint/model/key, save and test with one action. RedReply explicitly invokes generation; the page must not imply automatic takeover. Inspect actual task state, cancel unsubmitted work and clear short context.

FIRST VIEWPORT: AI navigation opens a compact enable/profile toolbar above endpoint, model and write-only key fields. Context and limits are collapsed advanced settings. The primary save/test action and test result precede the real task list; no invented counters or demo messages ship.

FORM: Extend the incumbent operational form composition. Existing approved B composition supplies the structure; no concept selection or new imagery is required. Unsaved secrets/drafts survive polling, all buttons have truthful busy/error/disabled states, generated and delivered are distinct.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
