---
name: "微信 · 红色问答"
description: "A Chinese operational console with warm light/dark surfaces, red actions and green state accents."
colors:
  red: "#bd2b20"
  red-soft: "#fff0ed"
  green: "#08734f"
  amber: "#92541b"
  bg: "#f4f3ef"
  panel: "#fff"
  side: "#faf9f6"
  ink: "#24201f"
  muted: "#706761"
  line: "#dedbd5"
  hover: "#f1eee8"
  log: "#f8f7f4"
  focus: "#bd2b20"
  dark-red: "#ff8f80"
  dark-red-soft: "#3a211d"
  dark-green: "#65d8a5"
  dark-amber: "#e9b36f"
  dark-bg: "#171414"
  dark-panel: "#211c1b"
  dark-side: "#1b1716"
  dark-ink: "#f5f0ec"
  dark-muted: "#bcaca6"
  dark-line: "#493a36"
  dark-hover: "#352a26"
  dark-log: "#1a1615"
  dark-focus: "#ff8f80"
  dark-primary-ink: "#2d1511"
typography:
  headline:
    fontFamily: "'Microsoft YaHei UI', 'Microsoft YaHei', system-ui, sans-serif"
    fontSize: "25px"
    fontWeight: 600
    lineHeight: 1.3
  title:
    fontFamily: "'Microsoft YaHei UI', 'Microsoft YaHei', system-ui, sans-serif"
    fontSize: "19px"
    fontWeight: 600
    lineHeight: 1.55
  body:
    fontFamily: "'Microsoft YaHei UI', 'Microsoft YaHei', system-ui, sans-serif"
    fontSize: "15px"
    fontWeight: 400
    lineHeight: 1.55
  label:
    fontFamily: "'Microsoft YaHei UI', 'Microsoft YaHei', system-ui, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.55
  secondary:
    fontFamily: "'Microsoft YaHei UI', 'Microsoft YaHei', system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.55
  log:
    fontFamily: "'Cascadia Mono', Consolas, 'Microsoft YaHei UI', monospace"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.8
rounded:
  control: "6px"
  primary: "7px"
  card: "12px"
  pane-top: "16px"
spacing:
  field-gap: "5px"
  compact-gap: "12px"
  form-column: "16px"
  pane-inline: "24px"
components:
  button-primary:
    backgroundColor: "{colors.red}"
    textColor: "{colors.panel}"
    rounded: "{rounded.primary}"
    padding: "14px 24px"
  button-primary-dark:
    backgroundColor: "{colors.dark-red}"
    textColor: "{colors.dark-primary-ink}"
    rounded: "{rounded.primary}"
    padding: "14px 24px"
  button-secondary:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "7px 15px"
  text-field:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.control}"
    padding: "7px 10px"
  navigation-selected:
    backgroundColor: "{colors.red-soft}"
    textColor: "{colors.red}"
    rounded: "{rounded.control}"
    padding: "12px 16px"
  filter-selected:
    backgroundColor: "{colors.red-soft}"
    textColor: "{colors.red}"
    rounded: "{rounded.control}"
    padding: "5px 16px"
  account-strip:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "13px 16px"
  state-cell:
    backgroundColor: "{colors.panel}"
    textColor: "{colors.ink}"
    rounded: "{rounded.card}"
    padding: "8px 14px"
---

# Design System: 微信 · 红色问答

## Overview

**Creative North Star: "Account-first operational control desk"**

The implemented world is a warm, readable Chinese console: quiet neutral surfaces, red actions and selection, and green operational accents. Light and dark themes carry the same hierarchy. Stable navigation, real fields and a persistent action footer give a Windows operator a clear place to work. The name describes the delivered account-first composition already selected by the user; it does not introduce a new visual concept.

Actual React Bits components add restrained light to useful boundaries: SpotlightCard encloses account binding, MagicBento displays three real operational states, StarBorder marks principal actions, and CountUp presents actual queue totals. These effects sit on flat working surfaces. They do not supply invented telemetry, progress or confirmation. This system records the current React/CSS artifact inside the Rust WebView2 host, not the older native fallback.

**Key Characteristics:**

- Warm light/dark neutrals with red actions and green operational accents.
- Chinese UI typography with a compact, readable hierarchy.
- Thin dividers, line SVG icons and gently curved controls.
- Restrained React Bits effects attached to real tasks and state.
- Stable navigation and footer around a compact, independently scrolling work area.

Source authority is `design/gui-react/src/app.css`, `App.jsx`, the AI extension's `AiPage.jsx` and `ai.css`, and their imported components. `PRODUCT.md` records runtime and verification limits; `.impeccable/surfaces/react-delivery.md` records this surface's task contract. Historical native indigo documentation and pixel-comparison state are not current React visual authority. These extracted tokens do not claim a newly approved React pixel comp or live WeChat verification.

The AI extension carries the same B composition, palette, typography and component materials into a sixth destination. It changes no visual token and introduces no raster asset; the existing avatar remains unchanged. Its focused finish review returned `ship` for that surface and integration only. The captures in `.impeccable/review/ai` and QA record in `evidence/ai-gui-qa-round2/browser-qa.json` cover both themes at the normal and minimum host sizes, without whole-product reapproval, a pixel-match claim or live cloud/WeChat AI delivery evidence.

## Colors

The palette pairs warm whites or brown-charcoal grounds with a red action family and a green operational family. Frontmatter preserves the CSS source values. Unsuffixed tokens are the light theme; `dark-` tokens replace the corresponding CSS custom properties under `data-theme=dark`.

### Primary

- **Operational Red** (`red`, `dark-red`): principal actions, selected navigation/filter text, caret and action/error emphasis.
- **Soft Red** (`red-soft`, `dark-red-soft`): selected navigation and segmented filter grounds.
- **Focus Red** (`focus`, `dark-focus`): visible focus outline for ordinary controls. The StarBorder component retains its own theme track outline.

### Secondary

- **Operational Green** (`green`, `dark-green`): positive status dots and success text, account/state icons, checkbox accent and the dark-theme React Bits glow.

### Tertiary

- **Pending Amber** (`amber`, `dark-amber`): pending and uncertain queue labels, distinct from confirmation and failure.

### Neutral

- **Warm Ground** (`bg`, `dark-bg`): outer application ground.
- **Working Surface** (`panel`, `dark-panel`): main pane, inputs and operational cards.
- **Quiet Rail** (`side`, `dark-side`): sidebar and custom title bar.
- **Reading Ink / Secondary Ink** (`ink`, `muted` and dark equivalents): primary content and supporting explanations.
- **Quiet Line / Hover Wash** (`line`, `hover` and dark equivalents): field/card boundaries, table rules and secondary hover states.
- **Log Ground** (`log`, `dark-log`): selectable log-reading area.
- **Dark Primary Ink** (`dark-primary-ink`): legible lettering on the dark theme's pale red primary actions.

**The State Before Effect Rule.** Write the actual state in text; color and light may reinforce it but must not imply a send, connection or recall result.

The sidecar's eight-step tonal ramps are synthesized inspection metadata, not additional application colors. Component-local trail and border colors belong to their effects and do not establish new page accent families.

## Typography

**Body and heading family:** Microsoft YaHei UI, then Microsoft YaHei, system UI and sans-serif fallbacks.
**Log family:** Cascadia Mono, Consolas, Microsoft YaHei UI and monospace fallbacks.

**Character:** The interface is operational Chinese UI, with hierarchy established through size, weight and placement. There is no separate marketing display face or ornamental heading system.

### Hierarchy

- **Headline:** `typography.headline` for page names. The final compact override retains this size at narrow widths.
- **Title:** `typography.title` for working sections; account-strip titles use (18px).
- **Body:** `typography.body` for controls, navigation and ordinary content.
- **Label:** `typography.label` for field labels; state-cell values share this size at (600) weight.
- **Secondary:** `typography.secondary` for statuses, supporting copy, filters, table content and feedback. Compact hints and table metadata use (12px).
- **Log:** `typography.log` for selectable, wrapping records.
- **Queue totals:** (18px), (600) and tabular numerals. The count animation does not change the underlying meaning.

Field hints have a (52ch) maximum; explanatory paragraphs commonly stop at (65ch), and the advanced runtime note at (75ch). Long names and errors wrap instead of becoming a second display style.

**The One Reading Voice Rule.** Keep interface labels in the established Chinese sans hierarchy; use monospace for logs rather than decoration.

## Layout

The Rust host creates a (1160 × 780) logical-pixel window with a (960 × 680) minimum. The compact web shell uses a (44px) custom title bar above a persistent rail and working pane. The rail is (190px), becoming (166px) at widths up to (1150px). The pane has a (10px) outer top/right inset, (20px 24px 0) padding, and rounded top corners. Its content scrolls independently; the heading, feedback and footer remain in the flex shell. Feedback uses a (38px) minimum with (8px) vertical padding rather than reserving a tall notice band.

The account surface follows the selected B task order: account binding above connection settings. On wider desktops, the account identity and binding controls share one strip; at (1150px) and below they stack with a divider. The strip uses (13px 16px) padding, an (18px) gap that becomes (12px) at the compact breakpoint, and (10px) bottom margin. Three state cells follow at (58px) row height, (12px) gap and (12px) radius, with (8px 14px) content padding and (19px) space below. They show actual binding, database readiness and saved scope. The connection form uses three columns, a two-column token field and an optional WSS disclosure. Its current spacing is (12px) between rows and (16px) between columns; field labels use `field-gap`.

Scope uses two columns with a (24px) gap and proportions of (1:1.15): available contacts beside applied-name rows and immediate add/remove actions. The available list grows with content between a (120px) minimum and (310px) maximum; the applied list scrolls above (250px). Contact rows use a (45px) minimum and (7px 11px) padding. Bulk multiline editing is a collapsed disclosure with a five-row textarea. This replaces the former permanently exposed tall editor and empty list box.

Queue uses wrapping totals above filters and a horizontally scrollable table; rows use (12px) padding. Logs use a full reading area with (15px) padding below category/pause/copy controls. Advanced settings use paired fields with a (17px) gap, a separate friend-permissions section, and a collapsed compatibility table. All share the same heading/status feedback and footer.

AI starts with an enable control and compact active-profile/add-profile toolbar. Named model and key fields use the existing paired form grid; the endpoint spans its width. Context, limits and request parameters stay behind a disclosure. The test question precedes real task and short-context sections in the scrolling body. Save and save/test actions are portalled into the existing persistent footer, keeping them available when the form or task history scrolls. The toolbar and limit grid stack at the existing narrow-web breakpoint; footer actions wrap at compact widths.

At widths up to (760px), the rail reduces to (64px), navigation labels hide while accessible button names remain, forms and scope stack, and binding controls wrap. Pane padding becomes (18px 16px 0). This is additional web reflow below the supported desktop host minimum, not a separate mobile product. The final source override keeps state rows at (58px). Footer buttons can wrap; the work area remains scrollable.

**The Stable Working Frame Rule.** Preserve visible state, named navigation and available actions while the page body scrolls; use reflow before compressing Chinese controls beyond readability.

## Elevation & Depth

Working surfaces are flat at rest. Warm tonal layers and thin borders separate rail, pane, fields, tables and log area. The delivered account strip and state cells explicitly suppress their component libraries' default drop shadows. SpotlightCard and MagicBento add pointer/focus-driven radial light and edge glow; StarBorder draws a traveling edge light. These are the shipped effect vocabulary, not a general floating-card or blur-material system.

**The Useful Boundary Rule.** Apply the implemented light treatments to their working account, state and action components; keep the form and reading surfaces quiet.

StarBorder uses a (6-second) perimeter cycle, (1.2px) trail thickness, (0.4) glow and a hover/focus lap. CountUp uses (0.3-second) transitions for fetched queue totals. The account SpotlightCard uses (0.08) intensity and (0.6) border glow with flare and grain disabled. State cells enable spotlight and border glow, with tilt, intro animation and text auto-hide disabled. Reduced-motion support exists in CSS and the component implementations; still captures do not establish animation quality over time.

## Shapes

Controls use compact curved rectangles with thin borders. Ordinary controls and segmented containers follow `rounded.control`; principal StarBorder actions use `rounded.primary`; account and state cards use `rounded.card`; only the pane's top corners use `rounded.pane-top`. The brand avatar is a small rounded image tile. No general pill or hard-offset-shadow style is established.

Application icons are inline SVG line geometry on a (24-unit) grid. The normal rendered size is (21px) with (1.7) stroke width, round caps and joins. Title-bar icons are (17px), account identity (30px), and state icons (21px). Their text labels or button accessible names carry meaning; decorative SVGs are hidden from assistive technology.

## Components

### Buttons

Primary actions are confident red fields with restrained StarBorder light. The actual `StarBorder` renders a button, retaining disabled and keyboard behavior. It is used for binding and saving, including AI save; the account start action also uses the ordinary primary-button treatment. Principal StarBorder actions have (600) weight and (1.25) line-height. Ordinary buttons have a (38px) minimum height; compact table/filter controls reduce this where evidenced.

Secondary buttons use the working surface, reading ink and a one-pixel line. Hover uses the theme hover wash; press translates the control by (1px). Primary hover darkens with `brightness(.92)`. Disabled buttons use (0.5) opacity. Ordinary focus uses a (2px) outline with (3px) offset. Preserve each component's visible focus, including StarBorder's theme-track outline.

### Inputs / Fields

Actual HTML inputs, selects and textareas preserve editing, selection and browser keyboard behavior inside WebView2. Fields have a (38px) minimum height. Labels sit above them with `field-gap`; hints remain below. Textareas resize vertically and use (1.8) line-height. The token is masked by default and has an explicitly named reveal action. Saving disables editable fields. The music-card cell is static label and secondary copy in the existing two-column grid, explaining the current-song page, audio, QQ track identity and source authorization; it has no editable link-mode selector.

The token field groups its input and reveal button within one border. WSS stays in a native HTML disclosure. The form's wrappers do not replace functional labels with decoration.

The AI API-key field is a separate write-only password input with no saved-key reveal action. A saved-key hint and placeholder communicate blank-to-retain behavior; deletion is an explicit pending change applied on save. Required-key and local/intranet HTTP options remain written checkbox controls beside the field. AI refreshes update task/context state without replacing unsaved fields, and page navigation retains the mounted form.

### Navigation

Six left-rail buttons pair SVG icons and Chinese names: account/access, scope, AI models, queue, logs/conversations and advanced settings. Selected navigation uses the theme soft red ground and red ink at (600) weight. Buttons use `aria-current=page`; the compact rail preserves their accessible names. The title bar has explicit theme, minimize, maximize/restore and close controls backed by the Rust host.

### Filters / State Labels

Segmented filters have a thin rounded outer border, (3px) inset and (2px) gaps. Selected buttons use the same soft-red/red pairing as navigation with `aria-pressed`. Status text pairs a small (7px) neutral or green dot with a written state. Queue labels use green for confirmed, red for errors, amber for pending/uncertain and neutral ink for cancelled/recalled states. These are reading labels, not decorative success badges.

### Account Strip and State Cells

SpotlightCard encloses the real account identity, selected window and refresh/bind controls; the persistent border and field labels remain readable without a hover. MagicBento uses custom content for three actual state cells. Its library demo charts, brands and generic content are not used in this console. Scope counts derive from the saved configuration, and contact membership distinguishes pending edits from applied scope.

### Applied Scope and Compatibility Detail

Applied scope uses compact named rows with a direct remove action. Manual addition pairs one field with its save-integrated action; the bulk editor stays in an ordinary HTML disclosure. Addition and removal update the saved snapshot only after the Rust save succeeds. Empty states use content-sized explanations rather than a permanently tall blank editor.

Advanced compatibility detail is a real backend-fed table behind a named disclosure. It lists operation, actual behavior and status, then unsupported actions. Cells use (12px) text with (9px) padding; the table wraps long content. Capability and validation status remain written text, including the voice row's explicit supported-version boundary. Transport and recipient evidence belong in `PRODUCT.md`, rather than changing visual success rules.

### Queue and Logs

Queue totals use CountUp only after a successful backend statistics snapshot; unavailable data renders an em dash and explicit error. The table retains real rows while cancellation/recall is pending and explains uncertain or failed results. No removal animation confirms an operation. Recent history is limited to (200) rows by the Rust endpoint.

Logs use a selectable monospace preformatted area with wrapping, a line border and theme-specific log ground. Automatic refresh runs every (2 seconds); pausing still allows an explicit category fetch. Category and displayed data must agree before copying is enabled. A failed poll preserves the last data and reports that it is stale.

### AI Models, Tasks and Short Context

The AI page extends the existing operational form rather than adding a chat-client visual system. Its enable text states that RedReply invokes generation explicitly. Current profile selection, provider presets and add/delete actions edit saved configuration; presets do not imply a successful connection. Shared context and request limits are distinguished from per-profile prompt and model parameters in the collapsed disclosure.

The latest (100) actual tasks appear in a bordered reading table with written states, optional provider usage, task IDs and expandable wrapping results on the existing log ground. Model-test completion, generated text awaiting submission, sending-queue submission and confirmed delivery stay visibly distinct. Cancel is available only for eligible unsubmitted tasks; submitted work directs operators to the sending queue. Short-context rows identify conversation/member/profile, with per-member and confirmed all-context clearing. Empty states explain that model tests and unsent results do not create delivered context. No sample chat rows, fabricated usage or animated delivery confirmation is part of this surface.

### Persistent Footer and Feedback

The footer has a line border, (69px) minimum height and (13px 0 16px) padding. Account start/stop sits to the left; save appears on editable pages. The feedback area reports pending edits, successful actions and errors through readable text and an `aria-live` region. Save/validation is integrated with dependent operations. Page navigation retains drafts; closing asks before discarding edits. Backend polling must not turn draft values into operational truth.

On the AI page, the footer hosts the StarBorder save action, secondary save/test action and saved/unsaved status. Testing saves and validates first, then reports an asynchronous task ID rather than a connection-success assumption. The task result remains in the scrolling body. These footer controls share the existing busy/disabled feedback and theme treatment.

The sidecar provides self-contained HTML/CSS previews of these primitives. Canvas/React motion and operational handlers are not recreated by a static preview. Examples use generic labels, explicitly marked style-preview content or neutral stopped/unbound state; they contain no private tokens, contacts or chat history.

## Do's and Don'ts

### Do:

- **Do** keep the current warm light/dark red, white and green identity.
- **Do** use the Chinese UI hierarchy, thin lines and inline SVG icons.
- **Do** bind React Bits effects to the existing working components and real state.
- **Do** preserve explicit unknown, stale, pending, failed and confirmed feedback.
- **Do** keep keyboard names, visible focus and the scrolling work area usable in both themes.

### Don't:

- **Don't** restore the historical indigo/native system as the current React design authority.
- **Don't** add fabricated metrics, sample production rows or animation that implies sending or recall success.
- **Don't** use unsaved scope edits as an applied operational state.
- **Don't** spread demo charts, decorative progress or library default content into working forms.
- **Don't** treat static GUI captures or this token record as proof of native playback or live WeChat delivery.
