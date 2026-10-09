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

/// Whether the time of day `at` was passed between two looks at the clock.
/// A program started after it waits for the next night.
fn due(prev: NaiveDateTime, now: NaiveDateTime, at: (u32, u32)) -> bool {
    [prev.date(), now.date()].iter().any(|day| {
        day.and_hms_opt(at.0, at.1, 0)
            .is_some_and(|t| prev < t && t <= now)
    })
}

/// How many files are under `dir`, and when the latest was changed. A time
/// after `limit` is a telescope with a wrong clock, not a file that is still
/// arriving, and must not hide one that is.
fn newest(dir: &Path, limit: SystemTime, found: &mut (usize, Option<SystemTime>)) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_dir() {
            newest(&e.path(), limit, found);
        } else if !e.file_name().to_string_lossy().starts_with('.') {
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
        loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let now = Local::now().naive_local();
            let Some(at) = crate::pages::nightly_time(&app) else {
                (prev, waiting) = (now, None);
                continue;
            };
            if due(prev, now, at) {
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
