# bio-assets

A small Rust MCP service for reusable scientific SVG primitives. Find an asset,
retrieve its SVG and credits, and let the LLM compose the figure. Search and
retrieval use only local files and SQLite FTS5. There is no renderer, scene model,
layout engine, biological database, or embedding service.

## Container

```sh
docker compose up --build -d
```

Streamable HTTP MCP: **http://127.0.0.1:8092/mcp**

Health check: `http://127.0.0.1:8092/health`

The container runs as an unprivileged user. Port 8092 is published on host
loopback only. Data persists in the `bio-assets-data` named volume across rebuilds
and restarts. `docker compose down` keeps it; `down -v` deletes it.
The initial index is empty. Start imports explicitly:

```sh
docker compose exec bio-assets bio-assets import bioicons
docker compose exec bio-assets bio-assets import niaid
docker compose exec bio-assets bio-assets import scidraw

docker compose exec bio-assets bio-assets status
docker compose exec bio-assets bio-assets update bioicons
```

Run imports sequentially; a file lock prevents competing writers. The server can
keep serving previously imported assets during an import. These commands require
network access; ordinary MCP calls never create an HTTP client or fetch assets.
For a small import check, append `--limit 2`. Omit the limit for the full source.

Clients supporting remote MCP should add the URL above as a Streamable HTTP
server. The endpoint implements MCP initialization and discovery through the
official Rust SDK; it is not a custom REST approximation of MCP.

For a client such as OpenWebUI that reaches the service through the Docker host,
the container is configured to allow the `Host` names `host.docker.internal` and
`bio-assets` in addition to the local defaults.

## API

Four tools are exposed:

| Tool | Arguments | Result |
| --- | --- | --- |
| `search_assets` | `query`, optional `source`, `category`, `limit` (default 20, maximum 100) | Array of `{id,name,source,category,tags}` |
| `get_asset` | `id`, optional `include_svg` (default `false`) | `{id,name,source,view_box,width,height,license,license_url,author,attribution,source_url}`; `svg` is included only when `include_svg` is explicitly `true` (debugging) |
| `get_assets` | `ids` (1–100), optional `include_svg` (default `false`) | `{assets:[…],errors:[{id,code,message}]}`; `svg` is included only when `include_svg` is `true` |
| `compose_svg` | `width`, `height`, optional `background`, `elements` (1–50; each an asset `{asset_id,x,y,scale,rotation}` or a native `text`/`line`/`rect`/`circle`, an untagged union by required fields) | `{status,result_id,url,mime_type,width,height}` (no SVG markup); `status` is `"ok"` on success |

Composed SVG never travels through an MCP tool result. It is delivered through a
read-only HTTP endpoint: `GET /results/{result_id}.svg` → `200` with
`Content-Type: image/svg+xml` and the stored bytes; no MCP session is required.
`result_id` must be `ba_comp_` plus 16 lowercase hex chars; traversal attempts,
unknown IDs, extra path segments and malformed names all get `404`. There is no
directory listing and no HTTP write path.

Batch successes retain request order, including repeated IDs. Missing or unreadable
items produce explicit errors; they do not trigger remote lookups. When
`include_svg` is `true`, a batch is limited to 64 MiB of SVG markup and a single
SVG to 16 MiB; oversized batch items are reported for separate retrieval. Search
results and default asset retrieval contain no SVG markup.

Search requires all query words and ranks matches with FTS5 BM25, favoring names
and tags. Source and category are exact filters. Queries are tokenized as literal
words rather than executed as raw FTS syntax. A small `src/synonyms.json` adds
search aliases, including RTK/receptor tyrosine kinase/membrane receptor; this is
not an ontology. Broad aliases do not reclassify every membrane receptor as RTK.

Each tool supplies attribution metadata with the asset; an extra attribution
tool is unnecessary for the initial API.

## Composition

The flow: `search_assets` → `get_asset` (metadata by default) → `compose_svg`
→ present the artifact `url` to the user. After a successful composition the
caller presents the artifact URL; the generated SVG must not be fetched or
inspected by the model.
Each placed asset is anchored at its own `viewBox` origin; the transform is
`translate·scale·rotate` about that origin, with scaling applied through the
transform so `width` and `height` stay the raw (un-scaled) viewBox size.
Negative and non-zero origins are preserved. Elements may also be native
`text`, `line`, `rect`, or `circle`, which render as native SVG with sensible
defaults.

Identical compose arguments always produce the same `result_id` and the same
stored bytes. `result_id` is content-addressed: `ba_comp_` prefix + first 16
hex chars of the SHA-256 of the final composed SVG bytes. Re-running
`compose_svg` with the same arguments reuses the stored artifact (idempotent).

Artifacts live in `results/{result_id}.svg` under the data directory
(`/data/results/…` in the container). Limits: 1–50 elements per composition;
`scale > 0`; `|rotation| ≤ 360`; canvas `width`/`height` finite and greater
than zero; assembled SVG ≤ 16 MiB.

Present the artifact URL to the user; do not fetch or inspect the generated
SVG.

## Sources and licensing

### Bioicons

Imports the complete [upstream Git repository](https://github.com/duerrsimon/bioicons)
into `cache/bioicons`, then reads native SVGs under
`static/icons/<license>/<category>/<author>/` and `authors.json`.
Updates fetch the upstream main branch into this dedicated cache. The upstream
repository and author registry remain available locally, alongside per-file
metadata snapshots.

Asset licenses vary: CC0, CC BY, CC BY-SA, MIT and BSD are present. The repository's
software MIT license is **not** applied to every illustration. BSD labels without
a clause count are preserved as `bsd`, not guessed. Keep original notices and
check source-specific requirements before redistributing an asset. CC BY-SA assets
retain their share-alike license. See the upstream
[contribution format](https://github.com/duerrsimon/bioicons/blob/main/CONTRIBUTING.md).

IDs initially use the original path, including author and license to avoid
collisions. They remain unchanged on content edits. Unambiguous renames detected
by Git against the last completed import reuse the old ID. Bioicons has no
immutable upstream identifier: an ambiguous move plus substantial edit can appear
as a new asset. Old locally imported assets are not automatically deleted.

### NIAID / NIH BioArt Source

Reads the [public catalogue](https://bioart.niaid.nih.gov/discover) through the same
`discoverSearch` server action used by the site. The importer discovers the current
action ID from the site's JavaScript on each run. It paginates entries, preserves
their metadata, and downloads only SVG variants from
`/api/bioarts/<entry>/files/<file>`.

IDs are `niaid:<entry-id>/<file-id>` because entries may have several SVG variants.
Creators, collections, keywords, descriptions, timestamps, categories and license
labels are preserved. The catalogue's `CC-BY` label does not specify a version;
the importer retains that label and links the entry instead of inventing one.
Public Domain and CC-BY entries coexist. The website's
[terms](https://bioart.niaid.nih.gov/terms) and
[FAQ](https://bioart.niaid.nih.gov/faqs) require checking each entry's license.

**Maintenance limitation:** this is an undocumented website interface, not a
promised bulk-download API. A website deployment can require an importer update.
Failure leaves existing local assets usable. `--manifest` supports downloaded
exports if the remote interface becomes unavailable. No complete bulk archive was
identified during inspection.

### SciDraw

Uses the public `/api/v1/drawings/?image_type=svg` catalogue with cursor pagination,
then `/api/v1/drawings/<slug>/` for full author lists and DOI metadata. The list's
primary author is insufficient: all authors from the detail record are retained.
Downloads only native SVGs. Upstream HTTP links are upgraded to HTTPS on the same
host. IDs are `scidraw:<upstream-uuid>` and do not depend on titles or categories.

Per-record `cc-by` and `cc0` values map to CC BY 4.0 and CC0 1.0 as presented by
[SciDraw](https://scidraw.io/terms). Unknown future licenses produce an import error,
not a guessed license. DOI citations are retained when present. This public website
API is undocumented; no complete bulk archive was identified.

Importers use modest request pacing and retry transient failures/rate limiting.
An interrupted download run can be resumed with `import`; cached originals are
reused. `update` refreshes downloads. Successfully indexed records are committed
individually, while a failed asset retains its previous indexed version. Removed
upstream records are retained locally; there is no automatic pruning.

## Files and SVG preparation

Inside the data directory (`/data` in the container):

```text
assets/
  bioicons/<asset-key>/<content-hash>.svg
  niaid/<asset-key>/<content-hash>.svg
  scidraw/<asset-key>/<content-hash>.svg
data/
  assets.sqlite
  manifests/<source>/
  normalized/<asset-key>/<content-hash>.svg
results/
  {result_id}.svg
cache/
  bioicons/
  niaid/
  scidraw/
```

Original SVG bytes are retained unchanged. The indexed copy preserves paths,
groups, viewBox, visual attributes and embedded PNG/JPEG images. IDs and simple
class/ID CSS selectors are prefixed consistently, including href, gradient, mask,
clip and accessibility references. No rasterization or geometry optimization is
performed. When viewBox is absent, it can be inferred only from positive numeric
width and height (optionally px).

Normalization sanitizes content it can transform safely and rejects content it
cannot, instead of guessing at rendering behavior. Sanitized: editor metadata such
as Inkscape path-effect definitions and Adobe private branches; unused and referenced
duplicate IDs, where the first occurrence in document order keeps the id, later
occurrences are renamed, and references keep resolving; `@font-face` rules whose
`src` is an `http(s)` URL. Rejected conservatively: `<script>` elements,
`foreignObject` with real content, event-handler attributes, external `href`s,
unresolved `url()`/`href` fragment references, CSS beyond simple class/id selectors
(including other at-rules such as `@media`, `@keyframes` and `@import`), and empty
or invalid XML.

A partial import (some assets skipped) prints a warning and exits 0; `--strict`
exits nonzero on any skip; a run importing zero assets always exits nonzero. Skipped
assets are listed in `data/manifests/<source>/last-import.json`, and a failed asset
retains its previous indexed version. Some upstream assets will therefore be stored
but not searchable. No visual renderer comparison is performed; structural tests
cover reference rewriting.

Different assets receive different ID/class prefixes. When embedding the *same*
asset multiple times, the composing LLM should add an instance prefix and update
references. Fonts are not bundled or converted to paths. Preserve credits and
identify any later modifications in the final figure's attribution.

The SQLite schema is in `src/schema.sql`: one assets table, one FTS5 table and
transactional synchronization triggers. Import manifests preserve source-specific
metadata outside the MCP API. Files are written before publishing index changes.
Older file versions are retained; no implicit cleanup removes originals.

## Local exports and development

```sh
cargo build --locked
cargo test --locked
cargo clippy --all-targets --all-features -- -D warnings

cargo run -- import bioicons --from /path/to/bioicons-checkout
cargo run -- import niaid --manifest /path/to/export/manifest.json
cargo run -- serve --bind 127.0.0.1:8092
```

The default local data directory is `storage/`; override it with `--data-dir` or
`BIO_ASSETS_DATA`. `BIO_ASSETS_BIND` overrides the listening address.
`BIO_ASSETS_ALLOWED_HOSTS` is an optional comma-separated list of additional
allowed `Host` names for the HTTP endpoint; bare names match any port on that
host, and a name with a port matches that exact port. The local defaults
(`localhost`, `127.0.0.1`, `::1`) are always kept and Host validation is never
disabled, so unset or empty means local-only access.
`BIO_ASSETS_PUBLIC_URL` sets a public base for artifact URLs; when set to a
non-empty value, trailing slashes are stripped and URLs are absolute
(`{base}/results/{result_id}.svg`), and when unset or blank they are relative
(`/results/{result_id}.svg`). It is read on each `compose_svg` call.

A local manifest is a JSON array. SVG paths are relative to the manifest and may
not escape its directory (including through symlinks):

```json
[
  {
    "id": "niaid:103/628766",
    "source": "niaid",
    "upstream_key": "103/628766",
    "name": "Tick Nymph Feeding Day 2",
    "category": "arthropods",
    "tags": ["tick", "nymph"],
    "description": "",
    "local_path": "tick.svg",
    "license": "Public Domain",
    "author": "Ryan Kissinger",
    "attribution": "Ryan Kissinger. NIAID Visual & Medical Arts. Tick Nymph Feeding Day 2. NIH BioArt Source. Public Domain.",
    "source_url": "https://bioart.niaid.nih.gov/bioart/103"
  }
]
```

Mount an export read-only for a container import:

```sh
docker compose run --rm -v /absolute/export:/import:ro bio-assets \
  import niaid --manifest /import/manifest.json
```

The integration test starts the real HTTP MCP transport, initializes a client,
lists exactly four tools, searches, retrieves metadata (no SVG by default),
exercises partial batch failure, composes, fetches the artifact over the
`GET /results/{result_id}.svg` endpoint, rejects traversal and malformed result
IDs, and checks argument validation using locally generated fixtures. Other
tests cover FTS updates, aliases, source filters, SVG reference rewriting,
unsafe inputs, NIAID variants, and SciDraw coauthors. The `compose` test drives
a composition round-trip (compose then read the stored artifact) and extends
the HTTP tests with `include_svg` checks on `get_asset`/`get_assets`.
