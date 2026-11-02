---
name: "微信 · 红色问答"
description: "A calm native Windows control desk for account binding and RedReply connection."
colors:
  primary: "#4949DD"
  ink: "#192029"
  muted: "#596974"
  sidebar: "#E6F2FB"
  working-pane: "#FFFFFF"
  line: "#D5DCE3"
  selected-fill: "#D2E4F9"
  selected-ink: "#2E399C"
  hover-fill: "#E5EFF9"
  pressed-fill: "#E7EEF4"
  primary-pressed: "#3533BB"
  disabled-fill: "#E5EAEE"
  disabled-ink: "#74828D"
typography:
  headline:
    fontFamily: "Microsoft YaHei UI"
    fontSize: "36px"
    fontWeight: 600
  title:
    fontFamily: "Microsoft YaHei UI"
    fontSize: "30px"
    fontWeight: 600
  body:
    fontFamily: "Microsoft YaHei UI"
    fontSize: "20px"
    fontWeight: 400
  secondary:
    fontFamily: "Microsoft YaHei UI"
    fontSize: "20px"
    fontWeight: 400
rounded:
  field: "4px"
  button: "5px"
  account-strip: "7px"
spacing:
  pane-inset: "28px"
  column-gap: "18px"
  navigation-inset: "12px"
components:
  button-primary:
    backgroundColor: "{colors.primary}"
    textColor: "{colors.working-pane}"
    typography: "{typography.body}"
    rounded: "{rounded.button}"
    height: "52px"
  button-primary-active:
    backgroundColor: "{colors.primary-pressed}"
    textColor: "{colors.working-pane}"
    rounded: "{rounded.button}"
  button-secondary:
    backgroundColor: "{colors.working-pane}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.button}"
    height: "52px"
  button-secondary-hover:
    backgroundColor: "{colors.hover-fill}"
    textColor: "{colors.ink}"
    rounded: "{rounded.button}"
  button-secondary-active:
    backgroundColor: "{colors.pressed-fill}"
    textColor: "{colors.ink}"
    rounded: "{rounded.button}"
  button-disabled:
    backgroundColor: "{colors.disabled-fill}"
    textColor: "{colors.disabled-ink}"
    rounded: "{rounded.button}"
  text-field:
    backgroundColor: "{colors.working-pane}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.field}"
  navigation:
    backgroundColor: "{colors.sidebar}"
    textColor: "{colors.ink}"
    typography: "{typography.body}"
    rounded: "{rounded.button}"
    height: "54px"
  navigation-selected:
    backgroundColor: "{colors.selected-fill}"
    textColor: "{colors.selected-ink}"
    rounded: "{rounded.button}"
    height: "54px"
  account-strip:
    backgroundColor: "{colors.working-pane}"
    textColor: "{colors.ink}"
    rounded: "{rounded.account-strip}"
---

# Design System: 微信 · 红色问答

## Overview

**Creative North Star: "Motrix-style account-first control desk"**

A pale-blue navigation rail and a spacious white working pane make Chinese account and connection settings easy to scan. A narrow pale-blue band crosses above the rounded white pane, as in the approved B composition. Restrained indigo actions, charcoal text and small geometric line icons carry the user's pinned Motrix reference into a native Rust / Win32 application delivered as one portable EXE. The surface works in Operate mode: account identity, connection identity and actual running state guide the next action.

This records the implemented system in `src/gui.rs` and `src/gui_theme.rs`, using `PRODUCT.md` and the account surface brief for durable product constraints. The approved option B composition places the account strip above connection settings and opens on “账号与接入”. It favors saving as part of the requested action. Native Windows chrome, selection, dropdowns, keyboard behavior and editable text remain real controls. There is no browser runtime, shipping UI raster or decorative animation.

**Key Characteristics:**

- Pale-blue side navigation, white working pane and restrained indigo actions.
- Chinese Microsoft YaHei UI typography with clear page and section hierarchy.
- Geometric GDI line icons paired with real native text and controls.
- Persistent operation footer and explicit bound, connected and stopped states.
- Compact responsive desktop settings with action-integrated saving.

**Implementation and acceptance are separate.** These source tokens describe the current native artifact; they do not certify pixel equivalence to the approved comp. Surface-specific review findings, comparison scores and acceptance belong in the account surface and finish-review artifacts, outside these durable tokens. The implemented native title bar, control geometry and flat white canvas must be disclosed when comparing to a comp. No HTML/CSS detector ran because it is inapplicable to this native UI. Review captures and the comp are internal inspection evidence, not shipped assets or examples for publication.

## Colors

The palette combines an icy blue rail, neutral white surfaces and a single blue-violet action accent. Frontmatter values are RGB hex, decoded from Win32 `COLORREF` BGR integers; screenshot sampling does not override source values.

### Primary

- **Restrained Indigo** (`primary`): binding and “保存并应用”, with white labels and line icons. Pressing uses `primary-pressed`; hovering retains the primary fill.

### Neutral

- **Charcoal Ink** (`ink`): headings, field contents and standard action labels.
- **Slate Secondary** (`muted`): explanations, binding/connection status and footer notice text.
- **State Dot** (`#A89A8C`): two small painted circles beside the account and RedReply state labels, matching the B comp's neutral status marker.
- **Icy Navigation Blue** (`sidebar`): the persistent left rail.
- **Working White** (`working-pane`): main canvas, native text-field backgrounds and flat containers.
- **Quiet Divider** (`line`): field frames, secondary button outlines, account strip and footer rule.
- **Selection Blue / Selection Ink** (`selected-fill`, `selected-ink`): selected navigation and active log filters.
- **Hover / Press / Disabled**: the corresponding state tokens apply to owner-drawn buttons; disabled content uses `disabled-ink` on `disabled-fill`.

**The Action Accent Rule.** Use indigo for the implemented primary actions and selected navigation ink treatment; keep operational reading surfaces light and neutral.

The sidecar's eight-step tonal ramps are synthesized preview-only metadata. They are not additional native palette tokens and must not be introduced into the application as established colors.

## Typography

**Display Font:** Microsoft YaHei UI for application page headings.
**Body Font:** Microsoft YaHei UI for fields, buttons, navigation and Chinese operational copy.

**Character:** One Chinese sans family provides the entire application hierarchy. Native title-bar text belongs to the Windows non-client area and does not establish an application display face. No custom fallback stack, tracking or line-height token is implemented.

### Hierarchy

- **Headline** (600): page heading, using `typography.headline`.
- **Title** (600): “微信账号” and “RedReply 连接”, using `typography.title`.
- **Body** (400): ordinary labels, controls and navigation, using `typography.body`.
- **Secondary** (400): supporting explanations and status, using `typography.secondary`; it has the same source size as body and differs through muted color and placement.

The frontmatter sizes are logical pixel heights. A shared `FONT_SIZES = [20, 36, 30, 20]` drives both startup and `WM_DPICHANGED`; the native font height is the negative role height multiplied by DPI/96. Keep this relationship when adding controls. Font metrics and line wrapping remain native.

**The One Chinese Voice Rule.** Use the same Chinese family for application labels and actions; establish hierarchy through the existing size, weight, color and placement roles.

## Layout

The native window defaults to (1280 × 900 logical pixels), with a minimum tracked outer size of (1020 × 800). The comp comparison uses an alternate (1536 × 1024) capture; it is review evidence, not a second default. All client layout calculations divide physical bounds by DPI scale before placing scaled controls.

For logical client width `w`, the rail is (192px) below `w = 1100`; at and above that threshold it is `min(round(w / 6), 264)`. The working pane begins at `rail + pane-inset` and retains the same inset at the right. A (16px) pale-blue band runs above the white pane, whose top corners use an (18px) rounded clip; this is the B composition and does not move native controls. Navigation rows start at (52px), advance by (62px), and have the tokenized row height, with the tokenized rail inset on both sides. Page order is account, scope, queue, logs, advanced.

The account/connection surface uses `v = clamp((clientHeight - 140) / 674, 0.90, 1.35)` for vertical placement. The account strip starts at `167 × v`; its height is `124 × v` with WSS collapsed and `96 × v` expanded, and a vertical divider separates identity from selection at 35.5% of the strip width. Its account identity stays to the left of window selection, refresh and binding. The selector keeps native ellipsis and a widened dropdown for long window labels. Only one selection/refresh/bind row is shown; the B comp's separate status dots are painted geometry in the rail-to-pane band.

Connection fields use a three-column first row (listen IP, connection address, port), a two-column second row (robot ID and masked token), and a management-address row with its copy action. The tokenized column gap is shared by these rows and the paired whitelist editors. Nominal single-line field height is `36 × v`; optional WSS fields use `34 × v`. Opening WSS moves the section start from `336 × v` to `280 × v` and tightens row offsets so certificate and key fields fit above the persistent footer.

The footer rule sits (116px) above the client bottom, with a thin line that separates it from the connection area. Notice text sits at `h - 128`; operation buttons sit at `h - 95` with (52px) height. Start/stop remain left; reload/save remain right, with the primary save action at the B comp's 180px width. Sidebar running status stays near its lower edge. Scope, advanced, logs and queue share this shell and footer; they use their own native content placements rather than inheriting the account form's vertical formula.

**The Visible Operation Rule.** Keep state, the next available action and the persistent footer readable across the supported desktop sizes, including expanded WSS.

## Elevation & Depth

Surfaces are flat. Tone separates the rail from the working pane; thin outlines distinguish fields and the account strip; a horizontal rule separates the footer. There are no custom shadows, gradients, floating cards, blur materials or motion tokens in this implementation. Windows may render its own non-client chrome outside this paint system.

**The Flat Working Surface Rule.** Preserve the implemented tonal layering and thin rules; do not add decorative elevation to settings or monitoring surfaces.

## Shapes

The application uses restrained curved rectangles. Frontmatter radii describe the equivalent corner radii of GDI `RoundRect` ellipse diameters: fields (8px diameter), buttons (10px diameter), and the account strip (14px diameter), all scaled by DPI. GDI rounding and native control edges need not match CSS rasterization exactly. Field and secondary action borders use a one-device-pixel GDI pen. The account icon tile uses a (12px) ellipse diameter and a pale fill specified in the source, with an account line icon rather than an image avatar.

Icons are geometric GDI strokes on a (24-unit) drawing grid. Navigation icons render at (21 logical pixels), action icons generally at (18), and the reveal-token icon at (20). Text remains native text; do not replace it with rasterized screen imagery or glyph icons.

## Components

### Buttons

Quiet native buttons communicate availability immediately. Owner drawing retains a Windows `BUTTON`, its Chinese text name and dialog keyboard behavior. Footer buttons use the documented height; binding, refresh, field-adjacent actions and filters use their observed compact heights. Shapes follow the button token.

Primary fill is used by “绑定账号” and “保存并应用”. Secondary buttons use working white, ink and a divider outline. Ordinary secondary hover and press use their state tokens; selected navigation and active filters use selection tokens. Disabled buttons use disabled tokens. Focus draws the native dotted focus rectangle inset (4 device pixels) from the owner-drawn bounds. State changes repaint immediately; no animation is implemented.

### Inputs / Fields

Native `EDIT` controls keep text selection, caret movement and dialog navigation. A GDI field frame supplies the thin outline and gentle corners; `field()` places the edit (9px) inside the nominal horizontal bounds and (4px) vertically. The frame extends around that edit; avoid treating the helper's offsets as a CSS padding contract.

The access token is masked by default and revealed only through its explicit eye button, whose native text name is “显示令牌”. Do not put real tokens into examples, previews or documentation. Account and contact selectors are native dropdown lists. Account rows are owner-drawn with ellipsis and native focused/selected state; the dropdown itself stays a native control.

### Navigation

Five persistent left-rail buttons pair geometric icons with Chinese labels. The selected row has the selection fill and ink; neutral rows inherit the rail. Owner-drawn hover, press, disabled and focus states remain visible. `IsDialogMessageW` handles dialog navigation; hidden page controls are skipped. Keep meaningful native names and visible labels for keyboard and UI Automation use.

### Account strip and connection settings

The stacked account strip is the chosen option B expression, with explicit binding and RedReply connection text above it. Binding validates the chosen logged-in window, saves that binding and collected settings, and reports the actual result. The connection area contains editable parameters and optional WSS disclosure. Its whitespace and outlines organize one task without a wizard or decorative metrics.

### Persistent operation footer

The notice line reports actual action results and errors. Start validates the selected binding and saves before launching; “复制 OneBot 地址” saves before copying. “加入白名单” saves after adding the chosen contact. Binding saves itself. The general save action remains available for manual edits; page navigation retains unsaved edits, while reload/close can prompt about discarding them. Keep these action-integrated saves when extending the flow.

Copy actions operate only on an explicit user request; the copied OneBot URL may include a secret. During this design/review pass the local listener and bound robot WeChat remain stopped. Documentation and preview controls do not authorize starting, binding or messaging.

### Advanced and monitoring surfaces

Advanced settings expose automatic self-chat identity as the default when the override is blank: resolve the exact bound account. Preserve offscreen operation, chat-only friend approval permissions and the maximum (120-second) explicit recall limit. These remain truthful operational settings rather than new visual motifs.

Logs use a native read-only multiline viewer and active filter buttons. Queue shares the viewer and displays actual durable queue states, including delivered, queued, failed and uncertain outcomes. It remains readable while stopped. An uncertain send must never acquire a successful label, color or statistic.

The sidecar contains self-contained HTML/CSS **preview translations** for inspecting these native primitives. They are not shipping controls, executable product handlers, a browser runtime or pixel-equivalent native captures. Preview outlines approximate native focus; application focus remains Windows-drawn.

## Do's and Don'ts

### Do:

- **Do** retain the pale-blue rail, white working pane and restrained indigo actions.
- **Do** use the existing Chinese typography roles and consistent DPI scaling.
- **Do** keep native selection, dropdowns, keyboard focus and meaningful control names.
- **Do** retain the persistent footer and honest binding, connection and listener states.
- **Do** preserve one-click saving in binding, whitelist addition, address copy and Start.
- **Do** distinguish queued, delivered, failed and uncertain outcomes.

### Don't:

- **Don't** introduce shipping UI rasters, glyph icons or rasterized screen text.
- **Don't** add unimplemented animations, decorative shadows or invented success metrics.
- **Don't** publish tokens, personal account details or inspection screenshots as design assets.
- **Don't** infer listener startup, account binding or messaging permission from a preview.
- **Don't** treat synthesized ramps or HTML/CSS preview geometry as native source tokens.
- **Don't** equate documented native tokens with accepted pixel equivalence to the comp.
