//! Background tasks, as in the desktop app: one task that changes the
//! catalogue at a time (SQLite has a single writer), the rest wait in a
//! queue; read-only tasks run alongside.

use anyhow::Result;
use astrofiler::config::Config;
use astrofiler::db;
use astrofiler::progress::JobState;
use rusqlite::Connection;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, RwLock};

pub type Work = Box<dyn FnOnce(&mut Connection, &Config, &JobState) -> Result<String> + Send>;

const KEEP_NOTES: usize = 6;

pub struct Running {
    pub name: String,
    pub state: Arc<JobState>,
    pub writes: bool,
}

struct Queued {
    name: String,
    work: Work,
}

#[derive(Default)]
struct Inner {
    running: Vec<Running>,
    queued: VecDeque<Queued>,
    /// Latest results and messages, newest first: (failed, text).
    notes: VecDeque<(bool, String)>,
    /// Goes up whenever something finished, so open pages know to reload.
    generation: u64,
}

/// What the job panel shows.
pub struct Snapshot {
    /// Name, files done, files in total, current message.
    pub running: Vec<(String, usize, usize, String)>,
    pub queued: Vec<String>,
    pub notes: Vec<(bool, String)>,
    pub generation: u64,
}

struct Shared {
    cfg: Arc<RwLock<Config>>,
    db_path: PathBuf,
    inner: Mutex<Inner>,
}

#[derive(Clone)]
pub struct Jobs(Arc<Shared>);

impl Jobs {
    pub fn new(cfg: Arc<RwLock<Config>>, db_path: PathBuf) -> Self {
        Jobs(Arc::new(Shared {
            cfg,
            db_path,
            inner: Mutex::new(Inner::default()),
        }))
    }

    /// Run a task that changes the catalogue; it waits while another runs.
    pub fn spawn<F>(&self, name: &str, work: F)
    where
        F: FnOnce(&mut Connection, &Config, &JobState) -> Result<String> + Send + 'static,
    {
        self.submit(name, true, Box::new(work));
    }

    /// Run a task that only reads the catalogue; it starts straight away.
    pub fn spawn_read<F>(&self, name: &str, work: F)
    where
        F: FnOnce(&mut Connection, &Config, &JobState) -> Result<String> + Send + 'static,
    {
        self.submit(name, false, Box::new(work));
    }

    pub fn submit(&self, name: &str, writes: bool, work: Work) {
        let mut inner = self.0.inner.lock().unwrap();
        if inner.running.iter().any(|j| j.name == name)
            || inner.queued.iter().any(|q| q.name == name)
        {
            push_note(&mut inner, false, format!("{name} is already running"));
            return;
        }
        if writes && inner.running.iter().any(|j| j.writes) {
            inner.queued.push_back(Queued {
                name: name.to_string(),
                work,
            });
            return;
        }
        self.start(&mut inner, name, writes, work);
    }

    fn start(&self, inner: &mut Inner, name: &str, writes: bool, work: Work) {
        let state = Arc::new(JobState::default());
        inner.running.push(Running {
            name: name.to_string(),
            state: state.clone(),
            writes,
        });
        let jobs = self.clone();
        let label = name.to_string();
        std::thread::spawn(move || {
            lower_priority();
            let cfg = jobs.0.cfg.read().unwrap().clone();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                db::open(&jobs.0.db_path).and_then(|mut conn| {
                    let mut cfg = cfg;
                    crate::nick::merge(&mut cfg, crate::nick::load(&conn));
                    work(&mut conn, &cfg, &state)
                })
            }))
            .unwrap_or_else(|panic| {
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "unknown error".into());
                Err(anyhow::anyhow!("internal error (please report): {msg}"))
            });
            if let Err(e) = &r {
                log::error!("{label}: {e:#}");
            }
            jobs.finished(&state, &label, r.map_err(|e| format!("{e:#}")));
        });
    }

    fn finished(&self, state: &Arc<JobState>, name: &str, result: Result<String, String>) {
        let mut inner = self.0.inner.lock().unwrap();
        inner.running.retain(|j| !Arc::ptr_eq(&j.state, state));
        match &result {
            Ok(msg) => push_note(&mut inner, false, format!("{name}: {msg}")),
            Err(e) => push_note(&mut inner, true, format!("{name} failed: {e}")),
        }
        state.finish(result);
        if !inner.running.iter().any(|j| j.writes) {
            if let Some(q) = inner.queued.pop_front() {
                self.start(&mut inner, &q.name, true, q.work);
            }
        }
    }

    /// Show a message in the job panel (the result of something quick).
    pub fn note(&self, failed: bool, text: String) {
        push_note(&mut self.0.inner.lock().unwrap(), failed, text);
    }

    /// Ask a running task to stop, or take a waiting one off the queue.
    pub fn cancel(&self, name: &str) {
        let mut inner = self.0.inner.lock().unwrap();
        inner.queued.retain(|q| q.name != name);
        for j in inner.running.iter().filter(|j| j.name == name) {
            j.state.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let inner = self.0.inner.lock().unwrap();
        Snapshot {
            running: inner
                .running
                .iter()
                .map(|j| {
                    let (done, total, msg) = j.state.snapshot();
                    (j.name.clone(), done, total, msg)
                })
                .collect(),
            queued: inner.queued.iter().map(|q| q.name.clone()).collect(),
            notes: inner.notes.iter().cloned().collect(),
            generation: inner.generation,
        }
    }

    pub fn generation(&self) -> u64 {
        self.0.inner.lock().unwrap().generation
    }

    #[cfg(test)]
    pub fn idle(&self) -> bool {
        let inner = self.0.inner.lock().unwrap();
        inner.running.is_empty() && inner.queued.is_empty()
    }
}

/// Let the web pages go first: file work runs at a lower CPU and disk
/// priority. Threads the task starts (copy parts, hashing pools) inherit it.
#[cfg(target_os = "linux")]
fn lower_priority() {
    // SAFETY: plain system calls on the calling thread, no pointers involved.
    unsafe {
        // who = 0 is the calling thread on Linux.
        libc::setpriority(libc::PRIO_PROCESS, 0, 10);
        // I/O class best-effort (2), lowest level (7).
        const IOPRIO_WHO_PROCESS: libc::c_long = 1;
        libc::syscall(
            libc::SYS_ioprio_set,
            IOPRIO_WHO_PROCESS,
            0 as libc::c_long,
            ((2 << 13) | 7) as libc::c_long,
        );
    }
}

#[cfg(not(target_os = "linux"))]
fn lower_priority() {}

fn push_note(inner: &mut Inner, failed: bool, text: String) {
    inner.notes.push_front((failed, text));
    inner.notes.truncate(KEEP_NOTES);
    inner.generation += 1;
}
