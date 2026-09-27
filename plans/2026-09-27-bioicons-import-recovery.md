# Proposal record: bioicons import recovery

Self-contained record of an executed proposal. A fresh session can audit or re-run
it from this file alone.

## Goal

Recover the bioicons import so that assets the importer can safely transform are
indexed, while unsafe content is still conservatively rejected with a stable,
documented reason.

## Starting state

- Corpus: 2829 SVGs total.
- Old binary: 2542 indexed, 287 failed.
- Rebuilt source, before the last two fixes: 2739 indexed, 90 failed.
- Three gap classes still rejecting otherwise-clean assets:
  1. Inkscape `inkscape:path-effect` editor elements false-positiving the `on*`
     event-handler check.
  2. Referenced duplicate IDs rejected outright instead of renamed with references
     kept resolving.
  3. `@font-face` rules with `http(s)` `src` rejected, including inert external
     font references.

## Changes

Three fixes, all confined to `src/svg.rs`.

- **Inkscape path-effect elements as editor metadata.** Elements in the Inkscape
   namespace (e.g. `inkscape:path-effect`) are removed as editor metadata rather
   than treated as active content. This stops their editor-only attributes
   (`only_selected`, etc.) from tripping the `on*` handler check. The `on*` safety
  net for real event handlers is unchanged.

- **Rename-first for referenced duplicate IDs.** In `distinguish_unused_ids`, when
  an ID repeats, the first occurrence in document order keeps the id and later
  occurrences are renamed to `{id}__duplicate_N`, then remapped to the asset's
  opaque prefixed ids so references keep resolving (browser tree-order semantics for
`url(#id)`/`aria`). A conservative guard keeps the rejection when a `<style>`
   block has an id selector for that exact id: the selector would be rewritten
   through the same id map, but targeting a renamed duplicate by id is an author
   intent the importer does not try to guess (zero such cases in this corpus).

- **Strip only external-src `@font-face`.** In `stylesheet`, `@font-face` rules
  whose `src` is an `http(s)` URL are stripped (external dependency, fonts not
  bundled, inert in the `<img>` context). `@font-face` with a `local`/`data:` src
  and every other at-rule (`@media`, `@keyframes`, `@import`, ...) are still
  rejected.

## Boundaries

- Only `src/svg.rs`, its tests, and documentation changed.
- The `prepare()` pipeline order is unchanged.
- The importer, index, and server are untouched.
- No numbers in the acceptance criteria below were altered; they are measured.

## Acceptance criteria and measured outcomes

1. Inkscape path-effect assets are imported (editor elements removed, `on*` check
   intact). Met: covered by
   `passes_inkscape_path_effects_without_editor_attributes`; zero failures in the
   `only_selected`/event-handler class remain in the final run.
2. Referenced duplicate IDs are renamed and kept resolvable; rejection kept only
   for a `<style>` id selector. Met: covered by
   `renames_later_duplicate_ids_and_keeps_references_resolvable` and
   `rejects_duplicated_id_targeted_by_a_style_selector`; zero ambiguous-duplicate
   failures remain in the final run.
3. External-src `@font-face` stripped; local/data and other at-rules still
   rejected. Met: covered by `strips_external_font_face_rules` and
   `still_rejects_local_font_faces_and_other_at_rules`; the 30 excalidraw woff2
   rules are no longer failures, and no at-rule failures remain.
4. Final import is 2781 indexed / 48 failed, exit 0, all 48 in documented
   rejection classes, with all three gates green. Met:
   - `cargo build --locked` passes.
   - `cargo test --locked` passes (12 unit + 3 integration).
   - `cargo clippy --all-targets --all-features -- -D warnings` passes (zero
     warnings).
   - `cargo run -- import bioicons` exits 0 (partial import warns by default).
   - `cargo run -- status` reports `{"assets":2781}`.
   - The 48 failures are: 22 external or unresolved SVG reference; 7
     `foreignObject` with real content; 6 unresolved `href`; 4 CSS beyond simple
     class/id selectors; 4 invalid SVG XML; 3 external SVG `href`; 2 `script`.

## Re-run instructions

```sh
cargo build --locked
cargo test --locked
cargo clippy --all-targets --all-features -- -D warnings
```

Import offline against the existing cache (no network), then check status:

```sh
cargo run -- import bioicons --data-dir storage
cargo run -- status
```

Expected: import exits 0 with a warning listing the 48 skipped assets (recorded in
`storage/data/manifests/bioicons/last-import.json`), and status reports
`{"assets":2781}`.
