# Devlog: bioicons import recovery

## 1. Goal and final result

Recover the bioicons import after a regression. The old binary indexed 2542 of
2829 and failed on 287. A rebuild from source (before the last two fixes) indexed
2739 and failed on 90. After the three fixes, the final run indexes **2781** and
fails on **48**, with exit code 0 (partial import warns by default).
`cargo run -- status` reports `{"assets":2781}`.

All verification gates pass: `cargo build --locked`, `cargo test --locked`
(12 unit + 3 integration), and `cargo clippy --all-targets --all-features --
-D warnings` (zero warnings). The 48 failures fall entirely into the documented
conservative rejection classes; none is a defect.

## 2. Decisions

### (a) Rename-first for referenced duplicate IDs

When an ID appears more than once, the first occurrence in document order keeps
the id and later occurrences are renamed to `{id}__duplicate_N`, then remapped to
the asset's opaque prefixed ids. References keep resolving.

Why: browsers resolve `url(#id)` and `aria-*` references to the first matching
element in document order, so keeping the first occurrence matches rendering
semantics rather than guessing. A conservative guard is still kept that rejects
an asset when a `<style>` block contains an id selector for that exact id:
stylesheet id selectors are rewritten through the same id map, so the outcome
would be browser-consistent, but targeting a renamed duplicate by id is an
author intent the importer does not try to guess. The guard matches zero assets
in this corpus.

### (b) Strip only external-src `@font-face`

`@font-face` rules whose `src` is an `http(s)` URL are stripped. `@font-face`
with a `local` or `data:` src, and every other at-rule (`@media`, `@keyframes`,
`@import`, ...), are still rejected.

Why: an `http(s)` src is an external dependency. Fonts are not bundled or
converted to paths, and such a rule is inert in the `<img>` context used for
display, so removing it loses nothing renderable. A `local`/`data` src cannot be
resolved to a bundled font, so it is kept as a conservative rejection rather than
guessed at. All 30 stripped rules in this corpus were `excalidraw.com` woff2 URLs.

### (c) Inkscape path-effect elements are editor metadata

`inkscape:path-effect` editor elements are removed as namespace-scoped editor
metadata, not treated as active content.

Why: their `only_selected` and related editor-only attributes were false-positiving
the `on*` event-handler check and rejecting otherwise-clean assets. The `on*`
safety net for real event handlers is unchanged and still rejects genuine handler
attributes.

### (d) What remains rejected, and why

The final 48 failures are all in intended conservative rejection classes:

| Count | Rejection class |
| --- | --- |
| 22 | external or unresolved SVG reference |
| 7 | unsupported active SVG element: `foreignObject` (real content, e.g. drawio) |
| 6 | unresolved `href` |
| 4 | only simple class/id CSS selectors are supported |
| 4 | invalid SVG XML |
| 3 | external SVG `href` is unsupported |
| 2 | unsupported active SVG element: `script` |

These are rejected because the importer cannot verify their rendering or provenance
locally: active content (`<script>`, `foreignObject` with content), external or
unresolvable references, and stylesheets beyond simple class/id selectors would
each require guessing at behavior or fetching resources.

## 3. Verification

- Gates: `cargo build --locked`, `cargo test --locked` (12 unit + 3 integration),
  `cargo clippy --all-targets --all-features -- -D warnings` (zero warnings).
- Import JSON: `data/manifests/bioicons/last-import.json` lists the 48 skipped
  assets; `cargo run -- status` reports `{"assets":2781}`.
- Class distribution: the counts in section 2(d) sum to 48.
- Unit tests added or updated in `src/svg.rs`:
  - `passes_inkscape_path_effects_without_editor_attributes`
  - `renames_later_duplicate_ids_and_keeps_references_resolvable`
  - `rejects_duplicated_id_targeted_by_a_style_selector`
  - `strips_external_font_face_rules`
  - `still_rejects_local_font_faces_and_other_at_rules`
  - `rejects_active_and_external_content` (updated)

## 4. Known residue

Inert `path-effect="#..."`-style attributes may remain on elements. These are
unknown attributes with no render effect; only the `inkscape:path-effect` elements
themselves are removed.

# Deterministic SVG composition (`compose_svg` / `get_composed`)

## Goal

Deterministic `compose_svg` + `get_composed` so the LLM places assets by
`asset_id` + `x/y/scale/rotation` instead of transporting raw SVG.

## Decisions

- `<symbol>` + `<use>` wrap is safe on this corpus: 0 external refs, 0
  `<script>`, 0 `foreignObject`, all `<image>` are `data:`, no self-references
  in the 806 assets that have a root `<svg id>`.
- Every `<use>` must set `width` and `height` to the asset's raw viewBox size;
  scaling is applied through the transform, not by inflating `width`/`height`.
- Fixed transform order `translate·scale·rotate` about the viewBox origin.
- String-embedding of already-normalized markup inside `<symbol>…</symbol>`
  (not xmltree re-emit); the normalized asset string is already fully namespaced
  and validated.
- All-or-nothing: missing asset ⇒ error, no file written.
- Determinism via `serde_json::to_string` canonical hash → `result_id`
  (`ba_comp_` + first 16 hex chars).
- `view_box` / `width` / `height` parsed from the normalized SVG at `get` time
  (no DB column).
- `include_svg` is a serialization concern (default `true` keeps the old shape;
  `false` strips `svg` but retains `source`/`view_box`/`width`/`height`).

## Verification

- `cargo build --locked`
- `cargo test --locked` (now 5 tools incl. `tests/compose.rs` + extended
  `tests/mcp_http.rs`)
- `cargo clippy --all-targets --all-features -- -D warnings`

## Known residue

When the SAME asset is placed more than once, its internal (asset-owned) ids are
duplicated in the output because markup is string-embedded verbatim. Symbol ids
are always unique and browsers resolve duplicate internal ids to the first copy,
which is visually identical — accepted as a property of string-embedding, not a
defect.

NIAID/SciDraw end-to-end needs network (smoke check only, not a gate).
