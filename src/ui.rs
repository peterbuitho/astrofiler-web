//! Page frame and the pieces every page shares.

use crate::jobs::Snapshot;
use crate::AppState;
use maud::{html, Markup, DOCTYPE};

pub const TABS: &[(&str, &str)] = &[
    ("/images", "Images"),
    ("/load", "Load"),
    ("/sessions", "Sessions"),
    ("/batch", "Batch"),
    ("/duplicates", "Duplicates"),
    ("/mappings", "Mappings"),
    ("/stats", "Statistics"),
    ("/settings", "Settings"),
    ("/log", "Log"),
];

/// A whole page. `live` pages reload their content when a task finishes;
/// pages with forms being filled in are left alone.
pub fn page(app: &AppState, active: &str, live: bool, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="theme-color" content="#1b1c1f";
                meta name="mobile-web-app-capable" content="yes";
                meta name="apple-mobile-web-app-capable" content="yes";
                title { "AstroFiler" }
                link rel="stylesheet" href="/assets/style.css";
                script src="/assets/htmx.js" {}
                script src="/assets/app.js" {}
            }
            body {
                nav {
                    strong { "AstroFiler" }
                    small class="version" { "v" (env!("CARGO_PKG_VERSION")) }
                    @for (href, label) in TABS {
                        a href=(href) class=[(*href == active).then_some("active")] { (label) }
                    }
                }
                (jobs_panel(&app.jobs.snapshot()))
                @if live {
                    main hx-get=(active) hx-trigger="refresh from:body" hx-select="main"
                        hx-target="this" hx-swap="outerHTML"
                        hx-disinherit="hx-select hx-target hx-swap" { (body) }
                } @else {
                    main { (body) }
                }
                dialog id="browse" {
                    div id="browse-body" {}
                    p { button type="button" onclick="el('browse').close()" { "Cancel" } }
                }
            }
        }
    }
}

/// Running and waiting tasks and the latest results. It polls itself, and
/// tells the page to refresh when something finished since the last poll.
pub fn jobs_panel(s: &Snapshot) -> Markup {
    let busy = !s.running.is_empty() || !s.queued.is_empty();
    html! {
        div id="jobs" hx-get={"/jobs?gen=" (s.generation)}
            hx-trigger=(if busy { "every 1s" } else { "every 3s" }) hx-swap="outerHTML" {
            @for (name, done, total, msg) in &s.running {
                div class="job" {
                    strong { (name) }
                    progress value=(done) max=((*total).max(1)) {}
                    span { (done) " / " (total) " " (msg) }
                    form method="post" action="/jobs/cancel" {
                        input type="hidden" name="name" value=(name);
                        button { "Cancel" }
                    }
                }
            }
            @for name in &s.queued {
                div class="job" {
                    strong { (name) }
                    span { "waiting for the running task" }
                    form method="post" action="/jobs/cancel" {
                        input type="hidden" name="name" value=(name);
                        button { "Cancel" }
                    }
                }
            }
            @if let Some((failed, text)) = s.notes.first() {
                details class="notes" {
                    summary class=[failed.then_some("failed")] { (text) }
                    @for (failed, text) in s.notes.iter().skip(1) {
                        div class=[failed.then_some("failed")] { (text) }
                    }
                }
            }
        }
    }
}

/// A folder field with a Browse button that opens the folder picker.
pub fn folder_input(id: &str, name: &str, value: &str) -> Markup {
    html! {
        input type="text" id=(id) name=(name) value=(value) size="50";
        button type="button" data-target=(id) onclick="browse(this.dataset.target)" { "Browse…" }
    }
}

pub fn opt(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("")
}
