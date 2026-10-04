//! AstroFiler web interface: the AstroFiler catalogue and file management,
//! served to a browser from the machine that holds the files.

mod filter;
mod jobs;
mod pages;
mod ui;

use anyhow::{Context, Result};
use astrofiler::config::Config;
use astrofiler::{db, logging};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use jobs::Jobs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

pub struct AppState {
    pub cfg: Arc<RwLock<Config>>,
    pub db_path: PathBuf,
    pub jobs: Jobs,
    /// Folder the folder picker starts in and can't leave.
    pub root: PathBuf,
    /// The same folder as the desktop sees it (e.g. the mounted share), so
    /// copied paths open there.
    pub desktop_root: Option<String>,
    pub password: Option<String>,
}

pub type App = Arc<AppState>;

impl AppState {
    pub fn new(cfg: Config) -> App {
        let db_path = cfg.database_path();
        let cfg = Arc::new(RwLock::new(cfg));
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        Arc::new(AppState {
            jobs: Jobs::new(cfg.clone(), db_path.clone()),
            cfg,
            db_path,
            root: env("ASTROFILER_WEB_ROOT").map_or_else(|| PathBuf::from("/"), PathBuf::from),
            desktop_root: env("ASTROFILER_WEB_DESKTOP_ROOT"),
            password: env("ASTROFILER_WEB_PASSWORD"),
        })
    }

    /// The settings, with the nicknames from the catalogue among the object
    /// names.
    pub fn cfg(&self) -> Config {
        let mut cfg = self.cfg_saved();
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
        if let Ok(conn) = rusqlite::Connection::open_with_flags(&self.db_path, flags) {
            astrofiler::nick::merge(&mut cfg, astrofiler::nick::load(&conn));
        }
        cfg
    }

    /// The settings as they are in the settings file.
    pub fn cfg_saved(&self) -> Config {
        self.cfg.read().unwrap().clone()
    }

    pub fn conn(&self) -> Result<rusqlite::Connection> {
        db::open(&self.db_path)
    }

    /// A path of this machine as the desktop sees it.
    pub fn desktop_path(&self, path: &str) -> String {
        let root = self.root.to_string_lossy();
        match (
            &self.desktop_root,
            path.strip_prefix(root.trim_end_matches('/')),
        ) {
            (Some(d), Some(rest)) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", d.trim_end_matches('/'))
            }
            _ => path.to_string(),
        }
    }
}

/// Any error becomes a plain "500" page with its text.
pub struct AppError(anyhow::Error);

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", self.0)).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError(e.into())
    }
}

/// Run catalogue or file work off the async threads.
pub async fn blocking<T, F>(f: F) -> Result<T, AppError>
where
    F: FnOnce() -> Result<T> + Send + 'static,
    T: Send + 'static,
{
    Ok(tokio::task::spawn_blocking(f).await??)
}

/// HTTP basic auth with any user name, when a password is set.
async fn auth(State(app): State<App>, req: Request, next: Next) -> Response {
    let Some(password) = &app.password else {
        return next.run(req).await;
    };
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Basic "))
        .and_then(|v| base64::engine::general_purpose::STANDARD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok())
        .and_then(|v| v.split_once(':').map(|(_, p)| p.to_string()));
    if given.as_deref() == Some(password.as_str()) {
        return next.run(req).await;
    }
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"AstroFiler\"")],
        "Password required",
    )
        .into_response()
}

pub fn router(app: App) -> axum::Router {
    pages::routes()
        .layer(middleware::from_fn_with_state(app.clone(), auth))
        .with_state(app)
}

/// The settings, created on first start from `$ASTROFILER_WEB_REPO` and
/// `$ASTROFILER_WEB_INBOX` when there is no config file yet.
fn load_config() -> Result<Config> {
    let mut cfg = Config::load()?;
    if !cfg.path.exists() {
        if let Ok(repo) = std::env::var("ASTROFILER_WEB_REPO") {
            cfg.repo = PathBuf::from(repo);
            if let Ok(inbox) = std::env::var("ASTROFILER_WEB_INBOX") {
                cfg.source = PathBuf::from(inbox);
            }
            cfg.save()?;
        }
    }
    Ok(cfg)
}

async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("signal handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    logging::init(std::env::var("ASTROFILER_WEB_VERBOSE").is_ok(), true);
    // File work reads that many files at once, by default one per core. On a
    // NAS that leaves nothing for the web pages, so cap it (RAYON_NUM_THREADS
    // sets it by hand).
    if std::env::var_os("RAYON_NUM_THREADS").is_none() {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        std::env::set_var(
            "RAYON_NUM_THREADS",
            cores.saturating_sub(1).clamp(1, 4).to_string(),
        );
    }
    let cfg = load_config()?;
    let app = AppState::new(cfg);
    if let Some(dir) = app.db_path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    app.conn()
        .with_context(|| format!("opening the catalogue {}", app.db_path.display()))?;
    let addr = std::env::var("ASTROFILER_WEB_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("listening on {addr}"))?;
    // Printed as well as logged: the log only shows warnings on stderr.
    let started = format!(
        "AstroFiler web {} on http://{addr}, repository {}, catalogue {}",
        env!("CARGO_PKG_VERSION"),
        app.cfg().repo.display(),
        app.db_path.display()
    );
    println!("{started}");
    log::info!("{started}");
    axum::serve(listener, router(app))
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrofiler::fits::{self, Header, ImageShape, OutType, Value};
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn frame(dir: &std::path::Path, name: &str, object: &str, date: &str) {
        let mut h = Header::default();
        h.set("IMAGETYP", Value::Str("Light".into()));
        h.set("OBJECT", Value::Str(object.into()));
        h.set("DATE-OBS", Value::Str(date.into()));
        h.set("EXPTIME", Value::Float(30.0));
        h.set("TELESCOP", Value::Str("RedCat 51".into()));
        h.set("INSTRUME", Value::Str("ASI2600".into()));
        let shape = ImageShape {
            width: 16,
            height: 8,
            planes: 1,
        };
        let data: Vec<f32> = (0..shape.len()).map(|i| (i + name.len()) as f32).collect();
        std::fs::create_dir_all(dir).unwrap();
        fits::write_image(&dir.join(name), &h, shape, &data, OutType::U16).unwrap();
    }

    fn test_app(tmp: &std::path::Path, password: Option<&str>) -> App {
        let cfg = Config {
            repo: tmp.join("astro/repo"),
            source: tmp.join("astro/inbox"),
            database: Some(tmp.join("t.db")),
            path: tmp.join("astrofiler.ini"),
            ..Default::default()
        };
        let db_path = tmp.join("t.db");
        let cfg = Arc::new(RwLock::new(cfg));
        Arc::new(AppState {
            jobs: Jobs::new(cfg.clone(), db_path.clone()),
            cfg,
            db_path,
            root: tmp.join("astro"),
            desktop_root: Some("/mnt/nas/Astro".into()),
            password: password.map(String::from),
        })
    }

    async fn send(app: &App, req: HttpRequest<Body>) -> (StatusCode, String) {
        let res = router(app.clone()).oneshot(req).await.unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    async fn get(app: &App, uri: &str) -> (StatusCode, String) {
        send(app, HttpRequest::get(uri).body(Body::empty()).unwrap()).await
    }

    async fn post(app: &App, uri: &str, form: &str) -> StatusCode {
        let req = HttpRequest::post(uri)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form.to_string()))
            .unwrap();
        send(app, req).await.0
    }

    async fn wait_idle(app: &App) {
        for _ in 0..500 {
            if app.jobs.idle() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("tasks did not finish");
    }

    #[tokio::test]
    async fn load_search_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), None);
        let inbox = tmp.path().join("astro/inbox");
        frame(&inbox, "a.fits", "M 31", "2026-09-01T21:00:00");
        frame(&inbox, "b.fits", "M 31", "2026-09-01T21:01:00");
        frame(&inbox, "c.fits", "M 76", "2026-09-02T21:00:00");

        let form = format!("folder={}&placement=move&on_conflict=skip", inbox.display());
        assert_eq!(post(&app, "/load", &form).await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        let files = db::all_files(&app.conn().unwrap(), false).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|f| f.hash.is_some()));

        // The same frame again: it stays in the folder unless asked to go.
        let again = tmp.path().join("astro/again");
        frame(&again, "a.fits", "M 31", "2026-09-01T21:00:00");
        let form = |extra: &str| {
            format!(
                "folder={}&placement=move&on_conflict=skip{extra}",
                again.display()
            )
        };
        assert_eq!(post(&app, "/load", &form("")).await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        assert!(again.join("a.fits").exists());
        // The folder stays while something is in it.
        assert!(again.is_dir());
        assert_eq!(
            post(&app, "/load", &form("&remove_known=1")).await,
            StatusCode::SEE_OTHER
        );
        wait_idle(&app).await;
        assert!(!again.join("a.fits").exists());
        // Nothing left in it: the folder goes too.
        assert!(!again.exists());
        assert_eq!(db::all_files(&app.conn().unwrap(), false).unwrap().len(), 3);

        let (status, rows) = get(&app, "/images/rows?q=m31").await;
        assert_eq!(status, StatusCode::OK);
        assert!(rows.contains("2 files"), "{rows}");
        // Paths are shown as the desktop sees them.
        assert!(
            rows.contains("data-path=\"/mnt/nas/Astro/repo/Light/M_31_Andromeda_Galaxy/"),
            "{rows}"
        );

        // The advanced SQL condition narrows the search; a bad one is reported.
        let (_, rows) = get(&app, "/images/rows?sql=fitsFileObject%20%3D%20%27M%2076%27").await;
        assert!(rows.contains("1 files"), "{rows}");
        let (_, rows) = get(&app, "/images/rows?sql=nonsense%20%3D%20").await;
        assert!(rows.contains("SQL:"), "{rows}");
        let (_, rows) = get(&app, "/images/rows?sql=1%3D1%3B%20DROP%20TABLE%20fitsFile").await;
        assert!(rows.contains("SQL:"), "{rows}");
        assert_eq!(db::all_files(&app.conn().unwrap(), false).unwrap().len(), 3);
        // Grouped: one row per object, its files fetched when it is opened.
        // What narrow screens need: columns they can leave out, help without hover.
        let (_, plain) = get(&app, "/images/rows").await;
        assert!(plain.contains("class=\"c3\"") && plain.contains("class=\"c2 file link\""));
        let (_, images) = get(&app, "/images").await;
        assert!(images.contains("id=\"sqlhelp\""));
        let (_, rows) = get(&app, "/images/rows?group=object").await;
        assert!(rows.contains("3 files in 2 groups"), "{rows}");
        assert!(rows.contains("Total (2 groups)"), "{rows}");
        assert!(!rows.contains("name=\"id\""), "{rows}");
        let (_, rows) = get(&app, "/images/rows?group=object&key=M%2031").await;
        assert_eq!(rows.matches("name=\"id\"").count(), 2, "{rows}");
        let (_, rows) = get(&app, "/images/rows?group=date&key=2026-09-02").await;
        assert_eq!(rows.matches("name=\"id\"").count(), 1, "{rows}");

        // Removing from the catalogue leaves the file on disk.
        let m76 = db::all_files(&app.conn().unwrap(), false)
            .unwrap()
            .into_iter()
            .find(|f| f.object.as_deref() == Some("M 76"))
            .unwrap();
        let body = format!("id={}", m76.id);
        assert_eq!(
            post(&app, "/images/remove", &body).await,
            StatusCode::SEE_OTHER
        );
        wait_idle(&app).await;
        assert_eq!(db::all_files(&app.conn().unwrap(), false).unwrap().len(), 2);
        assert!(std::path::Path::new(&m76.name).exists());

        // Everything the search matches, without ticking rows.
        // Not without the typed confirmation.
        post(&app, "/images/delete", "all=1&q=m31&kind=all").await;
        wait_idle(&app).await;
        assert_eq!(db::all_files(&app.conn().unwrap(), false).unwrap().len(), 2);
        assert_eq!(
            post(
                &app,
                "/images/delete",
                "grp=M%2031&group=object&kind=all&confirm=DELETE"
            )
            .await,
            StatusCode::SEE_OTHER
        );
        wait_idle(&app).await;
        let left = db::all_files(&app.conn().unwrap(), false).unwrap();
        assert!(left.is_empty());
        // Only the file that was removed from the catalogue is still there.
        let on_disk = astrofiler::ingest::collect_files(&tmp.path().join("astro/repo"), &[]);
        assert_eq!(on_disk.len(), 1, "{on_disk:?}");
    }

    #[tokio::test]
    async fn folder_picker_stays_inside_its_root() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), None);
        std::fs::create_dir_all(tmp.path().join("astro/inbox/night 1")).unwrap();
        let root = tmp.path().join("astro").canonicalize().unwrap();
        let (_, body) = get(&app, "/browse?target=x&path=/etc").await;
        assert!(
            body.contains(&format!("<strong>{}</strong>", root.display())),
            "{body}"
        );
        assert!(!body.contains("Up"), "{body}");
        let (_, body) = get(
            &app,
            &format!("/browse?target=x&path={}/inbox", root.display()),
        )
        .await;
        assert!(body.contains("night%201"), "{body}");
    }

    #[tokio::test]
    async fn password_is_required_when_set() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), Some("secret"));
        assert_eq!(get(&app, "/images").await.0, StatusCode::UNAUTHORIZED);
        let auth = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("me:secret")
        );
        let req = HttpRequest::get("/images")
            .header(header::AUTHORIZATION, auth)
            .body(Body::empty())
            .unwrap();
        assert_eq!(send(&app, req).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn merge_into_an_object_that_has_the_same_frames() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), None);
        let inbox = tmp.path().join("astro/inbox");
        frame(&inbox.join("0"), "a.fits", "NGC 281", "2026-09-01T21:00:00");
        frame(&inbox.join("1"), "a.fits", "NGC281", "2026-09-01T21:00:00");
        let form = format!("folder={}&placement=move&on_conflict=skip", inbox.display());
        post(&app, "/load", &form).await;
        wait_idle(&app).await;

        post(
            &app,
            "/batch/merge",
            "from=NGC281&to=NGC%20281&headers=1&refile=1",
        )
        .await;
        wait_idle(&app).await;
        // Both frames are kept; the summary says where the moved one went.
        let files = db::all_files(&app.conn().unwrap(), false).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| std::path::Path::new(&f.name).exists()));
        let (_, jobs) = get(&app, "/jobs").await;
        assert!(jobs.contains("NGC_281_Pacman_Nebula"), "{jobs}");
        assert!(jobs.contains("1 are copies"), "{jobs}");
    }

    #[tokio::test]
    async fn move_files_catalogued_in_place_into_the_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), None);
        // A folder inside the repository, catalogued where it is by a sync.
        let old = tmp.path().join("astro/repo/C 7 Spiral Galaxy/C 7_sub");
        frame(&old, "a.fits", "C 7", "2026-01-20T01:00:00");
        frame(&old, "b.fits", "C 7", "2026-01-20T01:01:00");
        frame(&old, "c.fits", "C 7", "2026-01-20T01:02:00");
        assert_eq!(post(&app, "/sync", "").await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        assert_eq!(db::all_files(&app.conn().unwrap(), false).unwrap().len(), 3);
        // A processed picture, and what the telescope leaves behind.
        let folder = tmp.path().join("astro/repo/C 7 Spiral Galaxy");
        std::fs::write(folder.join("C 7.png"), b"picture").unwrap();
        std::fs::write(folder.join("Stacked_778_C 7_thn.jpg"), b"thumb").unwrap();
        std::fs::write(old.join("Light_C 7_20.0s.jpg"), b"sub preview").unwrap();
        for n in ["a", "b", "c"] {
            std::fs::write(old.join(format!("{n}.jpg")), b"sub preview").unwrap();
        }

        let form = format!(
            "folder={}&placement=move&on_conflict=skip",
            tmp.path().join("astro/repo/C 7 Spiral Galaxy").display()
        );
        assert_eq!(post(&app, "/load", &form).await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        let files = db::all_files(&app.conn().unwrap(), false).unwrap();
        assert_eq!(files.len(), 3);
        for f in &files {
            // The folder's nickname is kept in the object's folder name.
            assert!(
                f.name.contains("/repo/Light/C_7_Spiral_Galaxy/"),
                "{}",
                f.name
            );
            assert!(std::path::Path::new(&f.name).exists(), "{}", f.name);
        }
        assert!(!old.join("a.fits").exists());
        // The picture went to the object's folder; the telescope's stayed.
        let object_dir = tmp.path().join("astro/repo/Light").join("C_7");
        let moved = std::fs::read_dir(tmp.path().join("astro/repo/Light"))
            .unwrap()
            .flatten()
            .any(|d| d.path().join("C 7.png").exists());
        assert!(moved, "{}", object_dir.display());
        assert!(!folder.join("C 7.png").exists());
        assert!(folder.join("Stacked_778_C 7_thn.jpg").exists());
        assert!(old.join("Light_C 7_20.0s.jpg").exists());
        assert!(old.join("a.jpg").exists());
        let names = app.cfg().object_names;
        assert_eq!(names.get("C 7").map(String::as_str), Some("Spiral Galaxy"));
    }

    #[tokio::test]
    async fn statistics_show_the_last_numbers_while_recalculating() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), None);
        let inbox = tmp.path().join("inbox");
        frame(&inbox, "a.fits", "M 31", "2026-01-20T01:00:00");
        let form = format!("folder={}&placement=move&on_conflict=skip", inbox.display());
        assert_eq!(post(&app, "/load", &form).await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        let (_, first) = get(&app, "/stats").await;
        assert!(first.contains("Calculating") && !first.contains("Integration by object"));
        let (_, fresh) = get(&app, "/stats/fresh").await;
        assert!(
            fresh.contains("M 31") && !fresh.contains("hx-get"),
            "{fresh}"
        );
        let (_, again) = get(&app, "/stats").await;
        assert!(
            again.contains("Recalculating") && again.contains("M 31"),
            "{again}"
        );
    }

    #[tokio::test]
    async fn mosaic_panels_are_one_group_with_a_folder_each() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), None);
        let inbox = tmp.path().join("inbox");
        frame(&inbox, "a.fits", "HD 199479(1)", "2026-09-09T22:00:00");
        frame(&inbox, "b.fits", "HD 199479(2)", "2026-09-09T23:00:00");
        let form = format!("folder={}&placement=move&on_conflict=skip", inbox.display());
        assert_eq!(post(&app, "/load", &form).await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        let files = db::all_files(&app.conn().unwrap(), false).unwrap();
        for (f, n) in files.iter().zip([1, 2]) {
            let dir = format!("/Light/HD_199479/RedCat_51/ASI2600/Panel_{n}/20260909/");
            assert!(f.name.contains(&dir), "{}", f.name);
        }
        let (_, rows) = get(&app, "/images/rows?group=object").await;
        assert!(rows.contains("2 files in 1 groups"), "{rows}");
        assert!(rows.contains("mosaic, 2 panels"), "{rows}");
        // The group opens to both panels; searching one panel finds just it.
        let (_, rows) = get(&app, "/images/rows?group=object&key=HD%20199479").await;
        assert!(rows.contains("HD 199479(1)") && rows.contains("HD 199479(2)"));
        let (_, rows) = get(&app, "/images/rows?q=HD%20199479(2)").await;
        assert!(rows.contains("1 files"), "{rows}");
        let (_, stats) = get(&app, "/stats/fresh").await;
        assert!(stats.contains("HD 199479 (2)"), "{stats}");
    }

    #[tokio::test]
    async fn nickname_from_a_picture_renames_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let app = test_app(tmp.path(), None);
        let inbox = tmp.path().join("inbox");
        frame(&inbox, "a.fits", "C 36", "2026-01-20T01:00:00");
        let form = format!("folder={}&placement=move&on_conflict=skip", inbox.display());
        assert_eq!(post(&app, "/load", &form).await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        let dir = tmp.path().join("astro/repo/Light/C_36");
        assert!(dir.is_dir());
        std::fs::write(dir.join("C 36 Koi Fish Galaxy.png"), b"picture").unwrap();

        let status = post(&app, "/batch/run", "action=layout_migrate").await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        let new = tmp.path().join("astro/repo/Light/C_36_Koi_Fish_Galaxy");
        let files = db::all_files(&app.conn().unwrap(), false).unwrap();
        assert!(
            files[0].name.starts_with(&*new.to_string_lossy()),
            "{}",
            files[0].name
        );
        assert!(new.join("C 36 Koi Fish Galaxy.png").exists());
        assert!(!dir.exists());

        // A nickname removed on the Settings page is not used any more.
        let form = format!(
            "repo={}&on_conflict=skip&nicknames=",
            tmp.path().join("astro/repo").display()
        );
        assert_eq!(post(&app, "/settings", &form).await, StatusCode::SEE_OTHER);
        assert!(app.cfg().object_names.is_empty());
    }
}
