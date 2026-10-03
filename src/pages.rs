//! The pages and what their buttons do.

use crate::filter::{self, Filter};
use crate::ui::{folder_input, jobs_panel, opt, page};
use crate::{blocking, App, AppError};
use astrofiler::batch::{self, EditOptions, EditReport, ExportLayout};
use astrofiler::db::{self, FitsFile};
use astrofiler::ingest::{self, IngestOptions, OnConflict, Placement};
use astrofiler::{logging, names, sessions, stats, util};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use axum_extra::extract::{Form, Query};
use maud::{html, Markup};
use serde::Deserialize;
use std::path::{Path, PathBuf};

type Page = Result<Markup, AppError>;

const PAGE_ROWS: usize = 200;
const LOG_LINES: usize = 500;

pub fn routes() -> Router<App> {
    Router::new()
        .route("/", get(|| async { Redirect::to("/images") }))
        .route("/images", get(images))
        .route("/images/rows", get(image_rows))
        .route("/images/edit", post(images_edit))
        .route("/images/export", post(images_export))
        .route("/images/remove", post(images_remove))
        .route("/images/delete", post(images_delete))
        .route("/load", get(load).post(load_start))
        .route("/sync", post(sync))
        .route("/sessions", get(sessions_page))
        .route("/sessions/files", get(session_files))
        .route("/sessions/create", post(sessions_create))
        .route("/sessions/clear", post(sessions_clear))
        .route("/sessions/export", post(sessions_export))
        .route("/batch", get(batch_page))
        .route("/batch/merge", post(batch_merge))
        .route("/batch/run", post(batch_run))
        .route("/batch/clean", post(batch_clean))
        .route("/duplicates", get(duplicates))
        .route("/duplicates/remove", post(duplicates_remove))
        .route("/mappings", get(mappings))
        .route("/mappings/add", post(mappings_add))
        .route("/mappings/remove", post(mappings_remove))
        .route("/stats", get(stats_page))
        .route("/settings", get(settings).post(settings_save))
        .route("/log", get(log_page))
        .route("/log/lines", get(log_lines))
        .route("/log/clear", post(log_clear))
        .route("/jobs", get(jobs))
        .route("/jobs/cancel", post(jobs_cancel))
        .route("/browse", get(browse))
        .route(
            "/assets/htmx.js",
            get(|| asset(include_str!("../assets/htmx.min.js"), "text/javascript")),
        )
        .route(
            "/assets/app.js",
            get(|| asset(include_str!("../assets/app.js"), "text/javascript")),
        )
        .route(
            "/assets/style.css",
            get(|| asset(include_str!("../assets/style.css"), "text/css")),
        )
}

async fn asset(body: &'static str, kind: &'static str) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, kind)], body)
}

/// A ticked checkbox sends its value; an unticked one sends nothing.
fn ticked(v: &Option<String>) -> bool {
    v.is_some()
}

// ------------------------------------------------------------------ Images

async fn images(State(app): State<App>) -> Markup {
    let fields: Vec<&str> = batch::EDITABLE.iter().map(|e| e.0).collect();
    let body = html! {
        form id="sel" method="post" {
            div class="toolbar" {
                input type="search" id="q" name="q" size="42" oninput="el('page').value=0"
                    placeholder="Search: M 76, M 76 LP, Barbell, 2026-09-27…";
                select id="kind" name="kind" onchange="el('page').value=0" {
                    option value="all" { "All" }
                    option value="light" { "Lights" }
                    option value="calibration" { "Calibration" }
                    option value="stacked" { "Stacked" }
                }
                input type="hidden" id="sort" name="sort" value="object";
                input type="hidden" id="desc" name="desc" value="";
                input type="hidden" id="page" name="page" value="0";
            }
            div class="toolbar" {
                span id="selcount" { "0 selected" }
                label { input type="checkbox" id="all" name="all" value="1";
                    " All matching files, not just this page" }
                button type="button" onclick="if(needSelection())dlg('edit')" { "Edit field…" }
                button type="button" onclick="if(needSelection())dlg('export')" { "Export…" }
                button formaction="/images/remove"
                    onclick="return needSelection()&&confirm('Remove the selected files from the catalogue? The files stay on disk.')"
                    { "Remove from catalogue" }
                button class="danger" formaction="/images/delete"
                    onclick="return needSelection()&&confirm('Permanently delete the selected files from disk?')"
                    { "Delete files" }
            }
            div id="rows" hx-get="/images/rows" hx-include="#q,#kind,#sort,#desc,#page"
                hx-trigger="load, input changed delay:300ms from:#q, change from:#kind, reload, refresh from:body" {}
            dialog id="edit" {
                h2 { "Edit the selected files" }
                p {
                    select name="field" { @for f in &fields { option { (f) } } }
                    " = "
                    input type="text" name="value" size="30";
                }
                label class="line" { input type="checkbox" name="headers" value="1" checked;
                    " Also rewrite it in the FITS headers" }
                label class="line" { input type="checkbox" name="refile" value="1" checked;
                    " Also rename and re-file the files" }
                p {
                    button formaction="/images/edit" { "Apply" } " "
                    button type="button" onclick="el('edit').close()" { "Cancel" }
                }
            }
            dialog id="export" {
                h2 { "Export the selected files" }
                p { "Copies go to: " (folder_input("export_dest", "dest", "")) }
                label class="line" { input type="checkbox" name="by_object" value="1" checked;
                    " Into <object>/<filter> folders" }
                p {
                    button formaction="/images/export" { "Export" } " "
                    button type="button" onclick="el('export').close()" { "Cancel" }
                }
            }
        }
    };
    page(&app, "/images", false, body)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RowsQuery {
    q: String,
    kind: String,
    sort: String,
    desc: String,
    page: usize,
}

async fn image_rows(State(app): State<App>, Query(query): Query<RowsQuery>) -> Page {
    let a = app.clone();
    let (files, shown, page) = blocking(move || {
        let files = db::all_files(&a.conn()?, false)?;
        let f = Filter {
            q: query.q,
            kind: query.kind,
            sort: query.sort,
            desc: !query.desc.is_empty(),
        };
        let idx = filter::apply(&files, &a.cfg().object_names, &f);
        let pages = idx.len().div_ceil(PAGE_ROWS).max(1);
        let page = query.page.min(pages - 1);
        let rows: Vec<FitsFile> = idx
            .iter()
            .skip(page * PAGE_ROWS)
            .take(PAGE_ROWS)
            .map(|&i| files[i].clone())
            .collect();
        Ok((rows, idx.len(), page))
    })
    .await?;
    let pages = shown.div_ceil(PAGE_ROWS).max(1);
    let head = |label: &str, key: &str| {
        html! {
            th class="sort" onclick={"setSort('" (key) "')"} { (label) }
        }
    };
    Ok(html! {
        div class="toolbar" {
            span { (shown) " files" }
            @if pages > 1 {
                button type="button" disabled[page == 0] onclick={"reload(" (page.saturating_sub(1)) ")"} { "‹ Previous" }
                span { "page " (page + 1) " of " (pages) }
                button type="button" disabled[page + 1 >= pages] onclick={"reload(" (page + 1) ")"} { "Next ›" }
            }
        }
        div class="scroll" {
            table {
                thead { tr {
                    th { input type="checkbox" onclick="toggleAll(this)" title="Select this page"; }
                    (head("Object", "object")) (head("Type", "type")) (head("Date", "date"))
                    (head("Filter", "filter")) (head("Exp (s)", "exposure"))
                    th { "Bin" } th { "Temp" }
                    (head("Telescope / camera", "telescope"))
                    th { "File" }
                } }
                tbody {
                    @for f in &files {
                        tr {
                            td { input type="checkbox" class="row" name="id" value=(f.id); }
                            td class="link" title="Show only this object" data-q=(opt(&f.object))
                                onclick="setSearch(this.dataset.q)" { (opt(&f.object)) }
                            td { @if f.stacked { "STACKED" } @else { (opt(&f.image_type)) } }
                            td { (opt(&f.date).replace('T', " ").chars().take(19).collect::<String>()) }
                            td { (opt(&f.filter)) }
                            td { (opt(&f.exptime)) }
                            td { (opt(&f.xbin)) "x" (opt(&f.ybin)) }
                            td { (opt(&f.ccd_temp)) }
                            td { (opt(&f.telescope)) " / " (opt(&f.instrument)) }
                            td class="file link" title={ (f.name) " (click to copy the path)" }
                                data-path=(app.desktop_path(&f.name))
                                onclick="copyText(this.dataset.path)" { (f.file_name()) }
                        }
                    }
                }
            }
        }
    })
}

/// The Images form: the ticked rows, or everything the search matches.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Selection {
    id: Vec<String>,
    all: Option<String>,
    q: String,
    kind: String,
    field: String,
    value: String,
    headers: Option<String>,
    refile: Option<String>,
    dest: String,
    by_object: Option<String>,
}

impl Selection {
    fn ids(&self, app: &App) -> anyhow::Result<Vec<String>> {
        if !ticked(&self.all) {
            return Ok(self.id.clone());
        }
        let files = db::all_files(&app.conn()?, false)?;
        let f = Filter {
            q: self.q.clone(),
            kind: self.kind.clone(),
            ..Default::default()
        };
        Ok(filter::apply(&files, &app.cfg().object_names, &f)
            .into_iter()
            .map(|i| files[i].id.clone())
            .collect())
    }
}

/// Status line for a bulk edit; the files that failed go to the log.
fn edit_summary(r: &EditReport) -> String {
    for (f, e) in &r.errors {
        log::warn!("Not changed: {f} ({e})");
    }
    format!(
        "{} updated, {} moved{}",
        r.updated,
        r.moved,
        if r.errors.is_empty() {
            String::new()
        } else {
            format!(", {} errors (see Log)", r.errors.len())
        }
    )
}

async fn images_edit(
    State(app): State<App>,
    Form(sel): Form<Selection>,
) -> Result<Redirect, AppError> {
    let a = app.clone();
    blocking(move || {
        let ids = sel.ids(&a)?;
        let opts = EditOptions {
            update_headers: ticked(&sel.headers),
            refile: ticked(&sel.refile),
        };
        let (field, value) = (sel.field, sel.value.trim().to_string());
        a.jobs.spawn("Edit files", move |conn, cfg, p| {
            let r = batch::set_field(conn, cfg, &ids, &field, &value, opts, p)?;
            Ok(edit_summary(&r))
        });
        Ok(())
    })
    .await?;
    Ok(Redirect::to("/images"))
}

async fn images_export(
    State(app): State<App>,
    Form(sel): Form<Selection>,
) -> Result<Redirect, AppError> {
    let a = app.clone();
    blocking(move || {
        if sel.dest.trim().is_empty() {
            a.jobs.note(true, "Export: no folder given".into());
            return Ok(());
        }
        let ids = sel.ids(&a)?;
        let dest = PathBuf::from(sel.dest.trim());
        let layout = if ticked(&sel.by_object) {
            ExportLayout::ByObject
        } else {
            ExportLayout::Flat
        };
        a.jobs.spawn_read("Export", move |conn, _, p| {
            Ok(format!(
                "{} files exported",
                batch::export_files(conn, &ids, &dest, layout, false, p)?
            ))
        });
        Ok(())
    })
    .await?;
    Ok(Redirect::to("/images"))
}

async fn delete(app: App, sel: Selection, from_disk: bool) -> Result<Redirect, AppError> {
    let a = app.clone();
    blocking(move || {
        let ids = sel.ids(&a)?;
        a.jobs.spawn("Delete", move |conn, _, _| {
            let (n, errs) = batch::delete_files(conn, &ids, from_disk)?;
            for (f, e) in &errs {
                log::warn!("Not deleted: {f} ({e})");
            }
            Ok(format!("{n} removed, {} errors", errs.len()))
        });
        Ok(())
    })
    .await?;
    Ok(Redirect::to("/images"))
}

async fn images_remove(
    State(app): State<App>,
    Form(sel): Form<Selection>,
) -> Result<Redirect, AppError> {
    delete(app, sel, false).await
}

async fn images_delete(
    State(app): State<App>,
    Form(sel): Form<Selection>,
) -> Result<Redirect, AppError> {
    delete(app, sel, true).await
}

// -------------------------------------------------------------------- Load

async fn load(State(app): State<App>) -> Markup {
    let cfg = app.cfg();
    let body = html! {
        h2 { "Load images" }
        form method="post" action="/load" {
            p { "Folder with FITS / XISF files: "
                (folder_input("load_folder", "folder", &cfg.source.to_string_lossy())) }
            label class="line" { input type="radio" name="placement" value="move" checked;
                " Move into the repository (rename and organise)" }
            label class="line" style="margin-left: 24px" { input type="checkbox" name="quick" value="1" checked;
                " Quick move: files that are only renamed get their checksums filled in afterwards" }
            label class="line" { input type="radio" name="placement" value="copy";
                " Copy into the repository, keep the originals untouched" }
            label class="line" { input type="radio" name="placement" value="in_place";
                " Catalogue in place (no renaming or moving)" }
            p { "If a different file already has the same name in the repository:" }
            p {
                @for c in OnConflict::ALL {
                    label { input type="radio" name="on_conflict" value=(c.key())
                        checked[c == cfg.on_conflict]; " " (c.label()) " " }
                }
            }
            p class="muted" { "Identical files are never copied twice, and empty or half-copied leftovers are always replaced." }
            label class="line" { input type="checkbox" name="dry_run" value="1";
                " Dry run: only show what would happen (the plan is written to the log)" }
            p { button { "Start" } }
        }
        h2 { "Sync repository" }
        p { "Catalogue files already in the repository (" (cfg.repo.display()) ") without moving them." }
        form method="post" action="/sync" { button { "Sync repository" } }
    };
    page(&app, "/load", false, body)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LoadForm {
    folder: String,
    placement: String,
    quick: Option<String>,
    on_conflict: String,
    dry_run: Option<String>,
}

async fn load_start(State(app): State<App>, Form(f): Form<LoadForm>) -> Redirect {
    let src = PathBuf::from(f.folder.trim());
    if f.folder.trim().is_empty() || !src.is_dir() {
        app.jobs
            .note(true, format!("Load: {} is not a folder", src.display()));
        return Redirect::to("/load");
    }
    let placement = match f.placement.as_str() {
        "copy" => Placement::Copy,
        "in_place" => Placement::InPlace,
        _ => Placement::Move,
    };
    let opts = IngestOptions {
        placement,
        dry_run: ticked(&f.dry_run),
        on_conflict: OnConflict::parse(&f.on_conflict).unwrap_or(app.cfg().on_conflict),
        quick: ticked(&f.quick) && placement == Placement::Move,
    };
    app.jobs.submit(
        if opts.dry_run { "Dry run" } else { "Load" },
        !opts.dry_run,
        Box::new(move |conn, cfg, p| {
            let r = ingest::ingest_folder(conn, cfg, &src, opts, p)?;
            if opts.dry_run {
                for (a, b) in &r.placed {
                    log::info!("plan: {} -> {}", a.display(), b.display());
                }
            }
            for (a, e) in &r.errors {
                log::warn!("{}: {e}", a.display());
            }
            for (a, b) in &r.conflicts {
                log::warn!(
                    "{}: skipped, a different file already exists at {}",
                    a.display(),
                    b.display()
                );
            }
            Ok(r.summary())
        }),
    );
    if opts.quick && !opts.dry_run {
        // Queued behind the load; does nothing if every file had to be read anyway.
        app.jobs.spawn("Fill checksums", |conn, _, p| {
            Ok(batch::fill_checksums(conn, p)?.summary())
        });
    }
    Redirect::to("/images")
}

async fn sync(State(app): State<App>) -> Redirect {
    app.jobs.spawn("Sync repository", |conn, cfg, p| {
        Ok(ingest::ingest_folder(conn, cfg, &cfg.repo, IngestOptions::IN_PLACE, p)?.summary())
    });
    Redirect::to("/images")
}

// ---------------------------------------------------------------- Sessions

async fn sessions_page(State(app): State<App>) -> Page {
    let a = app.clone();
    let list = blocking(move || db::all_sessions(&a.conn()?)).await?;
    let body = html! {
        div class="toolbar" {
            form method="post" action="/sessions/create" {
                button title="Group unassigned files by object, night and filter" { "Create sessions" }
            }
            form method="post" action="/sessions/clear" {
                button onclick="return confirm('Remove all sessions? Files are kept; you can re-create sessions any time.')"
                    { "Clear all sessions" }
            }
            span class="muted" { (list.len()) " sessions" }
        }
        div class="columns" {
            div class="scroll" {
                table {
                    thead { tr {
                        th { "Object" } th { "Date" } th { "Filter" } th { "Exp" } th { "Files" }
                        th { "Temp" } th { "Telescope / camera" }
                    } }
                    tbody {
                        @for s in &list {
                            tr class="pick" hx-get="/sessions/files" hx-vals={"{\"id\":\"" (s.id) "\"}"}
                                hx-target="#session-files" {
                                td { @if s.is_calibration() { em { (opt(&s.object)) } } @else { (opt(&s.object)) } }
                                td { (opt(&s.date)) }
                                td { (opt(&s.filter)) }
                                td { (opt(&s.exposure)) }
                                td { (s.file_count) }
                                td { (opt(&s.ccd_temp)) }
                                td { (opt(&s.telescope)) " / " (opt(&s.imager)) }
                            }
                        }
                    }
                }
            }
            div id="session-files" { p class="muted" { "Click a session to see its files." } }
        }
    };
    Ok(page(&app, "/sessions", true, body))
}

#[derive(Deserialize)]
struct SessionId {
    id: String,
}

async fn session_files(State(app): State<App>, Query(q): Query<SessionId>) -> Page {
    let a = app.clone();
    let id = q.id.clone();
    let files = blocking(move || sessions::session_files(&a.conn()?, &id)).await?;
    Ok(html! {
        form method="post" action="/sessions/export" class="toolbar" {
            input type="hidden" name="id" value=(q.id);
            span { (files.len()) " files. Export to: " }
            (folder_input("session_dest", "dest", ""))
            button { "Export session" }
        }
        div class="scroll" {
            table {
                thead { tr { th { "Date" } th { "File" } } }
                tbody {
                    @for f in &files {
                        tr { td { (opt(&f.date)) } td title=(f.name) { (f.file_name()) } }
                    }
                }
            }
        }
    })
}

async fn sessions_create(State(app): State<App>) -> Redirect {
    app.jobs.spawn("Create sessions", |conn, _, p| {
        let r = sessions::create_all(conn, p)?;
        Ok(format!(
            "{} sessions created ({} light, {} calibration)",
            r.total(),
            r.light,
            r.total() - r.light
        ))
    });
    Redirect::to("/sessions")
}

async fn sessions_clear(State(app): State<App>) -> Result<Redirect, AppError> {
    let a = app.clone();
    let n = blocking(move || sessions::clear_all(&mut a.conn()?)).await?;
    app.jobs.note(false, format!("{n} sessions removed"));
    Ok(Redirect::to("/sessions"))
}

#[derive(Deserialize)]
struct SessionExport {
    id: String,
    dest: String,
}

async fn sessions_export(State(app): State<App>, Form(f): Form<SessionExport>) -> Redirect {
    if f.dest.trim().is_empty() {
        app.jobs
            .note(true, "Export session: no folder given".into());
        return Redirect::to("/sessions");
    }
    app.jobs.spawn_read("Export session", move |conn, _, p| {
        let ids: Vec<String> = sessions::session_files(conn, &f.id)?
            .into_iter()
            .map(|f| f.id)
            .collect();
        Ok(format!(
            "{} files exported",
            batch::export_files(
                conn,
                &ids,
                Path::new(f.dest.trim()),
                ExportLayout::ByObject,
                false,
                p
            )?
        ))
    });
    Redirect::to("/sessions")
}

// ------------------------------------------------------------------- Batch

async fn batch_page(State(app): State<App>) -> Page {
    let a = app.clone();
    let objects = blocking(move || {
        let mut v: Vec<String> = db::all_files(&a.conn()?, false)?
            .into_iter()
            .filter_map(|f| f.object)
            .collect();
        v.sort();
        v.dedup();
        Ok(v)
    })
    .await?;
    let run = |action: &str, label: &str, hint: &str, confirm: Option<&str>| {
        html! {
            button name="action" value=(action) title=(hint)
                onclick=[confirm.map(|c| format!("return confirm('{c}')"))] { (label) }
        }
    };
    let body = html! {
        h2 { "Merge objects" }
        p { "Rename an object everywhere, e.g. \"Andromeda\" to \"M 31\"." }
        form method="post" action="/batch/merge" {
            p {
                "From " select name="from" { @for o in &objects { option { (o) } } }
                " to " input type="text" name="to" size="30" required;
            }
            label class="line" { input type="checkbox" name="headers" value="1" checked;
                " Also rewrite OBJECT in the FITS headers" }
            label class="line" { input type="checkbox" name="refile" value="1" checked;
                " Also rename and re-file the files" }
            p { button { "Merge" } }
        }
        h2 { "Repository maintenance" }
        form method="post" action="/batch/run" class="toolbar" {
            (run("verify", "Verify files", "Check every catalogued file still exists", None))
            (run("verify_hash", "Verify checksums", "Re-hash every file (slower)", None))
            (run("checksums", "Fill in checksums", "Hash the files that were filed without a checksum", None))
            (run("forget", "Forget missing files", "Remove catalogue entries whose file is gone", None))
            (run("empty_dirs", "Remove empty folders", "Under the repository", None))
            (run("regenerate", "Regenerate catalogue", "Rebuild the catalogue by rescanning the repository",
                Some("Rebuild the whole catalogue from the files in the repository? Sessions are removed.")))
        }
        h2 { "Folder layout" }
        p { "Files filed by an older version may sit in folders without the object's name, in Seestar serial-number folders, or under a renamed stacked result." }
        form method="post" action="/batch/run" class="toolbar" {
            (run("layout_check", "Check folders", "Count the files under an older layout", None))
            (run("layout_migrate", "Move them to the current layout", "", None))
        }
        h2 { "Clean up telescope previews" }
        p { "Delete JPG/PNG previews and empty Thumbnail folders under a folder (e.g. an old backup). FITS and JSON files are kept." }
        form method="post" action="/batch/clean" class="toolbar" {
            (folder_input("clean_dir", "dir", ""))
            button name="mode" value="preview" { "Preview" }
            button class="danger" name="mode" value="delete"
                onclick="return confirm('Delete the preview images under this folder?')" { "Delete previews" }
        }
    };
    Ok(page(&app, "/batch", false, body))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct MergeForm {
    from: String,
    to: String,
    headers: Option<String>,
    refile: Option<String>,
}

async fn batch_merge(State(app): State<App>, Form(f): Form<MergeForm>) -> Redirect {
    let (from, to) = (f.from, f.to.trim().to_string());
    if from.is_empty() || to.is_empty() {
        app.jobs
            .note(true, "Merge objects: both names are needed".into());
        return Redirect::to("/batch");
    }
    let opts = EditOptions {
        update_headers: ticked(&f.headers),
        refile: ticked(&f.refile),
    };
    app.jobs.spawn("Merge objects", move |conn, cfg, p| {
        let r = batch::merge_objects(conn, cfg, &from, &to, opts, p)?;
        Ok(edit_summary(&r))
    });
    Redirect::to("/batch")
}

#[derive(Deserialize)]
struct RunForm {
    action: String,
}

async fn batch_run(State(app): State<App>, Form(f): Form<RunForm>) -> Redirect {
    let jobs = &app.jobs;
    match f.action.as_str() {
        "verify" => jobs.spawn_read("Verify", |conn, _, p| {
            let r = batch::verify(conn, false, p)?;
            for f in &r.missing {
                log::warn!("missing: {}", f.name);
            }
            Ok(format!(
                "{} checked, {} missing",
                r.checked,
                r.missing.len()
            ))
        }),
        "verify_hash" => jobs.spawn_read("Verify checksums", |conn, _, p| {
            let r = batch::verify(conn, true, p)?;
            for f in &r.missing {
                log::warn!("missing: {}", f.name);
            }
            for f in &r.mismatched {
                log::warn!("changed: {}", f.name);
            }
            Ok(format!(
                "{} checked, {} missing, {} changed",
                r.checked,
                r.missing.len(),
                r.mismatched.len()
            ))
        }),
        "checksums" => jobs.spawn("Fill checksums", |conn, _, p| {
            Ok(batch::fill_checksums(conn, p)?.summary())
        }),
        "forget" => jobs.spawn("Forget missing", |conn, _, p| {
            Ok(format!(
                "{} entries removed",
                batch::remove_missing(conn, p)?
            ))
        }),
        "empty_dirs" => jobs.spawn_read("Remove empty folders", |_, cfg, _| {
            Ok(format!(
                "{} empty folders removed",
                batch::remove_empty_dirs(&cfg.repo)
            ))
        }),
        "regenerate" => jobs.spawn("Regenerate", |conn, cfg, p| {
            Ok(batch::regenerate(conn, cfg, p)?.summary())
        }),
        "layout_check" => jobs.spawn_read("Check folders", |conn, cfg, _| {
            Ok(format!(
                "{} files are filed under an older folder layout",
                batch::layout_plan(conn, cfg)?.len()
            ))
        }),
        "layout_migrate" => jobs.spawn("Update folders", |conn, cfg, p| {
            let r = batch::migrate_layout(conn, cfg, false, p)?;
            for (f, e) in &r.errors {
                log::warn!("Not moved: {} ({e})", f.display());
            }
            Ok(format!(
                "{} files moved to the current layout{}",
                r.moved.len(),
                if r.errors.is_empty() {
                    String::new()
                } else {
                    format!(", {} not moved (see Log)", r.errors.len())
                }
            ))
        }),
        other => jobs.note(true, format!("Unknown action {other}")),
    }
    Redirect::to("/batch")
}

#[derive(Deserialize)]
struct CleanForm {
    dir: String,
    mode: String,
}

async fn batch_clean(State(app): State<App>, Form(f): Form<CleanForm>) -> Redirect {
    let dir = PathBuf::from(f.dir.trim());
    if f.dir.trim().is_empty() || !dir.is_dir() {
        app.jobs.note(
            true,
            format!("Clean previews: {} is not a folder", dir.display()),
        );
        return Redirect::to("/batch");
    }
    let dry_run = f.mode != "delete";
    app.jobs.spawn_read("Clean previews", move |_, _, _| {
        let (files, bytes) = batch::clean_previews(&dir, dry_run)?;
        Ok(if dry_run {
            format!(
                "{} preview files ({}) would be deleted",
                files.len(),
                util::human_size(bytes)
            )
        } else {
            format!(
                "{} files deleted, {} freed",
                files.len(),
                util::human_size(bytes)
            )
        })
    });
    Redirect::to("/batch")
}

// -------------------------------------------------------------- Duplicates

async fn duplicates(State(app): State<App>) -> Page {
    let a = app.clone();
    let groups = blocking(move || batch::duplicate_groups(&a.conn()?)).await?;
    let extra: usize = groups.iter().map(|g| g.len() - 1).sum();
    let body = html! {
        div class="toolbar" {
            span { (groups.len()) " groups of identical files (same SHA-256)" }
            @if !groups.is_empty() {
                form method="post" action="/duplicates/remove" {
                    button class="danger"
                        onclick={"return confirm('Delete " (extra) " duplicate files from disk, keeping one copy of each?')"}
                        { "Delete duplicates, keep one of each" }
                }
            }
        }
        @for g in &groups {
            details {
                summary { (g.len()) " × " (g[0].file_name()) }
                @for (i, f) in g.iter().enumerate() {
                    div { @if i == 0 { "keep " } @else { span class="failed" { "delete " } } (f.name) }
                }
            }
        }
    };
    Ok(page(&app, "/duplicates", true, body))
}

async fn duplicates_remove(State(app): State<App>) -> Redirect {
    app.jobs.spawn("Remove duplicates", |conn, _, _| {
        let (n, b) = batch::remove_duplicates(conn)?;
        Ok(format!("{n} files removed, {} freed", util::human_size(b)))
    });
    Redirect::to("/duplicates")
}

// ---------------------------------------------------------------- Mappings

async fn mappings(State(app): State<App>) -> Page {
    let a = app.clone();
    let list = blocking(move || db::mappings(&a.conn()?)).await?;
    let body = html! {
        p { "Header values are rewritten on import. Leave \"current value\" empty to fill in a missing or Unknown value." }
        form method="post" action="/mappings/add" class="toolbar" {
            select name="card" {
                @for c in ["TELESCOP", "INSTRUME", "OBSERVER", "OBJECT", "FILTER", "NOTES"] { option { (c) } }
            }
            input type="text" name="current" placeholder="current value";
            "→"
            input type="text" name="replace" placeholder="replacement" required;
            button { "Add" }
        }
        table style="width: auto" {
            tbody {
                @for m in &list {
                    tr {
                        td { (m.card) }
                        td { (m.current.as_deref().unwrap_or("(missing / Unknown)")) }
                        td { "→ " (opt(&m.replace)) }
                        td { form method="post" action="/mappings/remove" {
                            input type="hidden" name="id" value=(m.id);
                            button title="Remove" { "✖" }
                        } }
                    }
                }
            }
        }
    };
    Ok(page(&app, "/mappings", false, body))
}

#[derive(Deserialize)]
struct MappingForm {
    card: String,
    current: String,
    replace: String,
}

async fn mappings_add(
    State(app): State<App>,
    Form(f): Form<MappingForm>,
) -> Result<Redirect, AppError> {
    if !f.replace.trim().is_empty() {
        let a = app.clone();
        blocking(move || db::add_mapping(&a.conn()?, &f.card, &f.current, f.replace.trim()))
            .await?;
    }
    Ok(Redirect::to("/mappings"))
}

#[derive(Deserialize)]
struct MappingId {
    id: i64,
}

async fn mappings_remove(
    State(app): State<App>,
    Form(f): Form<MappingId>,
) -> Result<Redirect, AppError> {
    let a = app.clone();
    blocking(move || db::remove_mapping(&a.conn()?, f.id)).await?;
    Ok(Redirect::to("/mappings"))
}

// -------------------------------------------------------------- Statistics

fn bars(title: &str, rows: Vec<(String, f64)>, as_hours: bool) -> Markup {
    let max = rows.iter().map(|r| r.1).fold(0.0, f64::max).max(1e-9);
    html! {
        h2 { (title) }
        div class="bars" {
            @for (label, v) in &rows {
                span class="name" title=(label) { (label) }
                div { div class="bar" style={"width: " (format!("{:.1}", v / max * 100.0)) "%"} {} }
                span { @if as_hours { (stats::hours(*v)) } @else { (format!("{v:.0}")) } }
            }
        }
    }
}

async fn stats_page(State(app): State<App>) -> Page {
    let a = app.clone();
    let s = blocking(move || stats::compute(&a.conn()?)).await?;
    let cfg = app.cfg();
    let by_object: Vec<(String, f64)> = s
        .by_object
        .iter()
        .take(25)
        .map(|(o, n, e)| {
            let label = match names::common_name(o, &cfg.object_names) {
                Some(name) => format!("{o} {name} ({n})"),
                None => format!("{o} ({n})"),
            };
            (label, *e)
        })
        .collect();
    let count = |v: &[(String, usize)]| v.iter().map(|(t, n)| (t.clone(), *n as f64)).collect();
    let body = html! {
        div class="grid" {
            strong { "Files" }
            span { (s.total_files) " (" (s.light_files) " lights, " (s.calibration_files) " calibration)" }
            strong { "Size on disk" } span { (util::human_size(s.total_bytes)) }
            strong { "Sessions" } span { (s.sessions) }
            strong { "Date range" } span { (opt(&s.first_date)) " → " (opt(&s.last_date)) }
            strong { "Total integration" } span { (stats::hours(s.by_object.iter().map(|o| o.2).sum())) }
        }
        div class="columns" {
            div { (bars("Integration by object", by_object, true)) }
            div {
                (bars("Integration by filter", s.by_filter.clone(), true))
                (bars("Frames by telescope", count(&s.by_telescope), false))
                (bars("Frames by camera", count(&s.by_instrument), false))
            }
        }
    };
    Ok(page(&app, "/stats", true, body))
}

// ---------------------------------------------------------------- Settings

async fn settings(State(app): State<App>) -> Markup {
    let c = app.cfg();
    let object_names: String = c
        .object_names
        .iter()
        .map(|(o, n)| format!("{o} = {n}\n"))
        .collect();
    let body = html! {
        form method="post" action="/settings" {
            div class="grid" {
                label title="Where organised files are kept" { "Repository" }
                div { (folder_input("cfg_repo", "repo", &c.repo.to_string_lossy())) }
                label title="Default folder for Load" { "Incoming folder" }
                div { (folder_input("cfg_source", "source", &c.source.to_string_lossy())) }
                label { "Save fixed headers" }
                label { input type="checkbox" name="save_modified_headers" value="1"
                    checked[c.save_modified_headers];
                    " Write normalised headers into the filed FITS files" }
                label { "Name conflicts" }
                select name="on_conflict" {
                    @for v in OnConflict::ALL {
                        option value=(v.key()) selected[v == c.on_conflict] { (v.label()) }
                    }
                }
                label { "Object names" }
                div {
                    p class="muted" { "Well-known objects get their common name in the folder name, e.g. Light/M_76_Barbell_Nebula. Add names here, or change a built-in one, one per line as \"M 76 = Barbell Nebula\". Leave the name empty to use just the catalogue number." }
                    textarea name="object_names" rows="8" cols="50" { (object_names) }
                    p class="muted" { "After saving, use Batch > Folder layout to rename existing folders." }
                }
            }
            p { button { "Save settings" } }
        }
        p class="muted" { "Config file: " (c.path.display()) br; "Database: " (app.db_path.display()) }
    };
    page(&app, "/settings", false, body)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct SettingsForm {
    repo: String,
    source: String,
    save_modified_headers: Option<String>,
    on_conflict: String,
    object_names: String,
}

async fn settings_save(State(app): State<App>, Form(f): Form<SettingsForm>) -> Redirect {
    let mut c = app.cfg();
    c.repo = PathBuf::from(f.repo.trim());
    c.source = PathBuf::from(f.source.trim());
    c.save_modified_headers = ticked(&f.save_modified_headers);
    c.on_conflict = OnConflict::parse(&f.on_conflict).unwrap_or(c.on_conflict);
    c.object_names = f
        .object_names
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(o, n)| (o.trim().to_string(), n.trim().to_string()))
        .filter(|(o, _)| !o.is_empty())
        .collect();
    match c.save() {
        Ok(()) => {
            *app.cfg.write().unwrap() = c;
            app.jobs.note(false, "Settings saved".into());
        }
        Err(e) => app
            .jobs
            .note(true, format!("Could not save settings: {e:#}")),
    }
    Redirect::to("/settings")
}

// --------------------------------------------------------------------- Log

fn log_text() -> String {
    let lines = logging::recent();
    lines[lines.len().saturating_sub(LOG_LINES)..].join("\n")
}

async fn log_page(State(app): State<App>) -> Markup {
    let body = html! {
        div class="toolbar" {
            form method="post" action="/log/clear" { button { "Clear" } }
            span class="muted" { "The latest " (LOG_LINES) " lines, newest last." }
        }
        pre class="log" hx-get="/log/lines" hx-trigger="every 2s" { (log_text()) }
    };
    page(&app, "/log", false, body)
}

async fn log_lines() -> Markup {
    html! { (log_text()) }
}

async fn log_clear() -> Redirect {
    logging::clear();
    Redirect::to("/log")
}

// -------------------------------------------------------------------- Jobs

#[derive(Deserialize, Default)]
#[serde(default)]
struct JobsQuery {
    gen: u64,
}

async fn jobs(State(app): State<App>, Query(q): Query<JobsQuery>) -> Response {
    let snapshot = app.jobs.snapshot();
    let panel = jobs_panel(&snapshot);
    if snapshot.generation != q.gen {
        ([("HX-Trigger", "refresh")], panel).into_response()
    } else {
        panel.into_response()
    }
}

#[derive(Deserialize)]
struct JobName {
    name: String,
}

async fn jobs_cancel(
    State(app): State<App>,
    headers: axum::http::HeaderMap,
    Form(f): Form<JobName>,
) -> Redirect {
    app.jobs.cancel(&f.name);
    // Back to the page the button was on.
    let back = headers
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok())
        .and_then(|r| r.split_once("://"))
        .and_then(|(_, rest)| rest.find('/').map(|i| rest[i..].to_string()))
        .unwrap_or_else(|| "/images".into());
    Redirect::to(&back)
}

// ----------------------------------------------------------- Folder picker

#[derive(Deserialize, Default)]
#[serde(default)]
struct BrowseQuery {
    target: String,
    path: String,
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// The folders inside one folder, never above the picker's root.
async fn browse(State(app): State<App>, Query(q): Query<BrowseQuery>) -> Page {
    let target: String = q
        .target
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let a = app.clone();
    let (root, dir, subdirs) = blocking(move || {
        let root = a.root.canonicalize().unwrap_or_else(|_| a.root.clone());
        let dir = Path::new(q.path.trim())
            .canonicalize()
            .ok()
            .filter(|p| p.is_dir() && p.starts_with(&root))
            .unwrap_or_else(|| root.clone());
        let mut subdirs: Vec<String> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            // Hidden folders and the NAS's own (@eaDir, #recycle).
            .filter(|n| !n.starts_with(['.', '@', '#']))
            .collect();
        subdirs.sort_by_key(|n| n.to_lowercase());
        Ok((root, dir, subdirs))
    })
    .await?;
    let link = |p: &Path| {
        format!(
            "/browse?target={target}&path={}",
            url_encode(&p.to_string_lossy())
        )
    };
    Ok(html! {
        p { strong { (dir.display()) } }
        div class="dirs" {
            @if dir != root {
                @if let Some(up) = dir.parent() {
                    button type="button" hx-get=(link(up)) hx-target="#browse-body" { "↑ Up" }
                }
            }
            @for name in &subdirs {
                button type="button" hx-get=(link(&dir.join(name))) hx-target="#browse-body" { (name) }
            }
            @if subdirs.is_empty() { p class="muted" { "No folders in here." } }
        }
        button type="button" data-target=(target) data-path=(dir.to_string_lossy())
            onclick="pick(this.dataset.target, this.dataset.path)" { "Choose this folder" }
    })
}
