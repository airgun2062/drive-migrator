# Architecture reference: what exists before the rewrite

A ground-truth snapshot of the backend engine, file-reading/extraction
logic, and GUI, written immediately before starting a from-scratch rewrite
on a new branch. This is not a design proposal - it's a record of what was
actually built, verified against the current source, so the rewrite can
deliberately keep what worked and deliberately change what didn't, instead
of forgetting either. Read alongside `LESSONS_LEARNED.md` (the *why* behind
several of the harder-won decisions here) and `SPEC.md` (the original design
intent, section-numbered, referenced throughout the code as `SPEC.md
section N`).

Workspace layout: `crates/engine` (library, all logic), `crates/cli` (thin
CLI binary `migrator`, subcommands `analyze` / `plan` / `run` /
`write-manifest` / `verify` / `similar`), `crates/gui` (Tauri 2 Rust
backend) + `crates/gui-frontend` (React 19 + TypeScript + Vite, a sibling
directory, not Tauri's default nested `src-tauri/`), `xtask` (synthetic
fixture generator - the only data the automated test suite ever touches).

---

## Part 1: the backend (`crates/engine`)

### 1.1 Scanning (`scan.rs`)

`scan_root(s)` / `scan_root(s)_with_progress` walk a directory tree and
return `Vec<ScanEntry>` (`path`, `size`, `modified`). A root that doesn't
exist yet scans as empty, not an error (the normal case for a destination
that hasn't been created).

Design decisions, in the order they mattered:

- **Junk filtering**: `.DS_Store`, `Thumbs.db`, `._*` (AppleDouble) are
  never treated as real files (SPEC.md section 7's ignore list).
- **Symlinks are never followed** (`follow_links(false)`), so a link cycle
  can't cause an infinite walk.
- **Top-level units, not one recursive walk.** `root`'s immediate children
  are processed as independent units - loose files directly under `root`
  first, then each immediate subdirectory, **sorted by name** - rather than
  one single `WalkDir::new(root)` call. Two reasons, discovered in that
  order: (1) `walkdir` doesn't guarantee traversal order, and the GUI's
  folder picker always displays folders name-sorted, so an unsorted walk
  made progress checkmarks appear scattered/skipped even though nothing
  was actually skipped; (2) processing folders as independent units means a
  stall deep in *one* folder only gives up on that folder (see below), not
  the entire rest of the tree.
- **Every step is stall-timed** (`WatchedIter`, `crates/engine/src/
  timeout.rs`; see Part 1.6). A directory open, or a file's metadata
  resolution, that goes 20s with no response is logged and abandoned - the
  walk moves to the next folder rather than hanging forever. This exists
  because real cloud-synced folders (OneDrive Files On-Demand and similar)
  can block a directory open indefinitely while trying and failing to
  materialize content. Important, non-obvious fact backing this: with
  `follow_links(false)`, `walkdir::DirEntry::metadata()` on Windows returns
  an **already-cached** value from the directory listing with zero
  syscalls - it is *not* the blocking call it looks like. The actual risk
  is opening a *new* directory to descend into, or (on non-Windows) a fresh
  `stat()`.
- **`ScanProgress::FolderWalked { path, files_found }`** is emitted once per
  top-level unit (including the loose-files bucket, keyed by an empty path)
  with its *exact* count, and is never subject to the GUI's IPC throttling
  (below). This exists because reconstructing per-folder counts from the
  throttled `Walking` event's `current`-file stream is fundamentally lossy:
  a folder small/fast enough to be walked entirely within one throttle
  window has its whole count silently attributed to whichever folder is
  `current` when the next throttled event fires. The fix wasn't a smarter
  reconstruction - it was a separate, authoritative, un-throttled signal.

### 1.2 Exact-duplicate detection - T1 (`analyze.rs`, `hash.rs`, `cache.rs`)

`analyze(_with_progress)` groups scanned entries by size (files with a
unique size can't be duplicates of anything), then for each same-size group:

1. **Sample-hash pre-filter** (`hash::sample_hash`): hashes the first and
   last 64 KiB (or the whole file if smaller). A cheap filter, not proof -
   two files sharing a sample hash may still differ elsewhere, but
   different sample hashes are always different files. Only survivors of
   this filter (≥2 files sharing a sample hash) get a full hash.
2. **Full BLAKE3 hash** (`hash::full_hash_with_progress`) for survivors,
   streamed through a fixed 256 KiB buffer (files are never loaded whole
   into memory).
3. Groups sharing a full hash are exact-duplicate groups.

**`FingerprintCache`** (`cache.rs`) persists `(path → size, mtime, hash)` so
an unchanged file is never re-hashed on a rescan. Never trusted blindly:
every lookup re-checks the file's *current* size and mtime (with a 2-second
tolerance for FAT/exFAT's coarse local-time storage) against what's
recorded before using the cached hash. Filesystem timestamps never
*define* a duplicate (a hard rule) - they only gate whether the cache entry
is still trustworthy.

Hashing runs in parallel across same-size groups via `rayon`. Progress is
reported **per file, not per group** - an early bug (a same-size group
containing many small files could hash for minutes with the UI apparently
frozen, since progress only fired once per whole group) fixed by reporting
after every file resolves, plus incremental byte reports from inside a
single large file's own hash (so one big outlier file doesn't look stuck
either).

Stall protection: both `sample_hash` (one-shot, wrapped in
`run_with_timeout`) and the full-hash streaming loop (`WatchedIter`, same
mechanism as the directory walk) are time-boxed - see Part 1.6.

### 1.3 Near-duplicate / version detection - T3 (`extract.rs`, `similarity.rs`)

See Part 2 for `extract.rs` in full detail (it's substantial enough to
deserve its own section). In brief: every supported file is reduced to a
`HashSet<u64>` of "chunks" (word shingles for text, one hash per row for
delimited/tabular data).

`similarity.rs` then:

1. Builds a **MinHash signature** per document (128 hash functions by
   default) - documents with similar token sets get similar signatures.
2. Buckets documents via **LSH banding** (32 bands) into candidate groups -
   this is what avoids comparing every pair (`O(n²)`) for a large corpus.
3. Within each candidate group, verifies every pair **pairwise** (never
   assumes a whole LSH-connected group is mutually similar - that could
   transitively merge unrelated files through a chain of coincidental
   overlaps).
4. For each pair above `duplicate_cutoff` (default 0.5 estimated Jaccard),
   computes **coverage in both directions** (`coverage(A→B)`: share of A's
   tokens also in B) and classifies the relationship:
   - Both directions ≥ `high_coverage` (default 0.8) → `NearDuplicate`.
   - One direction high, the other not → `BIsMoreComplete` /
     `AIsMoreComplete`.
   - Neither high → `Diverged` - flagged for manual merge, **never**
     auto-picked (SPEC.md section 5 is explicit about this).

These thresholds are **not calibrated against labeled data** - SPEC.md
section 4 calls for ~200 hand-labeled real pairs to choose real numbers;
that calibration tool was never built (blocked on human labeling effort,
not engineering effort). Treat the defaults as reasonable starting points,
not validated constants.

Extraction (the actual file-content read) is time-boxed the same way as
everything else - see Part 1.6.

### 1.4 Version ranking (`versions.rs`, `union_find.rs`)

Turns `similarity.rs`'s pairwise output into **families** - a graph, not a
chain (`union_find.rs`'s disjoint-set structure), since two versions might
both derive independently from a common ancestor rather than forming a
strict line. Within each family, members are ranked by:

1. **Completeness** - how many other family members this one transitively
   covers (Warshall's algorithm computes the transitive closure over the
   direct `BIsMoreComplete`/`AIsMoreComplete`/`NearDuplicate` edges, so a
   chain A-covers-B-covers-C still ranks A above C even if only adjacent
   pairs were directly compared).
2. **Date, as a tiebreak only** when completeness ties (`metadata.rs`'s
   `best_available_date` - see 1.5). SPEC.md section 5's rule: internal
   metadata first, then coverage, then filesystem time as a last resort -
   note this is a *different* priority order than described in ranking
   itself, where coverage is primary and date only tiebreaks; the date
   *source* priority (internal metadata over filesystem mtime) is a
   separate axis from *when in the ranking* date gets consulted.
3. Path, as a final deterministic tiebreak.

A member nothing else in its family covers is a **tip** - always kept
regardless of keep-newest-N (generalizes SPEC.md section 2's "diverged tips
are always kept" to any structurally-uncovered member, not only ones
reached via a `Diverged` edge, since those never join a family at all).

### 1.5 Best-available-date (`metadata.rs`)

Priority cascade, first match wins, falls through to the next on anything
missing or malformed (never fails the caller):

1. **Office core properties** (`docx`/`pptx`/`xlsx`): `docProps/core.xml`'s
   `dcterms:modified`, falling back to `dcterms:created` - deliberately not
   `lastModifiedBy`/`revision`, weaker indirect signals.
2. **EXIF** (`jpg`/`jpeg`/`tiff`) via `kamadak-exif`.
3. **Email header** (`eml`) `Date:` field (hand-rolled RFC 5322 parser).
4. **Filesystem modified time** - the universal fallback.

Hand-rolled civil-calendar date math (`days_from_civil`, Howard Hinnant's
public-domain algorithm) rather than pulling in a date/time crate for what
amounts to parsing a handful of date fields - `applog.rs`'s logging
timestamps later reused the *inverse* of this same algorithm
(`civil_from_days`) for the same reason.

### 1.6 Resilience: the stall-timeout mechanism (`timeout.rs`)

The single most important architectural lesson from this build (fully
covered in `LESSONS_LEARNED.md`). Two shapes, both in
`crates/engine/src/timeout.rs`:

- **`run_with_timeout(timeout, f)`** - one-shot: spawn `f` on a background
  thread, wait up to `timeout`, give up if it hasn't returned. For bounded,
  single-value operations: one file's extraction, one sample-hash.
- **`WatchedIter`** - streams many items from *one persistent* background
  worker, per-step timeout that resets on every item produced. For anything
  scaling with file/byte count: the directory walk, a large file's chunked
  hash, a file copy's read+write+fsync loop. Reusing one worker avoids
  paying thread-spawn cost per file/chunk (which would be real, measurable
  overhead at the file counts this app deals with); the cost is only paid
  again on the rare occasion a step actually stalls.

There is no portable way in Rust to force-cancel an arbitrary blocking
syscall from another thread. Both mechanisms only bound how long the
*caller* waits - an abandoned thread may keep running, and stay blocked,
forever. Accepted as a bounded cost (one idle thread per rare stall
occurrence), not a bug to chase further.

Applied to: directory walking, one file's content extraction, one
sample-hash, the full-file streaming hash (BLAKE3 and SHA-256), and a file
copy's read+write+fsync (all three folded into the *same* worker, so a
stalled fsync gets the same coverage as a stalled read). **Not yet
applied**: the GUI's file-preview-on-click path (`read_text_preview`) -
known, documented gap, not yet hit in practice.

Timeout value: a flat 20-second constant per module right now, not
user-configurable. Chosen as generous enough for a slow-but-healthy network
read, short enough that a dead operation doesn't block for minutes.
Revisit if real usage shows either direction is wrong.

### 1.7 "One bad file must never abort a scan" - the resilience principle, applied to errors

Distinct from (but complementary to) the stall-timeout principle above:
a file that's readable-but-invalid (corrupted zip, non-UTF-8 CSV, malformed
JSON/YAML, a file deleted mid-scan) must never abort the whole operation
either. Applied at every phase - walking, hashing, extraction - via
`let Ok(x) = y else { continue }`/explicit-match-and-skip patterns rather
than `?`-propagation. Every skip is logged (`applog.rs`) with the specific
path and reason. Real research data made this necessary, not theoretical:
corrupted `.docx`/`.pptx`/`.xlsx` files ("Could not find EOCD", missing
internal zip parts) and non-UTF-8 CSV exports from lab instruments were
common, not exotic.

### 1.8 Reconciliation (`reconcile.rs`)

Compares a source and destination tree and classifies every file
(`ReconcileState`): `Verified` (same path, matching content),
`Partial{reason}` (same path, size mismatch), `Missing` (no destination
counterpart anywhere), `Moved{destination_relative}` (content exists at a
*different* destination path - detected via a content-hash index built
over otherwise-unmatched entries), `Conflict` (same path, same size,
different content), `Blocked{issues}` (fails a preflight rule - see 1.10),
`DestinationOnly` (destination file with no source match). Never copies or
deletes anything - analysis only.

Path comparison uses a **normalized key** (forward-slashed, Unicode NFC,
lowercased) so two paths a user would call "the same name" (NFC vs NFD
composition, or differing only in case) collide correctly - a destination
that can't tell them apart is caught as `PathCollision`, not silently
overwritten. The *raw* path is always kept separately for actual writing.

Guards against a destination nested inside the source (or the reverse) via
`std::path::absolute` (not `canonicalize` - the destination usually doesn't
exist yet, and Windows's `\\?\` canonicalization prefix would break
equality comparison against a non-canonicalized path for the same
location).

### 1.9 Collapse mode (`collapse.rs`)

Three independent signals, each with its own precedence:

- **Exact duplicates** (T1): not versions - always collapse to exactly one
  kept copy (first by path, for determinism), unconditionally, regardless
  of family status or protected-extension status.
- **Version families** (T3 + ranking): keep-newest-N, tips always kept.
- **Protected extensions** (SPEC.md section 6, e.g. raw research data):
  never collapsed *as a version* - only ever as an exact duplicate.

`archive: bool` redirects what would be dropped to `_collapsed/<relative
path>` instead of discarding it outright. Analysis only
(`plan_collapse`) - `apply_collapse_to_plan` separately narrows an existing
`ReconcilePlan` to reflect the decisions, touching only `Missing`/`Partial`
entries (an already-`Verified` destination file is left alone even if a
fresh collapse decision would drop it - deleting previously-copied files is
a distinct, more destructive operation this doesn't perform).

### 1.10 Preflight rules (`policy.rs`)

Destination-compatibility checks run before anything is copied: path length
limit (default 255), illegal characters (`: * ? " < > |`), Windows reserved
names (`CON`, `PRN`, `COM1`-`9`, `LPT1`-`9`, ...), trailing dot/space on a
path component, and (opt-in) a FAT32 4 GiB file-size ceiling. Defaults are
the most restrictive common denominator (Windows-safe) since the engine
doesn't detect the actual destination filesystem.

### 1.11 Transfer (`transfer.rs`)

Copy protocol (SPEC.md section 7, a hard rule): write to `<name>.part`,
hash while writing (BLAKE3), fsync, re-open and re-read what actually
landed on disk to verify against the source hash (never trusts the
in-memory write-loop hash alone), then atomic rename to the final name. A
verification mismatch deletes the `.part` file and reports
`VerifyMismatch` rather than leaving a corrupt partial result under the
real name. Stray `.part` files from a previous interrupted run are
discarded at the start of every run. Best-effort mtime preservation
(filesystem timestamps are metadata, never correctness). The full
read+write+fsync+mtime sequence is stall-timed as one unit (1.6). A single
file's failure never stops the rest of the run - failures are collected
into `TransferSummary.results`, not propagated.

`MovedPolicy` (`Leave` default, `Copy`) decides whether a `Moved` reconcile
entry gets copied to its new relative path or left alone.

### 1.12 Manifest & verification (`manifest.rs`)

Writes a **visible, human-readable JSON manifest**
(`MIGRATION_MANIFEST.json`, warned as generated/checksummed) plus its
SHA-256 checksum file, a **hidden backup copy** (`.migrator/manifest.backup
.json` + checksum), a **SQLite database** (`.migrator/manifest.db`, exact
per-file records for fast re-verification), and an **app-local hash record
stored outside the destination entirely** (the OS app-data directory).

`check_integrity` implements SPEC.md section 8's **two-of-three vote**:
compares the visible manifest's hash, the backup's hash, and the app-local
hash: `trusted` is true only when at least two agree. A single copy being
tampered with (edited to hide a change) doesn't fool the check, since the
other two still agree with each other. A copy that's missing or
unparsable simply drops out of the vote rather than failing the whole
check.

`verify` re-scans the destination and reports what changed since the
manifest was written: unchanged, changed (path exists, hash differs),
moved (hash exists at a different path than recorded), deleted (recorded,
now absent), unrecorded (present now, never recorded).

Also computes duplicate groups and version families *as they currently
exist at the destination* (not carried over from whatever `CollapseReport`
produced that destination, since `write_manifest` runs as its own separate
command/step, deliberately not sharing state with `run`).

### 1.13 Journal (`journal.rs`)

SQLite (WAL mode) audit trail: `plan_runs`, `plan_entries`, `transfer_
events` tables recording intent and outcome. **Never trusted on its own**
(SPEC.md section 3, principle 2) - every run re-scans both roots and
reconciles fresh regardless of what the journal says happened before. Pure
history/audit, not a source of truth for current state.

### 1.14 Progress & cancellation (`progress.rs`)

`CancelToken` (`Arc<AtomicBool>`, cheap to clone) - cooperative
cancellation, checked between files (or small batches, where parallelized)
and inside long single-file operations (a large hash's per-chunk check).
Cancelling never produces an error - it returns whatever was gathered so
far, since a user clicking cancel is not a failure.

`ScanProgress` is one shared vocabulary across all phases: `Walking`,
`Hashing` (with nested `current_file_bytes_done`/`current_file_size` for
one outlier-large file's own sub-progress), `ComparingContent`,
`FolderWalked` (1.1's authoritative per-folder signal).

### 1.15 Logging (`applog.rs`)

`Log::info(tag, message)` / `Log::error(tag, message)`, writing to
`<repo root>/logs/<YYYY-MM-DD-HH>.log` (fresh file every hour). Path
resolved via `CARGO_MANIFEST_DIR` (stable regardless of the actual process
working directory - `cargo tauri dev` in particular runs from `crates/gui`,
not the repo root). Best-effort: a logging failure (disk full,
permissions) is silently swallowed rather than breaking whatever it was
logging. Built specifically because diagnosing a frozen app from
screenshots alone doesn't scale - see `LESSONS_LEARNED.md`.

The frontend has a matching `Log` (`gui-frontend/src/log.ts`) that forwards
into the *same* log files via a `log_message` Tauri command, so frontend
and backend activity for one scan/transfer interleave in one place.

### 1.16 Errors (`error.rs`)

`EngineError` (`thiserror`), one variant per failure category
(`Read`/`Write`/`Open`/`Seek`/`Fsync`/`Rename`/`Remove`/`CreateDir`/
`CsvParse`/`JsonParse`/`JsonSerialize`/`YamlParse`/`XlsxRead`/`ZipOpen`/
`VerifyMismatch`/`NestedRoots`/`PathNotUnderRoot`/`CacheRead`/`CacheWrite`/
`CacheParse`/`CacheSerialize`/`JournalDirCreate`/... - each carrying the
relevant path and underlying source error). No `unwrap()`/`expect()` in
library code touching the filesystem (a hard rule) - every fallible
filesystem operation returns a typed error, which callers then decide
whether to propagate, log-and-skip, or (per 1.6/1.7) treat as recoverable.

---

## Part 2: reading files (`extract.rs`)

The T3 pipeline's content-extraction layer: reduces any supported file to
an `ExtractedDocument { kind, tokens: HashSet<u64> }`. Two entry points
sharing the same per-format logic:

- **`extract(path, kind)`** - produces the token set MinHash/LSH operate on.
- **`preview_text(path, kind, max_chars)`** - produces human-readable text
  for the GUI's file preview, truncated to `max_chars`. Deliberately
  simpler than `extract()` for several formats (e.g. csv/tsv/json/yaml
  preview as raw text, not the canonicalized/row-parsed form extraction
  uses) - a preview should show what's actually in the file, not a
  normalized version of it.

### 2.1 `DocumentKind` classification (extension-based)

| Extension(s) | Kind | Extraction approach |
|---|---|---|
| `txt`, `md` | `PlainText` | raw text, shingled |
| `html`, `htm` | `Html` | `scraper`-stripped tag text, shingled |
| `json` | `Json` | parsed + canonically re-serialized (sorted keys, no whitespace), shingled |
| `csv`, `tsv` | `DelimitedRows` | `csv` crate, one hash per row |
| `docx` | `Docx` | zip + `word/document.xml`'s `w:t` runs, shingled |
| `pptx` | `Pptx` | zip + each `ppt/slides/slideN.xml`'s `a:t` runs, in slide order, shingled |
| `xlsx` | `Xlsx` | `calamine`, one hash per row across every sheet |
| `xml` | `Xml` | all text nodes via `quick-xml`, shingled |
| `yaml`, `yml` | `Yaml` | parsed (`yaml-rust2`) + canonical JSON re-serialization, shingled |
| `rtf` | `Rtf` | hand-rolled control-word stripper, shingled |
| `rs py js ts tsx jsx go java kt c h cpp hpp cc cs rb php sh bash swift scala m mm` | `SourceCode` | raw text, shingled (same as PlainText) |

**Shingling** (`shingle_tokens`): lowercase, split on non-alphanumeric,
5-word sliding window, each window BLAKE3-hashed down to a `u64`. A
document shorter than 5 words hashes as one token rather than producing
nothing. Row-based formats (csv/tsv/xlsx) instead hash one `u64` per row
(cells joined with a unit-separator character) - "chunk" means something
different per format family, but everything downstream (MinHash, Jaccard,
coverage) treats it uniformly as "a set of `u64`s."

### 2.2 The text-fallback rule

**A file whose format is text-readable but structurally corrupted falls
back to plain-text treatment rather than being excluded.** Applies to
`Json` (malformed syntax), `DelimitedRows` (bad delimiters, non-UTF-8
bytes), and `Yaml` (unparseable content) - each tries its structured parse
first, and on failure, shingles the raw lossily-decoded text instead of
erroring. Does **not** apply to `Docx`/`Pptx`/`Xlsx` - these are zip-based
binary containers, and a corrupted zip's raw bytes were never text to
begin with; there's nothing meaningful to fall back to, so those stay
excluded on corruption. `Html`/`Xml`/`Rtf` needed no such fallback because
their extractors already degrade gracefully *internally* on malformed
input (stop and return whatever text was recovered, never propagate an
error) - the fallback rule only applies where the underlying parser is
strict enough to hard-fail otherwise.

### 2.3 Docx/Pptx/Xlsx internals worth knowing

- Docx/Pptx read specific zip entries as raw XML and extract only text
  inside tags whose *local name* (ignoring namespace prefix) matches `t`
  (`w:t` for docx word runs, `a:t` for pptx text runs) - not a full
  OOXML parse, a good-enough recovery of visible text.
- Pptx slides are explicitly ordered by their numeric suffix
  (`ppt/slides/slideN.xml`) before concatenation, not zip entry order
  (which isn't guaranteed to be slide order).
- Xlsx reuses the same zip/XML machinery indirectly through `calamine`
  rather than hand-rolling spreadsheet parsing.
- `metadata.rs`'s Office-date reading (1.5) shares the same "read one zip
  entry, extract one tag's text" pattern independently, for
  `docProps/core.xml` specifically.

---

## Part 3: the GUI (`crates/gui` + `crates/gui-frontend`)

### 3.1 Stack and layout

Tauri 2 (Rust backend, native OS webview - WebView2/WKWebView/WebKitGTK,
not a bundled Chromium) + React 19 + TypeScript + Vite. `crates/gui` and
`crates/gui-frontend` are **siblings** under `crates/`, not Tauri's default
nested `src-tauri/` layout. Non-obvious consequence: `tauri.conf.json`'s
`beforeDevCommand`/`beforeBuildCommand` resolve relative to `tauri.conf.
json`'s *parent's parent* (i.e. `crates/`), not relative to `crates/gui/`
itself - hence `"npm run dev --prefix gui-frontend"` rather than `"..
/gui-frontend"`. Cost real debugging time to pin down; a from-scratch
rewrite keeping this directory shape should carry this fact forward
explicitly rather than rediscovering it.

Window: 1280×800 default, 900×600 minimum, starts maximized. Asset
protocol enabled with an empty static scope (`assetProtocol.scope: []`) -
widened at runtime per scan (`app.asset_protocol_scope().allow_directory
(&root, true)`) so the frontend can request in-scan-scope files via
`convertFileSrc()` for image/audio/video/pdf preview without exposing the
whole filesystem.

### 3.2 Tauri commands - the entire IPC surface (`commands.rs`)

No logic lives in `commands.rs` beyond turning frontend requests into
`engine` calls and engine reports into JSON - mirrors what `crates/cli`
does for the same underlying operations, deliberately not duplicated.

| Command | Purpose |
|---|---|
| `pick_folder` | native folder-picker dialog |
| `scan` | walk + analyze (T1) + find_similar (T3) in one call, sharing a single walk; widens asset-protocol scope; manages a `CancelToken` in `ScanState` |
| `cancel_scan` | signals the `CancelToken` held by an in-flight `scan` |
| `list_top_level_folders` | non-recursive immediate-subfolder listing, upfront, so the folder picker can show "N folders" before the walk reaches any of them |
| `transfer` | builds a `ReconcilePlan`, optionally applies Collapse-mode decisions, applies manual per-file exclusions, then runs the copy (or dry-runs) |
| `write_manifest_cmd` | wraps `manifest::write_manifest` |
| `verify_cmd` | wraps `manifest::verify` |
| `hash_file` | wraps `hash::full_hash`, hex-encoded |
| `read_text_preview` | wraps `extract::preview_text`, capped at 20,000 chars |
| `reveal_in_folder` | opens the OS file manager at a path (`explorer /select,` / `open -R` / `xdg-open` the parent) |
| `log_message` | frontend → backend log forwarding into the shared log files |

`scan` and `transfer` both stream progress via Tauri events
(`scan-progress`, `transfer-progress`) rather than a single return value -
necessary for anything taking more than an instant on real data volumes.

**IPC throttling**: `scan`'s progress callback caps actual `window.emit()`
calls to ~1 per 80ms (hashing especially can call back hundreds of times a
second across parallel rayon threads - forwarding every one would flood
the webview). Engine-side counting is *never* throttled, only what crosses
the IPC boundary. Critically, `FolderWalked` events (1.1) are explicitly
exempted from this throttle - a low-frequency (a few dozen per scan),
high-importance event must bypass whatever rate-limits the high-frequency
ones, or it silently inherits their lossiness.

**Cache path**: a per-source-folder `FingerprintCache` is loaded/saved
under `app_data_dir()/scan-cache/<sha256(canonical source path)>.json` -
outside the source, never inside it (a hard rule) - so rescanning the same
folder skips rehashing unchanged files.

### 3.3 Frontend structure

- **`App.tsx`** - all top-level state and orchestration: scan lifecycle
  (source path, scanning/cancelling flags, live `ScanProgressState`, error
  banner), the review workbench's selection state (`selectedPath` for the
  relations panel, `previewPath` for the preview panel - deliberately
  separate, since browsing relations and previewing don't have to be the
  same file), the `included: Set<string>` of paths the user has chosen to
  transfer (defaults to excluding every exact-duplicate but the first-by-
  path copy; near-duplicates default to *included*, since picking which
  version to keep needs calibrated confidence that doesn't exist yet), and
  the transfer-modal lifecycle.
- **`api.ts`** - thin `invoke()`/`listen()` wrappers, one function per
  backend command/event, camelCase-normalized.
- **`types.ts`** - TypeScript mirrors of every serde-serialized Rust type,
  verified against real command output rather than guessed from source.
  Manifest/verify types exist here but are **unwired** - the commands work,
  but no screen calls them yet.
- **`log.ts`** - frontend `Log.info`/`Log.error`, forwarding to
  `log_message` (1.15/3.2).
- **`grouping.ts`** (`buildIndex`) - turns a raw `ScanBundle` into display-
  ready rows: classifies every file `unique`/`near`/`exact`, sorts exact-
  first/near-second/unique-last, and provides `exactNeighbors(path)` /
  `nearNeighbors(path)` lookups for the relations panel. Near-neighbor
  relations are normalized to be relative to whichever file is the current
  *anchor* (SPEC.md's `BIsMoreComplete`/`AIsMoreComplete` are relative to
  the pair's raw a/b, not to whichever side the user clicked).
- **`scanProgress.ts`** (`applyScanProgress`) - a reducer turning the raw
  `ScanProgressEvent` stream into `ScanProgressState` (per-stage status,
  per-folder status/count map). Folder "active"/"done" status still comes
  from the (approximate, throttled) `Walking` event's `current`-file
  bucketing; the *count* shown, once a folder is done, is always overwritten
  by the authoritative `FolderWalked` event (3.2/1.1) - approximate live
  updates while active, exact number once finished.
- **`preview.ts`** - maps a file extension to a preview widget kind
  (`text`/`image`/`audio`/`video`/`pdf`/`unsupported`); its text-extension
  list is a hand-maintained mirror of `extract::classify` (2.1), not a
  shared source of truth - a rewrite should consider generating or sharing
  this list instead of hand-syncing two copies.
- **`format.ts`** - `formatBytes`, `fileName`, `dirName` display helpers.
- **`components/VirtualList.tsx`** - hand-rolled fixed-row-height
  windowed list (no library), per SPEC.md section 9's "tens of thousands of
  rows" requirement - renders only what's on screen plus a small overscan
  buffer.
- **`components/FileList.tsx`** - the workbench's main file list, built on
  `VirtualList`; each row shows a status dot, name/path, related-count chip,
  size.
- **`components/RelationsPanel.tsx`** - the selected file's exact/near
  neighbors, each with a similarity bar, a relation tag
  (identical/near-duplicate/more-complete/less-complete/diverged), a
  reveal-in-folder button, and an include/exclude toggle.
- **`components/PreviewPanel.tsx`** - renders whatever `preview.ts` says
  the file's kind is. Text preview diffs against the currently-selected
  *anchor* file when browsing relations (lines not present in the anchor
  are highlighted) - "what's different" is usually the actual question
  when comparing near-duplicate versions. Image/audio/video/pdf stream via
  `convertFileSrc()` (3.1's widened asset-protocol scope); audio gets a
  decorative static waveform (not derived from real audio data).
- **`components/ScanProgressPanel.tsx`** - the live 3-pane pipeline
  (Finding files / Checking for exact duplicates / Comparing document
  content), each pane showing its own status/counts/ETA, plus the embedded
  folder picker (below) and a Cancel button.
- **`components/FolderPicker.tsx`** (the "Bezel Scroll" component) - a
  scroll-snap picker-wheel for potentially 100+ folders: fixed-height rows,
  a companion drag/wheel/keyboard "dial" control, per-row `scale()`
  transform driven by distance from viewport center, left-aligned content
  with an independent right-aligned count column (each needs its *own*
  `transform-origin` - `left center` / `right center` respectively - a
  single shared origin causes a "staircase" drift as rows scale), a
  vertical mask-image fade at top/bottom, and a boundary-row exception
  (first/last few rows locked to scale 1, since a reduced top/bottom
  spacer means they can never reach true scroll-center under the normal
  falloff formula). Folder status: green check (done), pulsing (active),
  neutral (pending).
- **`components/TransferModal.tsx`** - requests the destination folder
  *only at transfer time*, not during scan (an explicit, deliberate
  redesign - the original flow asked for both source and destination
  upfront); shows live "copying X/Y files" progress and a final summary.

### 3.4 The duplicate-review workbench design

The GUI's actual shape is not SPEC.md section 9's originally-described
5-screen linear flow (Scan → Resume/reconcile → Review → Version history →
Transfer progress) - it evolved into a single-screen **duplicate-review
workbench**: color-coded file list (green/unique, orange/near-duplicate,
red/exact-duplicate) → click a file to see its relations tree, sorted by
similarity, each with reveal-in-folder and an include/exclude checkbox →
optional in-app content preview (text/image/audio/video/pdf) alongside the
relations tree. Transfer is a distinct, later step (3.3's `TransferModal`),
requesting the destination only when actually needed. This shape came from
iterative mockup-then-build design work, not from directly implementing
SPEC.md section 9 as originally written - worth deciding deliberately
whether the rewrite keeps this shape or returns to the original 5-screen
plan.

---

## Part 4: what's backend-complete but has no GUI screen

- **Manifest write / verify** (1.12): full engine support, Tauri commands
  exist and work, TypeScript types exist, **no UI calls them**.
- **Version history browsing**: `versions.rs` produces full family/ranking
  data; nothing in the GUI displays it as its own view (near-duplicate
  relations are visible per-file in the workbench, but not as a dedicated
  version-history screen per SPEC.md section 9).
- **Journal-backed run history**: the journal records everything; nothing
  surfaces it in the GUI.
- **Threshold calibration** (SPEC.md section 4): blocked on ~200
  hand-labeled real pairs, not built at all.
- **T4 embeddings**: SPEC.md's more advanced similarity tier, deferred
  since inception - never started.
- **P7 (perceptual image matching)**: the next roadmap phase after P8
  (this GUI) per `SPEC.md`/`CLAUDE.md` - not started; P8 was built ahead of
  it by deliberate reordering, since the GUI doesn't depend on the image
  tier existing.

---

## Dependency inventory

**`engine`**: `walkdir`, `rayon` (parallel hashing), `blake3` (T1/T3
content hashing), `serde`/`serde_json` (all report types), `thiserror`
(typed errors), `unicode-normalization` (NFC path-key folding), `rusqlite`
bundled (journal + manifest db), `sha2` (manifest checksums specifically -
BLAKE3 for analysis, SHA-256 for the manifest, per SPEC.md), `scraper`
(HTML text extraction), `csv`, `zip` (deflate only), `quick-xml`,
`calamine` (xlsx), `yaml-rust2`, `kamadak-exif`.

**`gui`**: `engine` (path dependency), `serde`/`serde_json`, `log` +
`tauri-plugin-log` (debug-only console logging, separate from `applog.rs`),
`tauri` (`protocol-asset` feature), `tauri-plugin-dialog`.

**`gui-frontend`**: `react`/`react-dom` 19, `@tauri-apps/api`,
`@tauri-apps/plugin-dialog`; dev: `@tauri-apps/cli`, `vite`, `typescript`,
`oxlint`.

No GPL dependencies anywhere (a hard rule) - every crate/package above was
chosen with that checked.
