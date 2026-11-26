# AstroFiler Web Audit

- **Repository:** `peterbuitho/astrofiler-web`
- **Audited commit:** `e0b05aa` (`main`, post-`v0.3.19`, `astrofiler` `v0.8.8`)
- **Date:** 2026-11-26
- **Automated checks run:**
  - `cargo test --locked --offline` — **30 passed, 0 failed**
  - `cargo clippy --all-targets --locked --offline -- -D warnings` — **clean**
  - `cargo fmt --check` — **clean**

---

## Executive Summary

`astrofiler-web` is a compact, well-structured Rust/Axum + Maud + HTMX application (~3.8k LOC) with solid integration tests and clear security awareness (already featuring `Origin` checks, `home_name` host restrictions when unauthenticated, SQLite `query_only` + progress timeout, panic catching in background jobs, and graceful shutdown).

This audit identified **4 High**, **7 Medium**, and **7 Low** findings spanning path-confinement bugs, clickjacking, open redirect, Watchtower deployment risks, multi-threaded `std::env::set_var` undefined behavior, NAS-specific directory traversal hazards, concurrency/blocking-I/O issues, and minor logic/UX bugs.

### Remediation Status (2026-11-26)

All actionable findings below have been fixed in the working tree with regression tests added, except for two intentional/upstream design notes:
- **H-1 (non-clean endpoints):** `ASTROFILER_WEB_ROOT` remains a UI picker boundary for `/load`, `/images/export`, `/sessions/export`, and `/settings` (some workflows/tests load from outside `ASTROFILER_WEB_ROOT`); `POST /batch/clean` now strictly enforces resolution inside `ASTROFILER_WEB_ROOT`.
- **M-3 (cancellation in upstream `astrofiler`):** `batch_clean` delete mode now serializes on the `Jobs` writer queue; making `astrofiler::batch::{delete_files, clean_previews, remove_duplicates, remove_empty_dirs, layout_plan}` honor `JobState::cancel` requires upstream changes in `astrofiler-rs`.

| ID | Severity | Area | Status | Summary |
|---|---|---|---|---|
| H-1 | **High** | Security / Data Loss | **Fixed (`batch_clean`)** | `POST /batch/clean` now verifies canonical target is a strict subfolder of `ASTROFILER_WEB_ROOT` (other path endpoints intentionally keep `ASTROFILER_WEB_ROOT` as UI-only) |
| H-2 | **High** | Security (Clickjacking) | **Fixed** | Added `X-Frame-Options: DENY`, `CSP frame-ancestors 'none'`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin` |
| H-3 | **High** | Security | **Fixed** | `POST /jobs/cancel` rejects protocol-relative (`//`) and `/\` `Referer` redirects |
| H-4 | **High** | Deployment / Ops | **Fixed** | Watchtower in `compose.yaml` moved behind opt-in `autoupdate` profile with `--stop-timeout 120s` and risk comment |
| M-1 | **Medium** | Soundness (UB) | **Fixed** | `RAYON_NUM_THREADS` is set in sync `main()` before constructing the multi-threaded Tokio runtime |
| M-2 | **Medium** | Reliability / NAS | **Fixed** | `nightly::newest` uses `file_type()`, skips symlinks and `.`/`@`/`#` entries; `nightly::start` deduplicates per `(NaiveDate, at)` across DST fall-back |
| M-3 | **Medium** | Concurrency | **Partially fixed** | `batch_clean` delete mode now uses the write queue; cancellation inside `astrofiler` batch functions remains an upstream enhancement |
| M-4 | **Medium** | Concurrency / I/O | **Fixed** | Moved FS/SQLite calls in page handlers into `blocking(...)`/`tokio::fs`; added atomic temp-file+rename writes; `settings_save` reports all sub-step errors independently |
| M-5 | **Medium** | Security / DoS | **Fixed** | Added SQLite `Limit` caps in `sql_ids` and graceful job-panel error handling in `Selection::ids()` |
| M-6 | **Medium** | Security (Auth/CSRF) | **Fixed** | Added `Sec-Fetch-Site`, `Referer` fallback, case-insensitive/default-port `Origin` vs `Host`, strict bracketed IPv6 + `.local` handling, and constant-time case-insensitive Basic Auth |
| M-7 | **Medium** | Logic Bug | **Fixed** | Explicit `"SH2"` prefix match in `filter::is_designation` |
| L-1 | **Low** | Frontend Bug | **Fixed** | Grouped Shift-click syncs parent/member checkboxes in `assets/app.js` |
| L-2 | **Low** | UX Bug | **Fixed** | Idle `#jobs` polls return `204 No Content`; `assets/app.js` preserves open `<details class="notes">` on busy swaps |
| L-3 | **Low** | Performance | **Fixed** | `Selection::ids()` uses `HashSet<&str>` lookups |
| L-4 | **Low** | Correctness | **Fixed** | Escaped `s.id` in `sessions_page`, guarded `image_file` metadata/`is_file()`, URL-encoded `f.id`, guarded `exptime` with `is_finite() && >= 0.0` |
| L-5 | **Low** | Input Validation | **Fixed** | Rejected empty `repo` in `settings_save`, filtered empty env vars in `load_config`, and guarded `repo` / ancestor paths in `filed_already` |
| L-6 | **Low** | Observability | **Fixed** | `AppError::into_response` logs errors via `log::error!` |
| L-7 | **Low** | Packaging / CI | **Fixed** | Updated `.gitignore`, `.dockerignore`, and `.github/workflows/docker.yml` (`main`/PR CI, `fmt`, `clippy`, `permissions: contents: read`) |

---

## High Findings

### H-1. Incomplete path confinement in `POST /batch/clean` (and missing root checks on other path endpoints)
- **Locations:** `src/pages.rs:1250-1260` (also `src/pages.rs:782-807`, `666-693`, `1010-1034`, `1638-1641`)
- **Details:**
  In `batch_clean`:
  ```rust
  let real = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
  if !dry_run && real(&app.root).starts_with(real(&dir)) {
      app.jobs.note(
          true,
          format!("Clean previews: choose a folder inside {}", app.root.display()),
      );
      return Redirect::to("/batch");
  }
  ```
  `real(&app.root).starts_with(real(&dir))` only blocks `dir == app.root` or an ancestor of `app.root`. Any **sibling or unrelated directory outside `app.root`** (e.g. `app.root = /photo/Astro`, `dir = /photo/FamilyPhotos`), as well as any symlink inside `app.root` whose canonical target lies outside `app.root`, evaluates to `false` and proceeds to **delete every JPG and PNG** in that directory—directly contradicting the handler's own error message.

  More broadly, `ASTROFILER_WEB_ROOT` is only enforced by `/browse` (`src/pages.rs:1794-1800`). `POST /load`, `POST /images/export`, `POST /sessions/export`, and `POST /settings` accept arbitrary filesystem paths directly from form input.
- **Recommendation:**
  - In `batch_clean`, require `dir` to resolve strictly inside `app.root`:
    ```rust
    let root = real(&app.root);
    let target = real(&dir);
    if !dry_run && (!target.starts_with(&root) || target == root) { ... }
    ```
  - Enforce `target.starts_with(&root)` consistently on `POST /load`, `POST /images/export`, `POST /sessions/export`, and `POST /settings` (or explicitly document that `ASTROFILER_WEB_ROOT` is UI-only and rely on container mounts for isolation, while still fixing `batch_clean`).

### H-2. Clickjacking bypasses `Origin` CSRF protection
- **Locations:** `src/main.rs:137-172`
- **Details:**
  No `X-Frame-Options` or `Content-Security-Policy: frame-ancestors 'none'` header is sent on responses. On a home LAN (where `ASTROFILER_WEB_PASSWORD` is unset by default, or where the browser has cached HTTP Basic Auth credentials), an external website visited by the user can embed `http://<nas>:8080/batch` (or `/settings`, `/load`, `/mappings`) inside an `<iframe>`.

  When a user is tricked into clicking a button inside the iframe, the resulting POST originates from the framed `http://<nas>:8080` document, so `Origin` matches `Host` and passes `cross_site()`. Several state-changing endpoints have no JavaScript `confirm()` dialog (e.g. `/batch/merge`, `/batch/run` for `forget`/`empty_dirs`/`layout_migrate`, `/load`, `/settings`, `/mappings/remove`).
- **Recommendation:**
  Add defensive response headers in the `auth` middleware (or a dedicated Tower header layer):
  - `X-Frame-Options: DENY`
  - `Content-Security-Policy: frame-ancestors 'none'`
  - `X-Content-Type-Options: nosniff`
  - `Referrer-Policy: same-origin`

### H-3. Open redirect in `POST /jobs/cancel` via `Referer`
- **Locations:** `src/pages.rs:1750-1764`
- **Details:**
  ```rust
  let back = headers
      .get(header::REFERER)
      .and_then(|v| v.to_str().ok())
      .and_then(|r| r.split_once("://"))
      .and_then(|(_, rest)| rest.find('/').map(|i| rest[i..].to_string()))
      .unwrap_or_else(|| "/images".into());
  Redirect::to(&back)
  ```
  If `Referer` is `http://nas:8080//evil.example/phish`, `rest[i..]` is `//evil.example/phish`. `Redirect::to` emits `Location: //evil.example/phish`, which browsers follow as a protocol-relative redirect to `http://evil.example/phish`.
- **Recommendation:**
  Only redirect to paths that start with a single `/` and not `//` or `/\` (or restrict to known tab prefixes from `ui::TABS`, falling back to `"/images"`).

### H-4. `compose.yaml` Watchtower risks: unmaintained image, root Docker socket, and 10s stop timeout
- **Locations:** `compose.yaml:6-7, 29-38`
- **Details:**
  1. `containrrr/watchtower` is unmaintained/archived (last release 2023; breaks on newer Docker daemon minimum API versions and receives no security fixes) and is pulled via unpinned `:latest`.
  2. Mounting `/var/run/docker.sock` gives Watchtower root-equivalent control over the NAS host, and auto-updating `ntmb/astrofiler-web:latest` without digest verification means a compromised Docker Hub token (`DOCKERHUB_TOKEN`) or CI pipeline achieves remote code execution on the NAS within 24 hours.
  3. **Data-integrity hazard:** `compose.yaml:7` sets `stop_grace_period: 2m` on `astrofiler` specifically so running tasks can finish cleanly without leaving the catalogue out of sync with disk. However, `command: astrofiler --interval 86400 --cleanup` omits `--stop-timeout`. `containrrr/watchtower` defaults to `--stop-timeout 10s`, which overrides Docker's `stop_grace_period` and can `SIGKILL` `astrofiler` after 10 seconds mid-job.
- **Recommendation:**
  - Make Watchtower optional (e.g. commented out or under a Compose `profile`) rather than enabled by default, or use an actively maintained updater / notification-only mode (`--monitor-only`).
  - If auto-updating is kept, pass `--stop-timeout 120s` (and a `--schedule` outside the nightly load window) so it matches `stop_grace_period: 2m`.

---

## Medium Findings

### M-1. Undefined Behavior: `std::env::set_var` called inside multi-threaded Tokio runtime
- **Locations:** `src/main.rs:199-211`
- **Details:**
  `#[tokio::main]` initializes the multi-threaded Tokio worker pool before entering `main()`. Calling `std::env::set_var("RAYON_NUM_THREADS", ...)` while worker threads exist is UB on POSIX systems (concurrent `getenv`/`setenv` data race) and is `unsafe` in Rust 2024.
- **Recommendation:**
  Replace `#[tokio::main]` with a synchronous `fn main()` that sets `RAYON_NUM_THREADS` (or configures `rayon::ThreadPoolBuilder::new().num_threads(...).build_global()`) *before* building and running the Tokio runtime.

### M-2. `nightly::newest` symlink crash, Synology `@eaDir`/`#recycle` traversal, and DST fall-back double-run
- **Locations:** `src/nightly.rs:20-43, 87-101`
- **Details:**
  1. **Stack overflow on symlink cycles:** `e.metadata()` follows symlinks (`stat`). A cyclic symlink inside the incoming folder causes unbounded recursion in `newest()` and crashes the server process.
  2. **Synology metadata folders:** `newest()` descends into every directory, including hidden directories (`.Trash`, etc.) and Synology's `@eaDir` / `#recycle` folders (which `browse()` explicitly filters at `src/pages.rs:1806`). Non-dot files inside `@eaDir` (e.g. `SYNOFILE_THUMB_M.jpg`) cause `look()` to treat an empty inbox as `Folder::Ready` or `Folder::Busy`.
  3. **DST fall-back double execution:** `due()` compares `Local::now().naive_local()`. On fall-back nights (e.g. `TZ=Europe/London` for times between `01:00` and `01:59`), the local clock passes the scheduled time twice, triggering the nightly load twice.
- **Recommendation:**
  - Use `e.file_type()` (does not follow symlinks) and skip symlinks as well as directories starting with `.`, `@`, or `#`.
  - Track the last triggered `(NaiveDate, (u32, u32))` or use timezone-aware `DateTime<Local>` so a scheduled run executes at most once per local calendar day.

### M-3. `batch_clean` delete mode bypasses writer queue; multiple destructive jobs ignore cancellation
- **Locations:** `src/pages.rs:699, 1197, 1206, 1261, 1310`, `src/ui.rs:68-77`
- **Details:**
  1. `batch_clean` calls `app.jobs.spawn_read(...)` even when `dry_run == false`. Deleting JPG/PNG files and empty folders can run concurrently with `Load` (which files processed JPG/PNG pictures), `Update folders`, or `Merge objects`.
  2. `Delete` (`batch::delete_files`), `Clean previews` (`batch::clean_previews`), `Remove duplicates` (`batch::remove_duplicates`), `Remove empty folders` (`batch::remove_empty_dirs`), and `Check folders` (`batch::layout_plan`) ignore the `JobState` argument (`_`). The UI still shows a **Cancel** button for every running job, and `Jobs::shutdown()` sets `cancel = true` and waits—meaning neither user cancellation nor `SIGTERM` can interrupt these tasks.
- **Recommendation:**
  - Use `app.jobs.submit("Clean previews", !dry_run, ...)` so preview deletion serializes with other writers.
  - Thread `JobState` cancellation support into `astrofiler` for long-running delete/clean/duplicate operations, or hide the Cancel button for non-cancellable jobs.

### M-4. Blocking I/O on async threads, unqueued SQLite writes, and non-atomic file writes
- **Locations:** `src/main.rs:53-60`; `src/pages.rs:736, 784, 794, 803, 1240, 1250, 1374, 1390, 1485, 1504, 1529, 1552, 1562-1575, 1613, 1671-1696`
- **Details:**
  1. **Blocking on async executor:** `AppState::cfg()` opens SQLite and queries nicknames synchronously; `settings`, `settings_save`, `stats_page`, `stats_fresh`, `load`, `load_start`, and `batch_clean` perform synchronous disk/SQLite I/O on Tokio async threads. On a NAS with slow/spinning disks or SQLite lock contention, this stalls unrelated HTTP requests (including `/jobs` polls).
  2. **Unqueued SQLite writes:** `settings_save` (`nick::replace`), `mappings_add` (`db::add_mapping`), and `mappings_remove` (`db::remove_mapping`) write to SQLite directly outside the `Jobs` single-writer queue, risking `SQLITE_BUSY` during background write jobs. In `settings_save`, if `nick::replace` fails, the closure exits early before writing `stats-names.txt` and `nightly.txt`, leaving settings half-saved even though `astrofiler.ini` and `app.cfg` were already updated.
  3. **Non-atomic `std::fs::write`:** `stats.html`, `stats-names.txt`, and `nightly.txt` are written via truncating `std::fs::write`. Concurrent requests or a crash mid-write can produce truncated files—notably `stats.html`, which `stats_page` injects raw via `maud::PreEscaped`.
- **Recommendation:**
  - Move synchronous DB/FS calls into `blocking(...)` (or `tokio::fs`).
  - Write `stats.html`, `stats-names.txt`, and `nightly.txt` via write-to-temp-file + atomic `rename`.
  - Handle each secondary save step in `settings_save` independently (or collect all errors rather than short-circuiting after `c.save()`), and ensure SQLite connections have an adequate `busy_timeout`.

### M-5. SQL condition (`sql_ids`) memory-exhaustion DoS, clause breakout, and uncapped concurrency
- **Locations:** `src/pages.rs:51-73, 284-298, 612`
- **Details:**
  1. **Memory DoS:** `progress_handler` caps execution time at `SQL_SECONDS` (10s), but does not cap memory allocation. A condition such as `length(zeroblob(1000000000)) > 0` or `randomblob(...)` can allocate gigabytes immediately and OOM-kill the container on a memory-constrained NAS.
  2. **Clause breakout:** Raw `{clause}` is wrapped in `WHERE COALESCE(fitsFileSoftDelete,0)=0 AND (\n{clause}\n)`. A condition containing `)` (e.g. `1=1) OR 1=1 --`) breaks out of the grouping. Currently `query_only = true` blocks writes and `matching()` intersects IDs against `all_files(..., false)`, preventing direct leaks or soft-delete bypasses, but allowing arbitrary `UNION SELECT` / subqueries against other tables as a boolean oracle.
  3. **Concurrency & error handling:** Repeated `/images/rows?sql=...` requests can saturate the Tokio blocking thread pool for 10s each (and disconnect does not cancel the SQLite query). In `Selection::ids()` (`src/pages.rs:612`), a syntax error or timeout in `sql` bubbles up as a raw HTTP 500 instead of a friendly job-panel note.
- **Recommendation:**
  - Set conservative SQLite limits on the read-only connection (e.g. `conn.set_limit(Limit::SQLITE_LIMIT_LENGTH, 1_000_000)` and `PRAGMA hard_heap_limit` if supported by the bundled SQLite).
  - Return a user-facing note + redirect when `sql_ids` fails inside `Selection::ids()`.
  - Consider bounding concurrent `/images/rows` queries (e.g. a small `tokio::sync::Semaphore`).

### M-6. Auth & CSRF middleware edge cases
- **Locations:** `src/main.rs:112-166`
- **Details:**
  1. `cross_site`: permits POST requests when the `Origin` header is absent (no fallback check on `Sec-Fetch-Site: cross-site` or `Referer`), and compares `Origin` host to `Host` case-sensitively without normalizing default ports (`:80` / `:443`).
  2. `home_name`:
     - `name.starts_with('[')` returns `true` for any string starting with `[` rather than validating an IPv6 literal (`Ipv6Addr`).
     - `name.ends_with(".local")` is case-sensitive (`NAS.LOCAL` gets 403).
     - `host.is_some_and(...)` allows requests with no `Host` header (HTTP/1.0).
  3. Basic Auth (`src/main.rs:153, 157`): `strip_prefix("Basic ")` is case-sensitive (RFC 7617 specifies case-insensitive scheme), and `given.as_deref() == Some(password.as_str())` uses non-constant-time comparison.
- **Recommendation:**
  - Reject POST requests with `Sec-Fetch-Site: cross-site`, and compare `Origin`/`Host` case-insensitively.
  - Validate IPv6 bracketed literals properly, lowercase before checking `.local`, and reject missing `Host` when password is unset.
  - Use constant-time comparison (e.g. `subtle` or fixed-length SHA-256 digest comparison) and case-insensitive `Basic ` prefix matching.

### M-7. Unreachable `"SH2"` catalogue prefix in `filter::is_designation`
- **Locations:** `src/filter.rs:20-32`
- **Details:**
  ```rust
  const CATALOGUES: [&str; 14] = [
      "M", "NGC", "IC", "C", "SH2", "LDN", "LBN", "B", "ABELL", "VDB", "PGC", "UGC", "MEL", "CR",
  ];
  let digits = key.trim_start_matches(|c: char| c.is_ascii_alphabetic());
  let prefix = &key[..key.len() - digits.len()];
  ```
  Because `trim_start_matches(is_ascii_alphabetic)` stops before the digit `'2'`, `prefix` for `"SH2101"` (`Sh2 101` / `Sh2-101`) is `"SH"`, never `"SH2"`. Searching for an uncatalogued Sharpless object therefore falls back to loose-word matching (`"sh2"` and `"101"`), matching unrelated timestamps/temperatures containing `101`.
- **Recommendation:**
  Match `"SH2"` explicitly (e.g. `key.strip_prefix("SH2")` before the generic alphabetic prefix split) and add a unit test for `"Sh2 101"`.

---

## Low Findings

### L-1. Shift-click selection desync in grouped Images view
- **Locations:** `assets/app.js:18-29, 69-78`
- **Details:**
  Shift-click iterates over `document.querySelectorAll('input.row')` and sets `boxes[i].checked = c.checked` directly without firing `change` events or updating parent `input.grp` checkboxes. Unticking member rows inside a ticked group via Shift-click leaves the parent `input.grp` checkbox checked, so the server still acts on the entire group (including the visually unticked rows) on Delete/Edit/Export.
- **Recommendation:**
  Sync parent `input.grp` checkboxes (or restrict Shift-click range selection to sibling checkboxes at the same level) and recalculate selection state accordingly.

### L-2. Job notes dropdown collapses every 1–3 seconds
- **Locations:** `src/ui.rs:66-96`
- **Details:**
  `#jobs` polls `/jobs?gen=...` every 1s (busy) or 3s (idle) with `hx-swap="outerHTML"`, replacing the entire `<details class="notes">` element and closing it whenever the user expands it to read older notes.
- **Recommendation:**
  Preserve open state across swaps (e.g. only re-render when snapshot changed, or restore `<details open>` via `htmx:beforeSwap`/`htmx:afterSwap`, or return HTTP `204 No Content` when idle and `generation == q.gen`).

### L-3. Quadratic selection filtering in `Selection::ids()`
- **Locations:** `src/pages.rs:615-623`
- **Details:**
  `self.id.contains(&f.id)` and `self.grp.contains(...)` perform linear scans on `Vec<String>` for every file in the catalogue ($O(N \times M)$).
- **Recommendation:**
  Collect `self.id` and `self.grp` into `HashSet<&str>` before filtering.

### L-4. Minor correctness issues in `sessions_page`, `image_file`, and exposure sums
- **Locations:** `src/pages.rs:344-348, 525, 547, 935`
- **Details:**
  - `sessions_page` (`line 935`) interpolates `s.id` directly into `hx-vals={"{\"id\":\"" (s.id) "\"}"}` without JSON escaping (unlike `group_vals` at `line 470`).
  - `image_file` (`line 547`) falls back to `Content-Length: 0` if `f.metadata().await` fails while still streaming the file body, and `file_table` (`line 525`) does not URL-encode `f.id` in `/images/file?id=...`.
  - Exposure sums in `image_rows` (`line 344-348`) parse `f64` without `.filter(|v| v.is_finite() && *v >= 0.0)`, so a corrupt `NaN`/`Inf` FITS `EXPTIME` poisons group and total durations.
- **Recommendation:**
  Reuse JSON escaping for `sessions_page`, omit `Content-Length` (or return 404/500) if `metadata()` fails or `!is_file()`, URL-encode `f.id`, and guard `exptime` with `is_finite()`.

### L-5. Input validation gaps in `settings_save`, `load_config`, and `filed_already`
- **Locations:** `src/main.rs:179-184`; `src/pages.rs:823-833, 1640-1641`
- **Details:**
  - `settings_save` saves empty `repo` or `source` paths without validation.
  - `load_config` does not filter empty `ASTROFILER_WEB_REPO` / `ASTROFILER_WEB_INBOX` values (unlike `AppState::new`).
  - `filed_already` only rejects `src` inside `MANAGED_DIRS`, not `cfg.repo` itself or an ancestor of `cfg.repo` (relying on `astrofiler::ingest::ingest_folder` to protect managed folders if a user selects `/photo` or `/photo/Astro` for a Move).
- **Recommendation:**
  Reject empty `repo` in `settings_save`, filter empty env vars in `load_config`, and explicitly verify/guard the case where `src` is an ancestor of `cfg.repo` in `Load`.

### L-6. `AppError` exposes raw internal errors without logging them
- **Locations:** `src/main.rs:89-93`
- **Details:**
  `AppError::into_response` sends `format!("{:#}", self.0)` directly in the 500 body (potentially exposing internal filesystem paths or SQLite internals) and does not call `log::error!`, making 500 errors invisible in the Log page.
- **Recommendation:**
  Log `log::error!("request failed: {:#}", self.0)` inside `IntoResponse` (and consider returning a generic message when an error is unexpected).

### L-7. Packaging, `.dockerignore`/`.gitignore`, and CI/CD hardening
- **Locations:** `Dockerfile`, `.dockerignore`, `.gitignore`, `.github/workflows/docker.yml`
- **Details:**
  - `Dockerfile` uses floating tags (`rust:1-alpine`, `alpine:3`), defines no non-root `USER` fallback, and includes no `HEALTHCHECK`.
  - `.dockerignore` only ignores `target`, `.git`, `.ua/`—leaving `.claude/settings.local.json` (containing local developer paths), `.DS_Store`, `.github/`, and docs in the Docker build context.
  - `.gitignore` does not exclude `.DS_Store`.
  - `.github/workflows/docker.yml` only triggers on `v*` tags and manual dispatch (no PR or `main`-branch CI), does not run `cargo clippy` or `cargo fmt --check`, omits top-level `permissions: contents: read`, and pins third-party Actions by mutable tags instead of commit SHAs.
- **Recommendation:**
  - Add a CI trigger on `push` to `main` and `pull_request` running `cargo test --locked`, `cargo clippy --all-targets --locked -- -D warnings`, and `cargo fmt --check`.
  - Pin base images/Actions, set `permissions: contents: read` at workflow level, add a non-root `USER` / `HEALTHCHECK` in `Dockerfile`, and tighten `.dockerignore` and `.gitignore`.

---

## Suggested Fix Order

1. **Immediate safety fixes:** H-1 (`batch_clean` root check), H-2 (framing/security headers), H-3 (`jobs_cancel` redirect check), H-4 (Watchtower stop timeout & opt-in/replacement).
2. **Reliability & soundness:** M-1 (`RAYON_NUM_THREADS` before Tokio runtime), M-2 (`nightly::newest` symlink/NAS filters + DST guard), M-3 (`batch_clean` on write queue), M-7 (`SH2` prefix fix).
3. **Hardening & polish:** M-4 (blocking I/O + atomic writes + settings save resilience), M-5 (SQLite memory limits), M-6 (auth/CSRF edge cases), and Low items (L-1 through L-7).
