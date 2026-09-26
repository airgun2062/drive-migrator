# Drive Migrator: specification

Status: design v1, written before any code. Update this file whenever a decision changes.

## Contents

1. Purpose and principles
2. Modes
3. Reconcile on every run
4. Detection pipeline
5. Coverage and version families
6. Large files and protected classes
7. Transfer engine
8. Manifest and tamper evidence
9. GUI (target screens)
10. Stack, layout, testing
11. Roadmap
12. Open questions

## 1. Purpose and principles

Two jobs, one engine:

- **Analyze:** find exact duplicates, near duplicates, and version families across one or two locations, using statistical scores.
- **Transfer:** copy a folder tree from a source location to a destination, preserving relative paths, with resume and optional deduplication.

Locations can be on the same drive, on different drives with the same OS, or on different filesystems and operating systems (for example an NTFS, exFAT, or ext4 drive mounted on the one machine running the tool). v1 assumes one machine with both locations mounted. A possible v2 runs a small agent on each machine and exchanges only fingerprints.

Principles:

1. The source is never modified or deleted. No code path writes to the source root.
2. Never trust saved progress. Every run re-scans both roots and reconciles against the journal (section 3).
3. A file at its final path is always complete and hash-verified (section 7).
4. Duplicate decisions are advisory until a person confirms them, except exact-hash duplicates and high-confidence cases the user has enabled for auto-apply.
5. Anything lossy (collapse without archive) is recorded in the manifest and shown in the UI as provenance, not backup.
6. Protect the app from careless users: tamper-evident manifest, dry-run, confirmation on destructive choices, safe defaults.

## 2. Modes

| Mode | Behavior |
|---|---|
| Analyze only | Scan, score, and report. Nothing is copied or changed. |
| Copy | Copy every file to the destination at the same relative path. Duplicate analysis is reported but removes nothing. |
| Collapse | Keep only the newest N versions per version family. Exact duplicates collapse to one copy. |

Collapse rules:

- "Keep newest N" (N >= 1) applies per version family (section 5).
- Collapsed versions are either not copied (they remain only on the source) or, with Archive enabled, copied to `_collapsed/<original relative path>` in the destination.
- A version is collapsed only if a kept version covers it above threshold tau. Diverged tips (versions not covered by any other version) are always kept, whatever N is.
- Exact duplicates are not versions: keep one copy and record all original paths in the manifest.
- Protected classes (section 6) are never collapsed, except as exact duplicates.
- Copy is the default. Move mode is out of scope for v1.
- A dry run shows the effect (files and bytes saved) before anything happens.

## 3. Reconcile on every run

At the start of every run, including a restart after interruption:

1. Scan the source and the destination from their roots.
2. Reuse cached fingerprints. Cache key: (volume, normalized path, size, mtime within tolerance, model version). Only new or changed files are re-processed.
3. Compare the source, the destination, and the job journal to classify each source file.

| State | Meaning | Default action |
|---|---|---|
| Verified | Same path, size, and hash in the destination | Skip |
| Partial | `.part` file or size mismatch | Discard and copy again (chunk resume for large files, section 6) |
| Missing | Not in the destination | Copy |
| Moved | Same hash at a different destination path | Ask: leave, move into place, or copy |
| Conflict | Same path, different content | Review with similarity and coverage scores: keep both, newer, or most complete |
| Blocked | Fails destination rules (section 7 preflight) | Review: rename or shorten |
| Destination-only | Not in the source | Never touched |

Also:

- Path identity uses a normalized key (Unicode NFC, forward slashes, case-folded) alongside the raw path. Raw paths are always preserved for writing.
- Deduplication runs over the union of source and destination, so a source file whose content already exists elsewhere in the destination is detected.
- Decisions are stored by content hash. On restart the options are: continue with saved decisions, re-review all duplicates, transfer without dedupe, or analyze only.
- The review queue never blocks copying: undisputed files transfer while ambiguous ones wait.
- Compare mtimes with a tolerance window (start at 2 seconds), because FAT and exFAT store local time at coarse precision. Fall back to hashing when ambiguous.
- Guard against a destination nested inside the source, or the reverse.
- Progress has two phases: analysis (scan, hash, similarity) and transfer. ETA uses an exponentially weighted moving average of files per second for hashing and bytes per second for copying, tracked separately.
- Fingerprint cache storage: SQLite (`rusqlite`, already a dependency for the journal and manifest - section 10), queried on demand per file (`SELECT ... WHERE path = ? AND size = ? AND modified = ?`) rather than loaded eagerly into memory as a whole. This keeps peak RAM independent of total cache size as it grows, at the cost of a per-lookup query instead of a single in-memory hashmap read - a worthwhile trade once the cache is large, since SQLite's own page cache manages what stays hot far better than materializing everything up front regardless of whether a given run needs it. The original implementation stores this as a single JSON file loaded whole into memory; migrating it to SQLite is planned, not yet done.

## 4. Detection pipeline

Run the cheapest tiers first. Each tier sees only what survives the previous one.

- **T1 Exact.** Group by size, then hash the first and last chunks, then a full BLAKE3 hash. Sampling may reject a candidate but never confirms one. BLAKE3 for analysis, SHA-256 for the manifest.
- **T2 Perceptual (required for images).** Images: must detect crops, not just resizes/recompression/minor edits. A plain global pHash or dHash is resize- and recompression-robust but degrades on cropping, since it removes part of the image and shifts the rest rather than preserving overall structure; the algorithm choice needs to be evaluated against that requirement specifically, not assumed from "pHash is standard." Candidates to evaluate: crop-resistant/local perceptual hash variants, or feature-point matching (ORB/SIFT-style keypoint matching), which is more robust to cropping than a single global hash but a bigger lift. Final algorithm choice deferred; the requirement (must survive cropping) is not. Video: keyframe fingerprints aligned in time (ffmpeg; check its license before bundling), optional. Evaluate `czkawka_core` (MIT) for this tier.
- **T3 Candidate generation for documents.** Shingle the text and use MinHash with LSH. Never compare all pairs.
- **T4 Semantic.** Chunk documents by paragraph or section, embed with a small local model (ONNX Runtime, no network), and compare with cosine similarity on candidates only.

### Formats

Case 1: identity by exact hash: pdf, png, jpg, jpeg, gif, bmp, tiff, eml, msg.

- pdf: exact hash. If the bytes differ, extract the text and treat it like case 2.
- images: exact hash, plus optional perceptual hash for re-saved or resized copies.
- eml: Message-ID plus normalized body. msg: parse the OLE container first.

Case 2: content similarity: docx, xlsx, pptx, csv, tsv, txt, md, html, htm, xml, json, yaml, yml, rtf, and source code.

| Format | Extract | Compare with |
|---|---|---|
| docx, pptx, rtf, txt, md | Plain text by section, markup stripped | Chunk embeddings |
| html, htm | Full raw text, markup NOT stripped | Chunk embeddings |
| xlsx, csv, tsv | Cell values per row, plus formulas | Row-level hashing and Jaccard |
| json, yaml, yml, xml | Parse and canonicalize (sorted keys, no whitespace) | Structural diff, then text |
| source code | Normalized text, optionally comments stripped | Token shingles and MinHash |

Embeddings are poor for numbers and code. Do not use them there.

Html/htm are extracted as full raw text rather than DOM-visible text only: two files can be generated from the same template but wired to different data through attribute values alone (`<script src="PPINetwork.js">` vs `<script src="@NETWORK_NAME@.js">`), which DOM-text extraction would miss entirely and score as identical. Markup syntax is not special-cased; shingling already splits on every non-alphanumeric character, so tags/attributes tokenize as ordinary words.

Docx, pptx, and xlsx also carry embedded media (images, and similar binary parts) that the text-only extraction above never sees, so two documents can score a perfect text match while actually differing in their pictures. For these three formats, embedded media entries are BLAKE3-hashed and compared as a set (not a multiset - a repeated image counts once) using the Sorensen-Dice coefficient, `2 * |A ∩ B| / (|A| + |B|)`. This produces a `media_match_score` (0 to 1, or absent when neither side has any media) reported alongside each pair. It is purely an additional, informational signal for a human choosing between versions - it does not feed into `coverage`/`jaccard_estimate` or the relationship classification in section 5, which stay text-only.

### Thresholds and grouping

- Calibrate on labeled data: hand-label about 200 pairs from real data (kept outside the repo), choose tau and the duplicate cutoff from the precision-recall curve, and store them with the model version in the job config. Cosine values are not comparable across models or domains.
- Report a confidence level with each group.
- Avoid transitive merging (A~B and B~C but A not~C). Use average-linkage clustering or verify pairwise.
- Cache extracted tokens/shingles and MinHash signatures per file, same SQLite-backed, on-demand-query storage as the fingerprint cache (section 3), keyed the same way (path, size, mtime). Extraction (reading and parsing every docx/pptx/xlsx/csv/etc.) is the expensive step; the duplicate cutoff and coverage threshold only affect the final, cheap classification pass over already-computed signatures. Caching the signatures means re-running with a different threshold - the normal way these get tuned in practice, since the defaults are uncalibrated (above) - costs a re-classification pass, not a full re-extraction of every file. Planned, not yet implemented.

## 5. Coverage and version families

Chunk-level asymmetric containment:

`coverage(A -> B)` = share of A's chunks whose best cosine match in B is at least tau.

Interpretation:

- A->B high and B->A low: B contains A plus more, so B is more complete.
- Both high: effectively the same, so the timestamp decides.
- Both moderate: diverged. Each has unique content. Flag for manual merge and never auto-pick.

For tables and code use row-level or token-shingle containment instead of embeddings.

Ordering versions: internal metadata first (docx/xlsx/pptx core properties: created, modified, last modified by, revision; EXIF date; email Date header), then coverage, then filesystem times as a last resort.

Families are graphs, not chains: X3 and X4 may both derive from X2. Tips (versions not covered by another version) are always kept.

Output per family: members, rank, coverage, dates, confidence.

## 6. Large files and protected classes

- Read each large file once. Compute the hash while copying. Verifying the destination is a setting: now, later, or sample.
- A file with a unique size cannot be a duplicate, so skip hashing it for dedupe.
- Chunked resume: for files over a threshold (start at 1 GiB), store a hash per 64 MiB chunk in the journal. On restart, verify the existing chunks and continue from the last good one. Smaller files restart from zero.
- Check free space up front, including the `.part` file. A same-drive copy doubles the space unless the filesystem supports cloning.
- Detect FAT32 and refuse files over 4 GB.
- Parallelism follows device type: sequential for spinning disks, parallel for SSDs, user-overridable.
- Stream with fixed buffers of a few MiB. Never load whole files.

| Class | Identity | Near-duplicate detection | Collapse |
|---|---|---|---|
| Raw research data | Full hash | None | Exact duplicates only, never keep-newest-N |
| Edited or exported video | Full hash | Frame fingerprints | Review only, never automatic. Quality (resolution, bitrate) is shown separately from recency. |
| Other large files | Full hash | Depends on type | Per the normal rules |

Raw data is immutable: a newer raw file is not a better version of an older one. Raw extensions are a configurable protected class. Per-format plugins may hash a canonical stream (for example a decompressed FASTQ). Folder-as-unit datasets (Zarr stores, editor project packages) are copied and hashed as one unit.

## 7. Transfer engine

Copy protocol per file: write to `<name>.part` in the destination directory, hash while writing, fsync, compare with the source hash, then atomically rename to the final name. Discard stray `.part` files at the start of a run.

Preserved by default: contents, relative path, modified time. Not preserved by default: permissions, ACLs, extended attributes, ownership (not portable). Ignore list: `.DS_Store`, `Thumbs.db`, `._*`.

Preflight compatibility report, before copying anything:

- Path length. Windows defaults to a 260-character limit. Long paths need the OS opt-in and the `\\?\` prefix in code.
- Illegal characters (`: * ? " < > |`) and reserved names (CON, NUL, and similar).
- Trailing dots and spaces.
- Case collisions on case-insensitive targets.
- NFC versus NFD name differences.
- The FAT32 4 GB file limit.

Same-drive fast paths: detect by device id. Use copy-on-write clones where supported (APFS, Btrfs, XFS, ReFS). Rename is for a future move mode.

Verification: flush before verifying. The OS cache can serve read-back from memory, so note that in the UI.

Journal: SQLite in WAL mode. It records intent and decisions. State is always re-verified against the disks.

## 8. Manifest and tamper evidence

Files at the destination root:

```
MIGRATION_MANIFEST.json            visible, warning first
MIGRATION_MANIFEST.json.sha256     checksum
.migrator/manifest.db              full record, source of truth (SQLite)
.migrator/manifest.backup.json     redundant copy
.migrator/manifest.backup.json.sha256
```

A hash is also stored in the app's own data folder on the machine.

- SHA-256 over canonical JSON (sorted keys, UTF-8, LF line endings, no extra whitespace), so reformatting does not trigger false alarms. The hash is computed by re-parsing whatever is on disk and re-serializing it canonically, not by hashing the file's raw bytes, so the visible file itself can still be pretty-printed for readability.
- Timestamps in the manifest (`created`) are Unix milliseconds, not RFC3339: no date/time crate is in section 10's candidate list, and this avoids adding one for formatting alone. Revisit if the GUI (P8) wants a human-readable string directly in the JSON.
- On open, compare the visible manifest, the backup, and the app-local hash. Use a two-of-three vote and offer repair.
- Set the read-only attribute on both files. Regenerate the JSON from the database if it is damaged.
- A Verify command re-scans the destination and reports files moved, renamed, changed, or deleted since the migration.
- The root file lists version families, duplicate groups, and a summary. Per-file records live in the database.
- Lookup order: content hash first (survives renames and moves), then path, then similarity against stored signatures.
- Collapsed files that were not archived exist only on the source. The manifest is provenance, not a backup, and the UI must say so.
- Optional "leave breadcrumbs" writes `<file>.versions.txt` next to kept files. Off by default.

Excerpt (schema v1):

```json
{
  "_warning": "GENERATED FILE. Edit at your own peril. The app checksums this file and will flag any change.",
  "format_version": 1,
  "created_unix_ms": 1790032432429,
  "version_families": [
    {
      "family_id": "f_00412",
      "keep_level": 2,
      "members": [
        {"rank": 1, "status": "kept", "dest_path": "Finance/2026/Q3/final/Q3_budget.docx", "sha256": "...", "modified": "2026-09-18"},
        {"rank": 2, "status": "kept", "dest_path": "Finance/2026/Q3/drafts/v3/Q3_budget.docx", "sha256": "...", "modified": "2026-09-12"},
        {"rank": 3, "status": "collapsed", "source_volume": "OldDrive", "source_path": "Finance/2026/Q3/drafts/v2/Q3_budget.docx", "sha256": "...", "coverage": 0.78}
      ]
    }
  ]
}
```

## 9. GUI (target screens)

Tauri with a TypeScript frontend. Lists must be virtualized (tens of thousands of rows). Avoid Slint (GPL-3.0 or commercial license).

1. **Scan:** choose source and destination, mode, and settings.
2. **Review:** duplicate groups with all locations, created and modified times, coverage, recommended keeper, confidence, and actions (compare, keep all, accept recommendation).
3. **Resume and reconcile:** analysis progress bars, plan counts per state (section 3), and the restart options.
4. **Version history:** mode selector, keep-newest slider, archive checkbox, family list showing kept, collapsed, or archived with each location, and search or drop-a-file lookup.
5. **Transfer progress:** bytes, files, ETA, and the review queue.

## 10. Stack, layout, testing

Rust workspace:

- `crates/engine` (library), `crates/cli` (binary `migrator`), later `crates/gui` (Tauri), `xtask/` (fixture generator).
- Engine modules: scan, cache, hash, reconcile, transfer, journal, manifest, similarity, extract, policy.

Candidate crates (from memory: verify maintenance and license before adding any): `walkdir` or `ignore` with `rayon` for parallel walking, `blake3`, `sha2`, `rusqlite`, `serde` and `serde_json`, `clap`, `thiserror`, `unicode-normalization`, `zip` with `quick-xml` (docx, pptx), `calamine` (xlsx), `csv`, `mail-parser` (eml), `cfb` (msg), `scraper` (html), `pdfium-render` (pdf), `ort` (embeddings), and optionally `czkawka_core` (verify API stability and dependency licenses first).

CI: GitHub Actions matrix on ubuntu, macos, and windows, running fmt, clippy, and tests.

Testing uses synthetic fixtures only: very long paths, Unicode names in NFC and NFD, case collisions, illegal Windows names, near-duplicate documents, large sparse files, and simulated interrupted copies. Never point a development build at real data.

## 11. Roadmap

Each phase should leave something usable. P1 to P4 already give a safe, verified, resumable copier with exact deduplication.

| Phase | Deliverable |
|---|---|
| P0 | Workspace skeleton, CI matrix, synthetic fixture generator |
| P1 | Parallel scan, size grouping, BLAKE3, fingerprint cache, `migrator analyze` (exact duplicates, text and JSON output) |
| P2 | Two-root reconcile plan (`migrator plan`), path normalization, preflight compatibility report, journal |
| P3 | Transfer engine: copy mode, `.part`, atomic rename, verify, resume (`migrator run`) |
| P4 | Manifest, tamper evidence, and the Verify command |
| P5 | Document extraction, MinHash, embeddings, coverage, and a threshold calibration tool |
| P6 | Collapse mode, version families, archive option |
| P7 | Large-file policy, protected classes, chunk resume, perceptual image tier (required, must detect crops - see section 4) and video tier (optional) |
| P8 | Tauri GUI |

## 12. Open questions

- Which raw research formats does the user have? This decides the format plugins and folder-as-unit rules.
- Embedding model: choose in P5 by benchmarking on the user's data.
- Move mode is out of v1. Revisit after P6.
- Two-machine operation (agents exchanging fingerprints) is deferred to v2.
- Perceptual image matching algorithm (T2): required capability confirmed, must survive cropping specifically (not just resize/recompression). Choose in P7 by evaluating candidates (crop-resistant hash variants vs. feature-point/keypoint matching) against real test images, since a plain global pHash/dHash is known to degrade on crops.
