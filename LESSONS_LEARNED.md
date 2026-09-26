# Lessons learned: hardening the P8 GUI against real data

This documents what running the P8 GUI against a real, messy research-data
folder (~88,000 files: nested lab folders, corrupted Office documents,
non-UTF-8 exports, cloud-synced paths) actually taught us, as opposed to
what the synthetic `xtask` fixtures and unit tests alone would have
surfaced. None of the bugs below were caught by the test suite first - they
were all found by a human running the app against real data, then fixed and
backed by a regression test afterward. That split (synthetic fixtures for
CI, real data for manual exploratory testing, never the other way around)
is deliberate and worked exactly as intended; keep doing it.

## The core principle: every external file operation must be time-boxed

The single biggest lesson. The engine's original resilience model (built
during the "no scan should throw an error" pass) was: a single bad file -
corrupted, permission-denied, deleted mid-scan - must never abort the whole
scan. That's necessary but not sufficient. Real drives don't just contain
bad files; they contain **slow** ones - specifically, files or folders on a
stalled network mount, or files under an unfinished cloud-sync (OneDrive
Files On-Demand, Dropbox smart sync, etc.) that block on `read()`, `open()`,
or even a directory listing while the sync client tries and fails to
materialize the content. A blocking syscall that never returns can't be
handled by "catch the error and skip" - there is no error, just an
indefinite hang. The whole application appeared as "(Not Responding)" more
than once during testing, and each time it was this, not a logic bug.

**The fix, applied uniformly:** any operation that touches a file or
directory outside this application's control - not a hard rule for the
app's own config/cache files, which are lower-risk - runs with a stall
timeout. If a single step goes 20 seconds with no response, it's abandoned:
logged, and treated exactly like any other single-file failure (skip, don't
abort the whole operation). 20 seconds was chosen as generous enough to
tolerate a slow-but-healthy network read, short enough that a genuinely
dead operation doesn't block for minutes. It is a plain constant per module
right now, not user-configurable - revisit if real usage shows it's wrong
in either direction.

There is no portable way in Rust to forcibly cancel an arbitrary blocking
syscall running on another thread. Every mechanism below only bounds how
long the *caller* waits for a stuck operation - the abandoned background
thread may keep running, and stay blocked, forever. That's an accepted,
bounded cost (one extra idle thread per genuinely-stalled occurrence, which
should be rare), not a bug to chase further.

### Two shapes of the same mechanism (`crates/engine/src/timeout.rs`)

- **`run_with_timeout(timeout, f)`** - a one-shot call: spawn `f` on a
  background thread, wait up to `timeout`, give up if it doesn't return.
  Used for bounded, single-value operations: one file's content extraction
  (`similarity.rs`), one sample-hash (`analyze.rs`).
- **`WatchedIter`** - streams many items from *one persistent* background
  worker, with a per-step timeout that resets on every item actually
  produced. Used for anything that scales with file/byte count: walking a
  directory tree (`scan.rs`), reading a large file chunk by chunk
  (`hash.rs`), the read+write+fsync loop of an actual file copy
  (`transfer.rs`).

The distinction matters for a non-obvious reason: spawning a fresh OS
thread per file is fine for a handful of calls but not for tens of
thousands (an 88k-file scan would pay real, user-visible overhead from
thread creation alone). `WatchedIter` pays that cost once per *stream* - a
handful of times per scan, not once per file - by reusing one worker thread
across every step, only replacing it (paying the thread-spawn cost again)
on the rare occasion a step actually stalls. A single whole-operation
timeout would also be wrong here for a different reason: a huge-but-healthy
file legitimately takes longer than a small one, so "no progress for 20s"
(per-step) is the correct trigger, not "total time exceeds 20s" (which
would misfire on large, healthy files and require an unboundable timeout to
avoid that).

### Where it's applied

| Operation | Mechanism | Module |
|---|---|---|
| Opening/reading each directory while walking | `WatchedIter` | `scan.rs` |
| One file's content extraction (docx/pptx/xlsx/csv/...) | `run_with_timeout` | `similarity.rs` |
| One file's sample-hash (bounded, ≤128 KiB) | `run_with_timeout` | `analyze.rs` |
| Streaming a whole file through BLAKE3/SHA-256 | `WatchedIter` | `hash.rs` |
| Copying a file: read, write, **and fsync** | `WatchedIter` | `transfer.rs` |

The fsync detail in `transfer.rs` is worth calling out: it would have been
easy to protect only the read/write loop and leave the final `sync_all()`
and mtime-preservation step running unprotected on the calling thread
afterward. Both were folded into the *same* background worker as the
copy loop instead, so a stalled fsync (which can absolutely happen on a
troubled disk or network destination) gets the same timeout coverage as a
stalled read, rather than being a second class of hang discovered later.

### Known remaining gap

The GUI's file-preview path (`read_text_preview` → `extract::preview_text`,
fired when a user clicks a file in the review workbench) is **not yet**
wrapped in this mechanism. Previewing a file that happens to sit on a
stalled path could still hang that one Tauri command. Lower priority than
the scan/transfer paths (it's not proven to freeze the whole window the way
the scan-phase hangs did, and it's scoped to whichever single file the user
clicked), but it's the same class of risk and should get the same
treatment eventually.

## Specific bugs, and why they're worth remembering

### A wrong hypothesis, corrected by reading the dependency's source

The first freeze investigated (during the walking phase) was assumed to be
`dir_entry.metadata()` blocking on a slow file. Wrong: with
`follow_links(false)` (which this app always uses), `walkdir`'s own source
shows that on Windows, `metadata()` returns an **already-cached** value
from the directory listing with zero additional syscalls. The real
blocking call is opening a *new directory* to descend into it (or, on
non-Windows, a fresh `stat()` for a symlink-following metadata call this
app doesn't even make). Lesson: when a hypothesis about *which specific
call* is blocking matters for the fix, verify it against the actual
dependency source rather than the plausible-sounding guess - the fix that
would have resulted from the wrong hypothesis (timing out `metadata()`)
would have done nothing for the actual hang.

### Reconstructing counts from a throttled event stream loses data

The GUI throttles scan-progress IPC events to ~1 per 80ms so hashing
(which can call back hundreds of times a second across parallel threads)
doesn't flood the webview. That's correct for a live progress bar. It's
**wrong** for anything that needs an exact count: a folder small/fast
enough to be walked entirely within one throttle gap has its whole file
count silently attributed to whichever folder's file happens to be
`current` when the next throttled event fires - the folder shows 0 files
and looks skipped (it wasn't), while a neighboring folder's count is
quietly inflated by the difference.

The fix wasn't a better reconstruction heuristic - any heuristic
reconstructing per-folder counts from a *lossy, rate-limited* stream will
have some failure mode like this. The fix was to stop reconstructing:
`ScanProgress::FolderWalked` is a new, **separate, never-throttled** event
emitted once per top-level folder with its exact, authoritative count.
Low-frequency, high-importance events must bypass whatever rate-limiting
protects the high-frequency ones, or they inherit its lossiness for free.
The throttled stream is still used for the live "counting up" visual while
a folder is active - just not trusted as the source of truth for the final
number.

### Directory traversal order is not display order

`walkdir` (like most directory-walking libraries) doesn't guarantee any
particular traversal order absent an explicit sort - it returns whatever
order the OS's directory listing happens to produce, which is often
close to creation order and never alphabetical. The GUI's folder picker
always displays folders name-sorted. Combining the two made progress
checkmarks appear scattered/non-contiguous (a folder near the top of the
displayed list still pending while one further down was already done),
which read as "folders are being skipped" even though nothing was actually
skipped - it was walked, just out of the order it was displayed in.

Fixed by making the walk itself deterministic and matching the display
order: process loose root-level files, then each immediate subdirectory,
sorted by name (`walkdir::WalkDir::sort_by_file_name()` for recursion
within each subdirectory too). This also made the walk's file order
deterministic run-to-run, which is a pleasant side effect for reasoning
about behavior, not just a cosmetic fix.

### Real research data breaks format assumptions constantly

Two related, recurring findings:

- **Corrupted Office documents are common, not exotic.** Real folders
  contained `.docx`/`.pptx`/`.xlsx` files that fail to open as valid zip
  archives ("Could not find EOCD") or are missing expected internal parts
  (`xl/_rels/workbook.xml.rels`). These are ordinary files a person can
  presumably still open in Word/Excel in some cases (partial corruption,
  a version mismatch, a file saved by a non-Microsoft tool) - not garbage.
  They must be skipped from comparison, logged clearly, and never allowed
  to abort the scan.
- **A file's extension doesn't guarantee its format is well-formed, but
  that doesn't mean it's not text.** A `.csv` with non-UTF-8 bytes or
  ambiguous delimiters, or a `.json`/`.yaml` file with a syntax error,
  still has real, comparable word content - excluding it entirely throws
  away information that plain-text shingling could still use. The rule
  that emerged: **if the underlying bytes are plausibly text, a structural
  parse failure falls back to plain-text treatment rather than exclusion.**
  This does *not* extend to zip-based binary containers (docx/pptx/xlsx) -
  a corrupted zip's raw bytes were never text, so there's nothing
  meaningful to fall back to; those stay excluded when corrupted.

## The logging system exists because screenshots don't scale

Debugging the freezes above by screenshot alone was slow and ambiguous -
a screenshot shows *that* something is stuck, never *which specific file or
directory step* it's stuck on. `crates/engine/src/applog.rs`'s
`Log::info(tag, message)` / `Log::error(tag, message)`, writing to
`drive-migrator/logs/<YYYY-MM-DD-HH>.log` (hourly rotation, resolved via
`CARGO_MANIFEST_DIR` so the path is stable regardless of `cargo tauri dev`'s
actual working directory), turned "the app is frozen" into "the log's last
line names the exact file/folder, and its timestamp says how long ago" -
that's what actually confirmed the extraction-phase freeze days after the
walking-phase one was fixed, rather than another round of screenshot
guessing.

Practical note for next time: `cargo test` and a real running GUI instance
write into the *same* `drive-migrator/logs/` directory (both resolve it via
the same `CARGO_MANIFEST_DIR`-relative path). Running the test suite while
a real scan's log is being reviewed will interleave the two. Low-stakes
since the directory is gitignored and disposable, but worth knowing if a
log looks like it has unrelated noise in it - it might be test output, not
production activity.

## Process lesson: don't rewrite working, hard-won code

Partway through this hardening pass, we considered squashing/rewriting the
whole P8 GUI branch from scratch to get a "clean" history. Decided against
it: every one of the fixes above was found by real-world testing that a
rewrite would have no way to reproduce except by accident - a clean rewrite
risks silently dropping or subtly re-breaking any of them, trading a known,
tested state for an unknown one. The lower-risk way to get a clean result:
finish the in-flight work, keep testing, and use `git rebase -i` to
reorganize the *already-validated* commits into a smaller number of
well-labeled ones once everything is green - that rewrites history, not
logic, so it can't reintroduce a bug the original commits already fixed.
