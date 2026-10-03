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

    pub fn cfg(&self) -> Config {
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

        let form = format!(
            "folder={}&placement=move&quick=1&on_conflict=skip",
            inbox.display()
        );
        assert_eq!(post(&app, "/load", &form).await, StatusCode::SEE_OTHER);
        wait_idle(&app).await;
        // The load, then the checksums it left to fill in.
        let files = db::all_files(&app.conn().unwrap(), false).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|f| f.hash.is_some()));

        let (status, rows) = get(&app, "/images/rows?q=m31").await;
        assert_eq!(status, StatusCode::OK);
        assert!(rows.contains("2 files"), "{rows}");
        // Paths are shown as the desktop sees them.
        assert!(
            rows.contains("data-path=\"/mnt/nas/Astro/repo/Light/M_31_Andromeda_Galaxy/"),
            "{rows}"
        );

        // Everything the search matches, without ticking rows.
        assert_eq!(
            post(&app, "/images/delete", "all=1&q=m31&kind=all").await,
            StatusCode::SEE_OTHER
        );
        wait_idle(&app).await;
        let left = db::all_files(&app.conn().unwrap(), false).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].object.as_deref(), Some("M 76"));
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
}
