//! The nightly load: once a night the incoming folder is moved into the
//! repository, as the Load page does. It waits for files that are still
//! being copied there.

use crate::App;
use astrofiler::ingest::IngestOptions;
use chrono::{Duration, Local, NaiveDateTime};
use std::path::Path;
use std::time::SystemTime;

pub const JOB: &str = "Nightly load";

/// Files changed more recently than this are taken to be still arriving.
const QUIET_MINUTES: i64 = 15;
/// How long a run waits for the folder to become quiet.
const GIVE_UP_HOURS: i64 = 6;

/// Which local date's time of day `at` was passed between two looks at the
/// clock. A program started after it waits for the next night.
fn due_day(prev: NaiveDateTime, now: NaiveDateTime, at: (u32, u32)) -> Option<chrono::NaiveDate> {
    [prev.date(), now.date()].into_iter().find(|day| {
        day.and_hms_opt(at.0, at.1, 0)
            .is_some_and(|t| prev < t && t <= now)
    })
}

fn should_schedule(
    last_run: &mut Option<(chrono::NaiveDate, (u32, u32))>,
    prev: NaiveDateTime,
    now: NaiveDateTime,
    at: (u32, u32),
) -> bool {
    let Some(day) = due_day(prev, now, at) else {
        return false;
    };
    if *last_run == Some((day, at)) {
        return false;
    }
    *last_run = Some((day, at));
    true
}

/// How many files are under `dir`, and when the latest was changed. A time
/// after `limit` is a telescope with a wrong clock, not a file that is still
/// arriving, and must not hide one that is.
fn newest(dir: &Path, limit: SystemTime, found: &mut (usize, Option<SystemTime>)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        // Ignore hidden files/folders and NAS metadata (@eaDir, #recycle).
        if e.file_name().to_string_lossy().starts_with(['.', '@', '#']) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        // Never follow symlinks so a cycle cannot overflow the stack.
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            newest(&e.path(), limit, found);
        } else if ft.is_file() {
            let Ok(meta) = e.metadata() else { continue };
            found.0 += 1;
            found.1 = found.1.max(meta.modified().ok().filter(|t| *t < limit));
        }
    }
}

enum Folder {
    Empty,
    /// Something was written to it a moment ago.
    Busy,
    Ready,
}

fn look(dir: &Path, now: SystemTime) -> Folder {
    let quiet = std::time::Duration::from_secs(QUIET_MINUTES as u64 * 60);
    let mut found = (0, None);
    newest(dir, now + quiet, &mut found);
    match found {
        (0, _) => Folder::Empty,
        (_, Some(t)) if t + quiet > now => Folder::Busy,
        _ => Folder::Ready,
    }
}

/// Move the incoming folder into the repository now, unless it is empty or
/// still being written to. Returns false when it should be tried again.
pub fn run(app: &App) -> bool {
    let cfg = app.cfg_saved();
    let src = cfg.source.clone();
    match look(&src, SystemTime::now()) {
        Folder::Empty => log::info!("{JOB}: nothing in {}", src.display()),
        Folder::Busy => {
            log::info!("{JOB}: files are still arriving in {}", src.display());
            return false;
        }
        Folder::Ready => {
            let opts = IngestOptions::MOVE.with_conflict(cfg.on_conflict);
            app.jobs.spawn(JOB, move |conn, cfg, p| {
                crate::pages::run_load(conn, cfg, p, &src, opts, false)
            });
        }
    }
    true
}

/// Watch the clock for as long as the program runs.
pub fn start(app: App) {
    std::thread::spawn(move || {
        let mut prev = Local::now().naive_local();
        // A run that is waiting for the folder: when to look again, and
        // when to stop trying.
        let mut waiting: Option<(NaiveDateTime, NaiveDateTime)> = None;
        let mut last_run: Option<(chrono::NaiveDate, (u32, u32))> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let now = Local::now().naive_local();
            let Some(at) = crate::pages::nightly_time(&app) else {
                (prev, waiting, last_run) = (now, None, None);
                continue;
            };
            if should_schedule(&mut last_run, prev, now, at) {
                waiting = Some((now, now + Duration::hours(GIVE_UP_HOURS)));
            }
            prev = now;
            let Some((again, until)) = waiting else {
                continue;
            };
            if now < again {
                continue;
            }
            waiting = if run(&app) {
                None
            } else if now + Duration::minutes(QUIET_MINUTES) > until {
                app.jobs.note(
                    true,
                    format!(
                        "{JOB}: skipped, files were still arriving after {GIVE_UP_HOURS} hours"
                    ),
                );
                None
            } else {
                Some((now + Duration::minutes(QUIET_MINUTES), until))
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
    }

    fn due(prev: NaiveDateTime, now: NaiveDateTime, at: (u32, u32)) -> bool {
        due_day(prev, now, at).is_some()
    }

    #[test]
    fn due_once_when_the_time_is_passed() {
        let at = (2, 0);
        assert!(!due(t("2026-10-05 01:59:00"), t("2026-10-05 01:59:30"), at));
        assert!(due(t("2026-10-05 01:59:30"), t("2026-10-05 02:00:00"), at));
        assert!(!due(t("2026-10-05 02:00:00"), t("2026-10-05 02:00:30"), at));
        // Started later in the day: the next night.
        assert!(!due(t("2026-10-05 10:00:00"), t("2026-10-05 10:00:30"), at));
        // Midnight, and a clock that was asleep over it.
        assert!(due(
            t("2026-10-05 23:59:50"),
            t("2026-10-06 00:00:20"),
            (0, 0)
        ));
        assert!(due(t("2026-10-05 23:00:00"), t("2026-10-06 03:00:00"), at));
    }

    #[test]
    fn dst_fallback_only_schedules_once_per_day() {
        let at = (1, 30);
        let mut last_run = None;
        // First pass of 01:30 (BST) triggers.
        assert!(should_schedule(
            &mut last_run,
            t("2026-10-25 01:29:40"),
            t("2026-10-25 01:30:10"),
            at
        ));
        // Clocks fell back from 02:00 to 01:00; passing 01:30 again does not re-trigger.
        assert!(!should_schedule(
            &mut last_run,
            t("2026-10-25 01:29:40"),
            t("2026-10-25 01:30:10"),
            at
        ));
        // Next night triggers normally.
        assert!(should_schedule(
            &mut last_run,
            t("2026-10-26 01:29:40"),
            t("2026-10-26 01:30:10"),
            at
        ));
    }

    #[test]
    fn nas_metadata_and_symlinks_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        std::fs::create_dir_all(tmp.path().join("@eaDir/sub")).unwrap();
        std::fs::create_dir_all(tmp.path().join("#recycle")).unwrap();
        std::fs::create_dir_all(tmp.path().join(".hidden")).unwrap();
        std::fs::write(tmp.path().join("@eaDir/sub/SYNOFILE_THUMB_M.jpg"), b"x").unwrap();
        std::fs::write(tmp.path().join("#recycle/old.fits"), b"x").unwrap();
        std::fs::write(tmp.path().join(".hidden/a.fits"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path(), tmp.path().join("loop")).unwrap();
        assert!(matches!(look(tmp.path(), now), Folder::Empty));
    }

    #[test]
    fn a_folder_being_written_to_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        assert!(matches!(look(tmp.path(), now), Folder::Empty));
        std::fs::create_dir_all(tmp.path().join("night")).unwrap();
        std::fs::write(tmp.path().join("night/a.fits"), b"x").unwrap();
        assert!(matches!(look(tmp.path(), now), Folder::Busy));
        let later = now + std::time::Duration::from_secs(16 * 60);
        assert!(matches!(look(tmp.path(), later), Folder::Ready));
        // A file dated in the future does not hold the load up...
        let earlier = now - std::time::Duration::from_secs(16 * 60);
        assert!(matches!(look(tmp.path(), earlier), Folder::Ready));
        // ...and does not hide one that is still arriving.
        std::fs::write(tmp.path().join("night/b.fits"), b"x").unwrap();
        let a = std::fs::File::options()
            .write(true)
            .open(tmp.path().join("night/a.fits"))
            .unwrap();
        a.set_modified(now + std::time::Duration::from_secs(86400))
            .unwrap();
        assert!(matches!(look(tmp.path(), now), Folder::Busy));
        assert!(matches!(look(tmp.path(), later), Folder::Ready));
    }
}
