# Proposal: deterministic SVG composition layer (`compose_svg`)

Add a deterministic composition tool so the LLM places assets by
`asset_id` + `x / y / scale / rotation` instead of transporting raw SVG path
data through its context. The server does the geometry; the LLM only supplies
placement parameters and the asset IDs.

This is a new capability layered on top of the existing
search → retrieve → "compose it yourself" model. It does not change import,
sanitization, or the existing three tools' behavior beyond `get_asset` gaining
an `include_svg` flag and a few always-present metadata fields.

## Goal

1. New MCP tool `compose_svg`: takes a canvas size + an ordered list of
   placed assets, returns a compact result (id + summary, **no SVG markup**).
2. New MCP tool `get_composed`: takes a composition id, returns the full SVG.
3. `get_asset` gains `include_svg` (default `true`) and always returns
   `source`, `view_box`, `width`, `height`.
4. Compositions are written to `data/composed/{result_id}.svg`, retrieved on
   demand, and are deterministic (same inputs → same output bytes).
5. Everything stays offline, no new dependencies, no weakening of the existing
   SVG sanitization, and `storage/` remains gitignored.

## Starting state (verified against the repository)

- `src/lib.rs` declares `importers`, `index`, `models`, `server`, `svg`.
- `src/models.rs`: `Asset` (import record), `AssetSummary` (search result),
  `AssetContent` (`{id,name,svg,license,license_url,author,attribution,
  source_url}`). `AssetContent` has **no** `source`/`view_box`/`width`/`height`.
- `src/server.rs`: exactly three tools — `search_assets`, `get_asset`,
  `get_assets`. `GetArgs` is `{ id }` only.
- `src/index.rs`: `Library::get(id)` reads the normalized SVG file and returns
  `AssetContent`. `initialize()` creates
  `assets`, `data/manifests`, `data/normalized`, `cache`.
  `atomic_write` and `contained_path` helpers exist.
- `src/svg.rs`: `prepare(bytes, id)` returns the normalized SVG **string** with
  a guaranteed `viewBox` (either present upstream or inferred from positive
  numeric width/height). It namespaces every `id` →
  `ba_{sha256(asset.id)[:16]}_i{N}` and every class →
  `ba_{sha256(asset.id)[:16]}_c{N}`, rewrites `href`/`xlink:href`,
  `url(#…)`, `aria-*`, and `<style>` selectors. It rejects scripts, active
  elements, event handlers, external refs, and CSS beyond simple class/id
  selectors. `digest(bytes)` is the sha256 hex helper. `MAX_SVG_BYTES` = 16 MiB.
- `src/schema.sql`: one `assets` table, one FTS5 table, triggers.
  No composition table exists.
- `Cargo.toml`: `rmcp 3.4.1`, `xmltree 0.11`, `rusqlite 0.38`, `sha2`, `regex`,
  `serde`/`serde_json`. No `symbol`/composition code exists yet.
- `tests/`: `local_import.rs`, `mcp_http.rs`. `mcp_http.rs` asserts exactly
  **three** tools and drives search/get/batch/validation over real HTTP.
- `README.md` documents "Exactly three tools" and the `get_asset` result shape.
- `.gitignore` ignores `/storage/` only; `tests/fixtures/` (new) is committed
  source and is **not** ignored.
- Corpus: 2781 bioicons assets locally (storage gitignored). NIAID/SciDraw are
  not imported locally (need network).

## Investigation verdict (why `<symbol>` + `<use>` is safe here)

Across the full local corpus (2781 assets):

- **0** external references, **0** `<script>`, **0** `foreignObject` content,
  **0** `xlink:href` (only plain `href`), all `<image>` are `data:`, all
  internal refs are `#`.
- 806 assets have a root `<svg id="…">`; **none** self-reference their own id,
  so no `<use>`/`<clipPath>`/`url()` inside the asset points back at the root
  element that we replace with a `<symbol>`.
- 47 assets have a negative or non-zero `viewBox` origin; the composer must
  therefore read the viewBox from each asset and wrap its content in a
  `<symbol viewBox="…">` (not assume `0 0 w h`).
- Construct counts that must survive the symbol wrap: 1366 `clipPath`,
  291 `linearGradient`, 704 `style`, 82 nested `<use>`, 42 `filter`, 22 `mask`,
  7 `pattern`, 119 embedded rasters.
- **Critical correctness rule:** every `<use>` MUST set `width` and `height`.
  A `<use>` referencing a `<symbol>` with no width/height stretches the symbol
  to 100% of the canvas viewport. Set them to the asset's own viewBox
  width/height and apply scaling through the transform (below).

Caveats for the implementer (validate, do not assume):

- xmltree re-emission of parsed asset content (now wrapped in a `<symbol>`)
  must be re-checked on the trickiest assets (drosophila with nested `<use>` +
  `userSpaceOnUse` gradients; SwissBioPics `rdf`/`cc` metadata). If xmltree
  mangles namespaces or `userSpaceOnUse` gradient coordinates on re-emission,
  fall back to **string-embedding** the already-normalized asset markup verbatim
  inside the `<symbol>…</symbol>` rather than re-parsing and re-emitting it.
  The normalized asset string is already fully namespaced and validated, so
  string embedding is the safe default; only re-serialize if a string path is
  infeasible.
- Phase 5 (NIAID/SciDraw end-to-end) needs network; it is a smoke check, not a
  gate. All acceptance criteria run offline against the local bioicons corpus.

## Design

### Geometry model (locked)

Each placed asset references its own `viewBox = [ox, oy, ow, oh]`. Placement is
anchored at the asset's viewBox origin (top-left), scaled by `s`, rotated by
`r` degrees about that top-left corner.

The composer emits, per placed asset:

```xml
<symbol id="{sym}" viewBox="{ox} {oy} {ow} {oh}"> …asset inner content… </symbol>
<use href="#{sym}" width="{ow}" height="{oh}"
     transform="translate({x} {y}) scale({s}) rotate({r})"/>
```

- `width`/`height` are the **raw** viewBox width/height `ow`/`oh` (never the
  scaled size) — this satisfies the "must set width/height" rule and maps the
  symbol's viewBox 1:1 into the use box.
- Scaling is applied by `scale({s})` in the transform, not by inflating
  width/height.
- Transform order is fixed: `translate({x} {y}) scale({s}) rotate({r})`, which
  yields rotation about the top-left (viewBox origin) corner anchored at
  `(x, y)`.
- `{sym}` is a fresh unique id per element (e.g. `ba_comp_{sha256(result_id)[:12]}_{i}`)
  so multiple placements of the same asset never collide.

Canvas: the root `<svg>` uses `viewBox="0 0 {W} {H}"` with the caller-supplied
canvas width/height `W`/`H`. Assets placed outside `0..W, 0..H` are allowed
(clipping is the browser's job; we do not clip).

### Result id and determinism

`result_id` is a stable content hash:
`ba_comp_{sha256(canonical_json(args))[:16]}` where `canonical_json` is a fixed
key order with no whitespace. Identical inputs therefore always produce the
same `result_id` and the same stored bytes, so `get_composed` is idempotent.
The composed file is content-addressed exactly like
`data/normalized/{key}/{hash}.svg`.

## Changes

All new code is additive. Existing tools keep their names and existing
argument/result fields (so existing clients keep working).

### 1. `src/lib.rs`
Add `pub mod compose;`.

### 2. `src/models.rs`
- Extend `AssetContent` with:
  - `source: String`
  - `view_box: [f64; 4]` (the asset's normalized viewBox)
  - `width: f64`, `height: f64` (derived from the viewBox: `ow`, `oh`)
- Add `ComposeElement`:
  `{ asset_id: String, x: f64, y: f64, scale: f64, rotation: f64 }`
- Add `ComposeArgs`:
  `{ width: f64, height: f64, background: Option<String>,
     elements: Vec<ComposeElement> }` (1..=50 elements, each `scale > 0`,
  `|rotation| <= 360`).
- Add `ComposeResult`:
  `{ result_id: String, width: f64, height: f64,
     element_count: usize, attribution: Vec<String> }`
  (no SVG markup; attribution is the union of the placed assets' attributions,
  deduped, in first-seen order — the composing agent must preserve credits).

### 3. `src/compose.rs` (new)
Pure function + small helpers. Public surface:

```rust
pub fn compose(args: &ComposeArgs,
               resolve: &dyn Fn(&str) -> Result<PlacedAsset>)
           -> Result<(String, /*svg bytes*/ Vec<u8>)>
```

where `PlacedAsset` carries `asset_id`, the normalized SVG **string**,
`view_box: [f64;4]`, and `attribution`. The resolver is injected so the module
stays free of `Library` and is unit-testable with in-memory fixtures.

Responsibilities:
- Validate `ComposeArgs` (element count 1..=50; `scale > 0`; finite numbers;
  canvas `width`/`height` finite and `> 0`).
- Resolve each `asset_id` via the injected resolver; on any missing/invalid
  asset, fail with a stable error naming the offending `asset_id` and do **not**
  write a file (all-or-nothing).
- Build the output deterministically:
  - root `<svg xmlns="…/2000/svg" viewBox="0 0 W H">`,
  - optional `<rect>` background if `background` is set,
  - for each element in request order: the `<symbol>`/`<use>` pair above,
  - a `<!-- attribution: … -->` comment carrying the union attribution (so the
    stored artifact is self-describing),
  - no DOCTYPE, no editor metadata, no scripts.
- Assemble `result_id` from the canonical hash; return `(result_id, bytes)`.
- Reuse `crate::svg::digest` for hashing. Keep `MAX_SVG_BYTES`-style bounds:
  reject if the assembled SVG exceeds 16 MiB.
- **Do not** re-run `svg::prepare` on already-normalized asset content (it is
  already namespaced/validated); string-embed it inside `<symbol>…</symbol>`.

### 4. `src/index.rs`
- `initialize()`: also create `data/composed`.
- Extend `get(id)` to populate the new `AssetContent` fields
  (`source` from the DB row; `view_box`/`width`/`height` parsed from the
  normalized asset's root `viewBox`). The DB already stores `reusable_path`;
  parse the viewBox from the loaded normalized SVG (or store it at import time
  if the implementer prefers a column — but adding a column is optional and
  must keep the existing schema/tests valid; reading it from the normalized SVG
  is the minimal change).
- Add:
  - `compose(args) -> Result<ComposeResult>`: resolves each asset via `get`,
    calls `compose::compose`, writes `data/composed/{result_id}.svg` with
    `atomic_write`, and returns the compact `ComposeResult` (no markup).
  - `get_composed(result_id) -> Result<String>`: validates `result_id` shape,
    reads `data/composed/{result_id}.svg` via `contained_path`, returns the SVG.

### 5. `src/server.rs`
- `GetArgs`: add `include_svg: bool` (`#[serde(default = "default_true")]`).
  - When `true` (default) `get_asset` returns the full `AssetContent` including
    `svg` (backward compatible). When `false`, omit the `svg` field but still
    return `id,name,source,view_box,width,height,license,…,attribution,
    source_url`.
- `get_asset` handler: pass `include_svg` through to a `get(id, include_svg)`
  variant (add the boolean to `Library::get`, defaulting to `true` for the
  existing callers).
- Add `compose_svg` tool (`ComposeArgs` → `ComposeResult`):
  - `annotations(read_only_hint=false, destructive_hint=false,
    idempotent_hint=true, open_world_hint=false)`.
  - description: deterministic placement of local assets by id + x/y/scale/
    rotation; returns a result id, not SVG; retrieve with `get_composed`;
    preserve all returned attributions.
  - runs the blocking `library.compose(args)` via `spawn_blocking`.
- Add `get_composed` tool (`{ result_id: String }` → `{ result_id, svg }`):
  - read-only annotations; retrieves the stored SVG by id.
- Update the `ServerConfig` `instructions` string to mention the new
  composition flow (search → `get_asset` for metadata → `compose_svg` →
  `get_composed`), and note that the composer anchors at each asset's viewBox
  origin.

### 6. `tests/fixtures/` (new, committed)
Seven real **CC0** bioicons assets (normalized form) chosen to cover the hard
cases, plus one synthetic mask. Each fixture is the *normalized* output of
`prepare()` for that asset (so it is exactly the shape `compose` will consume).
Fixture set:

| asset id (bioicons:…) | what it exercises |
| --- | --- |
| `…drosophila…` (Drosophila) | nested `<use>` + `userSpaceOnUse` gradients |
| `…celegans…` (C. elegans) | `clipPath` |
| `…erlenmeyer-flask…` (Erlenmeyer flask) | negative/non-zero viewBox origin |
| `…fruit-fly…` (Fruit fly) | `<style>` block |
| `…bottles…` (bottles) | `filter` |
| `…cirrhotic-liver…` (cirrhotic liver) | `pattern` |
| `…spatula-side…` (spatula, side) | plain paths |
| `mask-synthetic` | a `mask` element (synthetic; no CC0 mask asset exists) |

The exact bioicons upstream keys must be confirmed against the local corpus
(`/tmp/opencode/current_assets.tsv` has `id|reusable_path|name` for all 2781)
before finalizing filenames. Fixtures are plain text SVG under
`tests/fixtures/`, referenced by the tests. They are committed (not in
`storage/`).

### 7. `tests/compose.rs` (new)
Unit + integration tests that build a `Library` over a tempdir, store the
fixtures as assets, and drive `compose`/`get_composed`. Cover at minimum:

- Determinism: two `compose` calls with identical args return the same
  `result_id` and identical bytes.
- Symbol/use correctness: the output has one `<symbol>` + one `<use>` per
  element; every `<use>` has `width` and `height`; every `href="#…"` resolves
  to an emitted `<symbol id>` (no dangling refs).
- Geometry: with `scale=2, rotation=0`, the transform is exactly
  `translate(x y) scale(2) rotate(0)`; with `rotation=90`, the transform is
  `translate(x y) scale(s) rotate(90)` (rotation about top-left).
- Reuse of the same asset twice produces two distinct `<symbol>` ids and no
  id collision (assert all emitted ids are unique).
- Negative/non-zero viewBox asset: the `<symbol>` `viewBox` matches the asset's
  original origin/size (not forced to `0 0 w h`).
- Hard-case fixtures round-trip: each of the 7 real fixtures composes without
  error, the output parses as XML (`xmltree::Element::parse`), and the asset's
  inner markup (gradient/clip/style/filter/pattern/mask ids) is present and
  still namespaced/prefixed (no bare upstream ids leaked).
- All-or-nothing: a composition listing a missing `asset_id` returns an error
  and does **not** create a file in `data/composed`.
- Bounds: >50 elements is rejected; `scale<=0` is rejected.
- `get_composed` returns the exact stored bytes and 404-style error for an
  unknown `result_id`.
- Attribution: `ComposeResult.attribution` is the deduped union of the placed
  assets' attributions in first-seen order.

### 8. `tests/mcp_http.rs` (extend)
The existing test asserts exactly **three** tools; update it to **five** and
add over-the-wire checks:
- `get_asset` with `include_svg` omitted returns `svg`; with `include_svg:false`
  omits `svg` but still returns `view_box`, `width`, `height`, `source`.
- `compose_svg` returns a compact result (no `svg` field, has `result_id` and
  `attribution`) and `get_composed` then returns the full SVG referencing the
  placed assets.
- Argument validation: `compose_svg` with 51 elements, and with `scale:0`,
  both produce `isError`/error responses.

### 9. `README.md` (extend)
- Update the API table to five tools and the new `get_asset` result fields and
  `include_svg` flag.
- Add a "Composition" section describing `compose_svg`/`get_composed`, the
  geometry model (anchor at viewBox origin, `translate·scale·rotate`, scale via
  transform, width/height = asset viewBox size), determinism and result ids,
  the `data/composed/` path, the 1..=50 element and 16 MiB limits, and the
  credit-preservation note.
- Update the integration-test description (five tools, composition round-trip).

### 10. `docs/devlog.md` (append a new section)
Record the goal, decisions (symbol+use safety, width/height rule, transform
order, all-or-nothing, string-embedding default, determinism), verification
commands and measured outcomes, and any known residue.

## Boundaries

- Only `src/lib.rs`, `src/models.rs`, `src/compose.rs` (new), `src/index.rs`,
  `src/server.rs`, `tests/`, `README.md`, `docs/devlog.md`, and the new plan
  file change.
- No new dependencies in `Cargo.toml`.
- `src/svg.rs::prepare` and the importer are **unchanged**.
- Existing tools keep their names and existing fields; `get_asset` additions are
  additive (new optional flag, new always-present metadata fields).
- `storage/` stays gitignored; `tests/fixtures/` is committed.

## Acceptance criteria

1. `compose_svg` returns a compact result (no SVG markup) with a deterministic
   `result_id`; `get_composed` returns the stored SVG. Verified by
   `tests/compose.rs` determinism + reuse tests.
2. Every emitted `<use>` sets `width`/`height`; every `href` resolves to an
   emitted `<symbol>`; all emitted ids are unique. Verified by
   `tests/compose.rs` symbol/use + reuse tests.
3. Geometry is exactly `translate(x y) scale(s) rotate(r)` about the asset's
   viewBox origin; the `<symbol>` `viewBox` preserves a negative/non-zero
   origin. Verified by geometry + Erlenmeyer-flask tests.
4. All 7 real CC0 fixtures + 1 synthetic mask compose, parse as XML, and retain
   their namespaced ids (no bare upstream ids leaked). Verified by
   `tests/compose.rs` hard-case tests.
5. `get_asset` returns `source`, `view_box`, `width`, `height` always, and
   honors `include_svg` (default `true` keeps the old shape). Verified by
   `tests/mcp_http.rs`.
6. A composition referencing a missing asset fails without writing a file;
   >50 elements and `scale<=0` are rejected. Verified by `tests/compose.rs`.
7. Gates green, offline:
   - `cargo build --locked`
   - `cargo test --locked`
   - `cargo clippy --all-targets --all-features -- -D warnings` (zero warnings)
8. `storage/` is not modified or committed; `tests/fixtures/` is committed.

## Re-run / verification

```sh
cargo build --locked
cargo test --locked
cargo clippy --all-targets --all-features -- -D warnings
```

The composed-artifact smoke check is offline (local bioicons corpus). If
NIAID/SciDraw assets are later imported (network), run one `compose_svg`
covering one asset from each source to confirm the symbol wrap holds there too —
this is a smoke check, not a gate.

## Open questions for the implementer (decide, then record in the devlog)

- Parse `view_box`/`width`/`height` for `AssetContent` from the normalized SVG
  at `get` time (minimal) vs. adding a column to `src/schema.sql`. Prefer the
  minimal path unless a column is clearly cleaner; whatever is chosen must keep
  the existing schema/tests passing.
- String-embed the normalized asset markup inside `<symbol>` (default) vs.
  re-parsing/re-emitting via xmltree. Default to string embedding; only switch
  if it is infeasible, and re-verify the drosophila (nested `<use>` +
  `userSpaceOnUse`) and SwissBioPics metadata cases after any switch.
- Confirm the exact bioicons upstream keys for the 7 fixture assets against
  `/tmp/opencode/current_assets.tsv` before finalizing `tests/fixtures/`
  filenames.
