//! Musik Faktory UI (Dioxus web app, installable as a PWA).

mod api;
mod editor;
mod home;
mod parent;
mod studio;
mod track;
mod util;

use dioxus::prelude::*;
use musik_shared::*;

use home::Home;
use parent::Parent;
use track::TrackPage;

const CSS: &str = include_str!("../assets/style.css");

#[derive(Routable, Clone, PartialEq, Debug)]
#[rustfmt::skip]
enum Route {
    #[layout(Shell)]
        #[route("/")]
        Home {},
        #[route("/track/:id")]
        TrackPage { id: String },
        #[route("/parent")]
        Parent {},
        #[route("/:..segments")]
        NotFound { segments: Vec<String> },
}

/// App-wide state.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub status: Signal<Option<AppStatus>>,
    /// Parent session token. Kept in memory only (never in browser storage)
    /// and dropped as soon as the Parents page is left.
    pub token: Signal<Option<String>>,
    /// Message for the parent login screen (e.g. "session timed out").
    pub admin_notice: Signal<Option<String>>,
    /// Unsaved control-room edits kept across a session timeout:
    /// (settings they were based on, edited settings).
    pub kept_draft: Signal<Option<(Settings, Settings)>>,
}

impl Ctx {
    pub fn refresh_status(self) {
        let mut st = self.status;
        spawn(async move {
            if let Ok(s) = api::status().await {
                st.set(Some(s));
            }
        });
    }

    pub fn status(self) -> AppStatus {
        self.status.read().clone().unwrap_or_default()
    }

    pub fn set_token(self, token: Option<String>) {
        let mut t = self.token;
        t.set(token);
    }

    /// The parent session ended: go back to the login screen, keeping any
    /// unsaved settings.
    pub fn session_expired(self, unsaved: Option<(Settings, Settings)>) {
        let mut kept = self.kept_draft;
        if unsaved.is_some() {
            kept.set(unsaved);
        }
        let mut n = self.admin_notice;
        n.set(Some(if kept.peek().is_some() {
            "Your parent session timed out. Unlock again: your unsaved changes are kept.".into()
        } else {
            "Your parent session timed out. Please unlock again.".into()
        }));
        self.set_token(None);
    }
}

fn main() {
    // Readable panic messages in the browser console (release builds too).
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    let ctx = use_context_provider(|| Ctx {
        status: Signal::new(None),
        token: Signal::new(None),
        admin_notice: Signal::new(None),
        kept_draft: Signal::new(None),
    });
    // Keep the budget display fresh (another device may be spending too).
    use_future(move || async move {
        loop {
            if let Ok(s) = api::status().await {
                let mut st = ctx.status;
                st.set(Some(s));
            }
            gloo_timers::future::TimeoutFuture::new(20_000).await;
        }
    });
    rsx! {
        document::Title { "Musik Faktory" }
        style { {CSS} }
        Router::<Route> {}
    }
}

#[component]
fn Shell() -> Element {
    let ctx = use_context::<Ctx>();
    // Leaving the Parents page always locks the control room again, so the
    // Chromebook can be handed back to the child safely. (Done here in the
    // always-mounted layout, not while the page is being torn down.)
    let on_parent_page = matches!(use_route::<Route>(), Route::Parent {});
    use_effect(use_reactive((&on_parent_page,), move |(on_parent_page,)| {
        if !on_parent_page {
            // Bind first: a guard from `peek()` in the `if let` would still be
            // alive inside the block and make `set_token` panic.
            // Edits kept for a quick re-unlock don't outlive the visit.
            let mut kept = ctx.kept_draft;
            kept.set(None);
            let mut notice = ctx.admin_notice;
            notice.set(None);
            let current = ctx.token.peek().clone();
            if let Some(t) = current {
                ctx.set_token(None);
                spawn(async move {
                    let _ = api::admin_logout(&t).await;
                });
            }
        }
    }));
    let st = ctx.status.read().clone();
    rsx! {
        header { class: "topbar",
            Link { to: Route::Home {}, class: "brand",
                img { src: "/icon.svg", alt: "", width: "36", height: "36" }
                span { "Musik Faktory" span { class: "sub", "HOME STUDIO" } }
            }
            if let Some(st) = st {
                BudgetPill { status: st }
            }
            nav { class: "nav",
                Link { to: Route::Home {}, class: "navlink", active_class: "active", "🎵 My songs" }
                Link { to: Route::Parent {}, class: "navlink", active_class: "active", "🔒 Parents" }
            }
        }
        if ctx.status.read().as_ref().map(|s| s.mock_mode).unwrap_or(false) {
            div { class: "mockbar", "🧪 Mock mode: songs are pretend beeps and cost nothing." }
        }
        main { class: "page", Outlet::<Route> {} }
    }
}

/// Budget read-out in the top bar: LCD + LED meter of what's left.
#[component]
fn BudgetPill(status: AppStatus) -> Element {
    if !status.generation_enabled {
        return rsx! {
            div { class: "lcd red", title: "A parent paused making new songs",
                div { class: "lcd-cell", span { class: "lbl", "STUDIO" } span { class: "val", "⏸ PAUSED" } }
            }
        };
    }
    let b = &status.budget;
    // Fraction left of the tightest active limit.
    let frac = [b.week.as_ref(), b.month.as_ref()]
        .into_iter()
        .flatten()
        .filter(|p| p.limit > 0.0)
        .map(|p| p.remaining / p.limit)
        .reduce(f64::min);
    match (b.remaining, b.remaining_seconds) {
        (Some(left), Some(secs)) => {
            let tone = if secs < 30 { "red" } else if frac.unwrap_or(1.0) < 0.34 { "amber" } else { "" };
            rsx! {
                div { class: "row", title: "Budget left before the next refill",
                    div { class: "lcd {tone}",
                        div { class: "lcd-cell", span { class: "lbl", "BUDGET LEFT" } span { class: "val", "{format_money(left, &status.currency)}" } }
                        div { class: "lcd-cell", span { class: "lbl", "MUSIC LEFT" } span { class: "val", "{format_duration(secs.saturating_mul(1000))}" } }
                    }
                    studio::Meter { fraction: frac.unwrap_or(1.0), segments: 14, remaining: true }
                }
            }
        }
        _ => rsx! {
            div { class: "lcd", div { class: "lcd-cell", span { class: "lbl", "BUDGET" } span { class: "val", "NO LIMIT" } } }
        },
    }
}

#[component]
fn NotFound(segments: Vec<String>) -> Element {
    rsx! {
        div { class: "card center",
            h2 { "🙈 This page doesn't exist" }
            Link { to: Route::Home {}, class: "btn primary", "Back to my songs" }
        }
    }
}
