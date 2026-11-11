//! Home page: the list of songs.

use crate::studio::{Lcd, Wave};
use crate::{api, util, Ctx, Route};
use dioxus::prelude::*;

#[component]
pub fn Home() -> Element {
    let ctx = use_context::<Ctx>();
    let nav = navigator();
    let mut tracks = use_resource(|| async { api::tracks().await });
    let mut title = use_signal(String::new);
    let mut creating = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let status = ctx.status();

    let mut create = move || {
        if creating() {
            return;
        }
        creating.set(true);
        let t = title();
        spawn(async move {
            match api::create_track(t).await {
                Ok(track) => {
                    nav.push(Route::TrackPage { id: track.id });
                }
                Err(e) => error.set(Some(e.message)),
            }
            creating.set(false);
        });
    };

    rsx! {
        document::Title { "Musik Faktory" }
        if status.setup_needed {
            div { class: "banner warn",
                "👋 Before the first song, a parent needs to set up the app. "
                Link { to: Route::Parent {}, "Go to the Parents page →" }
            }
        } else if !status.api_key_configured {
            div { class: "banner warn",
                "🔑 The ElevenLabs key is missing. Ask a parent to add it on the "
                Link { to: Route::Parent {}, "Parents page" }
                "."
            }
        }

        section { class: "panel hero",
            div { class: "eq eq-deco", for i in 0..7 { i { key: "{i}" } } }
            h1 { "What song shall we " span { class: "grad", "make today" } "? 🎶" }
            p { class: "muted", "Every song is a project. Each time you change something you get a new take — and you can always go back." }
            div { class: "row",
                input {
                    class: "big-input",
                    placeholder: "Name your new project, e.g. “Pizza Cat Disco”",
                    value: "{title}",
                    maxlength: "80",
                    oninput: move |e| title.set(e.value()),
                    onkeydown: move |e| if e.key() == Key::Enter { create() },
                }
                button { class: "btn primary big", disabled: creating(), onclick: move |_| create(), "＋ New project" }
            }
            if let Some(e) = error() {
                p { class: "error", "{e}" }
            }
        }

        match &*tracks.read() {
            None => rsx! { p { class: "muted center", "Loading your songs…" } },
            Some(Err(e)) => rsx! {
                div { class: "banner error",
                    "{e.message} "
                    button { class: "btn small", onclick: move |_| tracks.restart(), "Try again" }
                }
            },
            Some(Ok(list)) if list.is_empty() => rsx! {
                div { class: "empty",
                    div { class: "big", "🎧" }
                    p { "The studio is empty. Give your first song a name above and press “New project”!" }
                }
            },
            Some(Ok(list)) => rsx! {
                div { class: "row between", style: "margin-bottom: 10px",
                    h3 { class: "muted", "📁 My projects" }
                    Lcd { label: "PROJECTS", value: format!("{:02}", list.len()) }
                }
                div { class: "grid",
                    for t in list.iter().cloned() {
                        Link { key: "{t.id}", to: Route::TrackPage { id: t.id.clone() }, class: "project",
                            div { class: "top",
                                div { class: "emoji", "{t.emoji}" }
                                div {
                                    h3 { "{t.title}" }
                                    p { class: "tiny muted", "Changed {util::fmt_datetime(&t.updated_at)}" }
                                }
                            }
                            div { class: "mini-wave", style: "color: hsl({(t.id.bytes().map(|b| b as u32).sum::<u32>() * 37) % 360}, 75%, 62%)",
                                Wave { seed: t.id.clone(), bars: 48 }
                            }
                            div { class: "row",
                                span { class: "tag",
                                    if t.version_count == 1 { "1 take" } else { "{t.version_count} takes" }
                                }
                                if let Some(l) = &t.latest_label { span { class: "tag mono", "latest {l}" } }
                                if let Some(f) = &t.favorite_label { span { class: "tag", "★ {f}" } }
                            }
                            if t.generating {
                                div { class: "badge", span { class: "led rec" } "REC" }
                            }
                        }
                    }
                }
            },
        }
    }
}
