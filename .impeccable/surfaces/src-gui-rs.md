---
version: 1
slug: "src-gui-rs"
primary_target: "src/gui.rs"
related_targets: []
---

# Account and connection redesign

Mode: Operate. Windows native settings and monitoring client. Default page confirmed by the user: account and connection configuration. The user chose comp-first and now confirmed server image rollback is satisfactory; design may resume. Keep the listener and robot WeChat stopped.

## Direction contract

THESIS: Binding the intended account and connecting its RedReply should read as one calm setup task, with both identities and stopped state visible.

OWN-WORLD: The user's pinned Motrix reference governs: icy pale-blue sidebar, spacious white working pane, small consistent line icons, charcoal Chinese sans typography, restrained blue-violet action color. Windows window chrome and keyboard affordances remain native. The light surface is for a desktop operator reading settings and checking status during normal indoor use.

STORY: Select the intended logged-in WeChat and bind once; binding saves itself. Fill local or remote connection parameters and copy the address or start: either action saves first. Self-chat identity is detected automatically when no override is provided. Navigating away keeps unsaved edits visible. Queued, delivered and uncertain outcomes stay distinct on monitoring pages.

FIRST VIEWPORT: A complete desktop account/connection surface around1440x960, one left navigation region, a clear Chinese page heading, account selection/binding, editable connection fields, token masked, real disabled/stopped states and persistent save controls. Three comps vary side-by-side versus stacked versus master-detail navigation. No invented success statistics, accounts or installed-client promises.

FORM: Motrix-style account-first control desk, grounded candidate5, seed d52d2bc1. The explicitly user-pinned Motrix identity takes precedence over unrelated catalog styles. Candidate structures considered: classic preferences list, setup inspector, two-pane workspace, connection wizard, account-first control desk, stacked adaptable settings, category hub. Catalog challengers are declined because specimens, ticket ephemera and character catalogs weaken operational clarity, and a dark-console world contradicts the pinned light reference. Their useful disciplines remain readability, separated destructive actions, explicit state labels and clear section ownership, without importing motifs.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Current gate

The user approved option B: stacked account strip above connection settings, with fewer steps and one-click actions where possible. Approved comp: `.impeccable/mocks/account-b.png`. Native Win32 controls and Chinese Microsoft YaHei UI carry the user's pinned Motrix style; do not literalize image-generated font classifier suggestions or sample success claims. Source implementation is ready for final capture and independent finish review. No raster is shipped in the application UI. Local listener and robot WeChat stay stopped during GUI QA.
