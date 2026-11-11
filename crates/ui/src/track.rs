//! Session view of one song: takes (version tree), the deck (player with
//! arrangement timeline) and the take details.

use crate::editor::{fresh_seed, seed_from_error, seed_from_version, Composer, EditorSeed};
use crate::studio::{clip_color, clip_name, Lane, Lcd};
use crate::{api, util, Ctx, Route};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use musik_shared::*;
use wasm_bindgen::JsCast;

#[component]
pub fn TrackPage(id: String) -> Element {
    // Re-mount everything when switching to another song. The inner block
    // makes TrackView the first node of its own fragment, so the key counts.
    rsx! {
        {
            rsx! { TrackView { key: "{id}", id: id.clone() } }
        }
    }
}

/// The app status, fetching it if it isn't known yet.
async fn known_status(ctx: Ctx) -> Option<AppStatus> {
    // Bind first: never hold a signal guard across the await below.
    let known = ctx.status.peek().clone();
    if known.is_some() {
        return known;
    }
    let st = api::status().await.ok()?;
    let mut s = ctx.status;
    s.set(Some(st.clone()));
    Some(st)
}

fn generating(t: &TrackDetail) -> bool {
    t.versions.iter().any(|v| v.status == VersionStatus::Generating)
}

/// Pick what to show when nothing (valid) is selected.
fn default_selection(t: &TrackDetail) -> Option<String> {
    let ready = t.versions.iter().filter(|v| v.status == VersionStatus::Ready);
    t.versions
        .iter()
        .find(|v| v.favorite && v.status == VersionStatus::Ready)
        .or_else(|| ready.max_by(|a, b| a.created_at.cmp(&b.created_at)))
        .or_else(|| t.versions.iter().max_by(|a, b| a.created_at.cmp(&b.created_at)))
        .map(|v| v.id.clone())
}

#[component]
fn TrackView(id: String) -> Element {
    let ctx = use_context::<Ctx>();
    let nav = navigator();
    let track_id = use_signal(|| id.clone());
    let mut track = use_signal(|| None::<TrackDetail>);
    let mut load_error = use_signal(|| None::<String>);
    let mut selected = use_signal(|| None::<String>);
    let mut editor = use_signal(|| None::<EditorSeed>);
    let mut editor_nonce = use_signal(|| 0u32);
    let mut refresh = use_signal(|| 0u32);
    let mut title_draft = use_signal(String::new);

    // Loader + poller: refetch when asked, and every 2 s while a take is recording.
    use_future(move || async move {
        let mut seen = u32::MAX;
        let mut was_generating = false;
        let mut title_loaded = false;
        let mut auto_open = true;
        // After a take finishes the server still measures its cost for a
        // moment (studio "busy"): keep refreshing the status until it's idle.
        let mut settling = 0u32;
        let mut idle_ticks = 0u32;
        loop {
            let want = *refresh.peek();
            let gen_now = track.peek().as_ref().map(generating).unwrap_or(false);
            idle_ticks += 1;
            // Also refresh now and then: another device may have changed the song.
            if want != seen || gen_now || idle_ticks >= 33 {
                idle_ticks = 0;
                seen = want;
                match api::get_track(&track_id.peek()).await {
                    Ok(t) => {
                        let is_gen = generating(&t);
                        if was_generating && !is_gen {
                            ctx.refresh_status();
                            settling = 40;
                        }
                        was_generating = is_gen;
                        let sel = selected.peek().clone();
                        if sel.as_ref().map(|s| !t.versions.iter().any(|v| &v.id == s)).unwrap_or(true) {
                            selected.set(default_selection(&t));
                        }
                        if !title_loaded {
                            title_draft.set(t.title.clone());
                            title_loaded = true;
                        }
                        // Open the composer on an empty song, but only with the
                        // real limits (max length, defaults) known.
                        if auto_open {
                            if t.versions.is_empty() && editor.peek().is_none() {
                                if let Some(st) = known_status(ctx).await {
                                    editor.set(Some(fresh_seed(&st)));
                                    auto_open = false;
                                }
                            } else {
                                auto_open = false;
                            }
                        }
                        load_error.set(None);
                        track.set(Some(t));
                    }
                    Err(e) => load_error.set(Some(e.message)),
                }
            }
            if settling > 0 {
                settling -= 1;
                if settling % 3 == 0 {
                    let busy = ctx.status.peek().as_ref().map(|s| s.busy).unwrap_or(false);
                    if busy {
                        ctx.refresh_status();
                    } else {
                        settling = 0;
                    }
                }
            }
            TimeoutFuture::new(if gen_now { 2000 } else { 300 }).await;
        }
    });

    let mut open_editor = move |seed: EditorSeed| {
        editor_nonce += 1;
        editor.set(Some(seed));
    };
    // Seeds need the real limits; while they're unknown, ask for them first.
    let mut open_fresh = move || match ctx.status.peek().clone() {
        Some(st) => open_editor(fresh_seed(&st)),
        None => ctx.refresh_status(),
    };
    let mut open_remix = move |v: VersionInfo| match ctx.status.peek().clone() {
        Some(st) => open_editor(seed_from_version(&v, &st)),
        None => ctx.refresh_status(),
    };

    let Some(t) = track() else {
        return rsx! {
            if let Some(e) = load_error() {
                div { class: "banner error", "{e} " Link { to: Route::Home {}, "Back to my songs" } }
            } else {
                div { class: "making", div { class: "eq", for i in 0..7 { i { key: "{i}" } } } p { class: "muted", "Loading the session…" } }
            }
        };
    };

    let sel_version = selected().and_then(|s| t.versions.iter().find(|v| v.id == s).cloned());
    let status = ctx.status();

    let save_title = move || {
        let new_title = title_draft();
        spawn(async move {
            if let Ok(t) = api::update_track(&track_id.peek(), UpdateTrackReq { title: Some(new_title), emoji: None }).await {
                title_draft.set(t.title.clone());
                track.set(Some(t));
            }
        });
    };

    let delete_song = move |_| {
        let n = track.peek().as_ref().map(|t| t.versions.len()).unwrap_or(0);
        if !util::confirm(&format!("Delete this whole song and its {n} take(s)? This can't be undone.")) {
            return;
        }
        spawn(async move {
            match api::delete_track(&track_id.peek()).await {
                Ok(_) => {
                    nav.push(Route::Home {});
                }
                Err(e) => load_error.set(Some(e.message)),
            }
        });
    };

    rsx! {
        document::Title { "{t.title} · Musik Faktory" }
        div { class: "session-head",
            Link { to: Route::Home {}, class: "btn small ghost", "◀ Songs" }
            span { class: "emoji", "{t.emoji}" }
            input {
                class: "title-input",
                value: "{title_draft}",
                maxlength: "80",
                title: "Click to rename",
                oninput: move |e| title_draft.set(e.value()),
                onblur: move |_| save_title(),
                onkeydown: move |e| if e.key() == Key::Enter { save_title() },
            }
            Lcd { label: "TAKES", value: format!("{:02}", t.versions.len()) }
            button { class: "btn small ghost", onclick: delete_song, "🗑 Delete song" }
        }

        if let Some(e) = load_error() {
            div { class: "banner error", "{e}" }
        }
        if let Some(err) = t.last_error.clone() {
            ErrorBanner {
                err: err.clone(),
                on_fix: {
                    let versions = t.versions.clone();
                    move |use_suggestion: bool| open_editor(seed_from_error(&err, &versions, use_suggestion))
                },
                on_dismiss: move |_| {
                    spawn(async move {
                        if let Ok(t) = api::dismiss_error(&track_id.peek()).await {
                            track.set(Some(t));
                        }
                    });
                },
            }
        }

        div { class: "session",
            TakesPanel {
                versions: t.versions.clone(),
                selected: selected(),
                on_select: move |id: String| selected.set(Some(id)),
                on_new: move |_| open_fresh(),
            }
            div { class: "stack",
                if let Some(seed) = editor() {
                    if let Some(p) = seed.parent.clone().filter(|p| p.status == VersionStatus::Ready) {
                        Deck { key: "deck-{p.id}", version: p.clone(), compact: true }
                    }
                    {
                        rsx! {
                            Composer {
                                key: "{editor_nonce}",
                                track_id: t.id.clone(),
                                seed,
                                on_close: move |_| editor.set(None),
                                on_started: move |v: VersionInfo| {
                                    editor.set(None);
                                    selected.set(Some(v.id.clone()));
                                    refresh += 1;
                                    ctx.refresh_status();
                                },
                            }
                        }
                    }
                } else if let Some(v) = sel_version.clone() {
                    if v.status == VersionStatus::Generating {
                        Recording { version: v.clone(), currency: status.currency.clone() }
                    } else {
                        Deck { key: "deck-{v.id}", version: v.clone(), compact: false }
                        {
                            rsx! {
                                TakeInfo {
                                    key: "info-{v.id}",
                                    version: v.clone(),
                                    parent_label: v.parent_id.as_ref().and_then(|p| t.versions.iter().find(|x| &x.id == p)).map(|p| p.label.clone()),
                                    descendants: count_descendants(&t.versions, &v.id),
                                    currency: status.currency.clone(),
                                    on_remix: move |v: VersionInfo| open_remix(v),
                                    on_changed: move |_| refresh += 1,
                                    on_select: move |id: String| selected.set(Some(id)),
                                }
                            }
                        }
                    }
                } else {
                    div { class: "panel",
                        div { class: "panel-head", span { class: "title", "Studio" } }
                        div { class: "panel-body center",
                            p { class: "muted", "No takes yet." }
                            button { class: "btn primary", onclick: move |_| open_fresh(), "🎛 Start composing" }
                        }
                    }
                }
            }
        }
    }
}

fn count_descendants(versions: &[VersionInfo], id: &str) -> usize {
    let mut stack = vec![id.to_string()];
    let mut n = 0;
    while let Some(cur) = stack.pop() {
        for v in versions.iter().filter(|v| v.parent_id.as_deref() == Some(cur.as_str())) {
            n += 1;
            stack.push(v.id.clone());
        }
    }
    n
}

/// Depth-first list of the version tree. For every row: the version, for
/// each ancestor level whether that ancestor was the last child, and whether
/// the row itself is the last child.
fn flatten(versions: &[VersionInfo]) -> Vec<(VersionInfo, Vec<bool>, bool)> {
    fn walk(parent: &str, all: &[VersionInfo], flags: &[bool], out: &mut Vec<(VersionInfo, Vec<bool>, bool)>) {
        let mut kids: Vec<&VersionInfo> = all.iter().filter(|v| v.parent_id.as_deref() == Some(parent)).collect();
        kids.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        let n = kids.len();
        for (k, c) in kids.into_iter().enumerate() {
            let last = k + 1 == n;
            out.push((c.clone(), flags.to_vec(), last));
            let mut f = flags.to_vec();
            f.push(last);
            walk(&c.id, all, &f, out);
        }
    }
    let ids: std::collections::HashSet<&str> = versions.iter().map(|v| v.id.as_str()).collect();
    let mut roots: Vec<&VersionInfo> = versions
        .iter()
        .filter(|v| v.parent_id.as_deref().map(|p| !ids.contains(p)).unwrap_or(true))
        .collect();
    roots.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let mut out = Vec::new();
    for r in roots {
        out.push((r.clone(), vec![], true));
        // Level 0 = the roots themselves; they have no connector line.
        walk(&r.id, versions, &[true], &mut out);
    }
    out
}

#[component]
fn TakesPanel(versions: Vec<VersionInfo>, selected: Option<String>, on_select: EventHandler<String>, on_new: EventHandler<()>) -> Element {
    let rows = flatten(&versions);
    rsx! {
        div { class: "panel takes",
            div { class: "panel-head", span { class: "title", "Takes" } span { class: "push", "{versions.len()}" } }
            div { class: "panel-body",
                div { class: "take-list",
                    if rows.is_empty() {
                        p { class: "muted small center", "Your takes will appear here." }
                    }
                    for (v, flags, _last) in rows {
                        {
                            let sel = selected.as_deref() == Some(v.id.as_str());
                            let color = util::branch_color(&v.branch);
                            let id = v.id.clone();
                            let led = match v.status {
                                VersionStatus::Generating => "led rec",
                                _ if sel => "led on",
                                _ => "led",
                            };
                            let name = if !v.name.is_empty() {
                                v.name.clone()
                            } else {
                                v.song_metadata.as_ref().and_then(|m| m.title.clone()).unwrap_or_default()
                            };
                            let depth = flags.len();
                            rsx! {
                                button {
                                    key: "{v.id}",
                                    class: if sel { "take sel" } else { "take" },
                                    onclick: move |_| on_select.call(id.clone()),
                                    span { class: "stripe", style: "background: {color}" }
                                    if depth > 0 {
                                        span { class: "indent",
                                            for (lvl, last) in flags.iter().enumerate().skip(1) {
                                                span { key: "{lvl}", class: if *last { "tick last" } else { "tick" } }
                                            }
                                        }
                                        span { class: "elbow" }
                                    }
                                    span { class: "{led}" }
                                    span { class: "label", style: "color: {color}", "{v.label}" }
                                    span { class: "name", "{name}" }
                                    if v.favorite { span { class: "star", "★" } }
                                    span { class: "dur", "{format_duration(v.duration_ms)}" }
                                }
                            }
                        }
                    }
                }
                div { class: "row", style: "margin-top: 10px",
                    button { class: "btn small ghost", style: "width: 100%", onclick: move |_| on_new.call(()), "＋ New take from scratch" }
                }
            }
        }
    }
}

/// Chunks to show on the timeline of a version.
fn version_chunks(v: &VersionInfo) -> Vec<PlanChunk> {
    match &v.plan {
        Some(c) if !c.is_empty() && total_duration_ms(c) > 0 => c.clone(),
        _ => vec![PlanChunk::Generate(GenChunk {
            text: "[Full song]".into(),
            duration_ms: v.duration_ms.max(1),
            ..GenChunk::default()
        })],
    }
}

fn fmt_time(secs: f64) -> String {
    let s = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
    format!("{:02}:{:04.1}", (s / 60.0).floor() as u32, s % 60.0)
}

/// The player: transport, time LCD, arrangement timeline with playhead.
#[component]
pub fn Deck(version: VersionInfo, compact: bool) -> Element {
    let mut audio = use_signal(|| None::<web_sys::HtmlMediaElement>);
    let mut pos = use_signal(|| 0.0f64);
    let mut length = use_signal(|| version.duration_ms as f64 / 1000.0);
    let mut playing = use_signal(|| false);
    let mut volume = use_signal(|| 0.9f64);
    let chunks = version_chunks(&version);
    let src = version.audio_url.clone().unwrap_or_default();
    let title = version
        .song_metadata
        .as_ref()
        .and_then(|m| m.title.clone())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Untitled take".into());
    let frac = if length() > 0.0 { pos() / length() } else { 0.0 };

    let toggle = move |_| {
        if let Some(a) = audio.read().as_ref() {
            if a.paused() {
                let _ = a.play();
            } else {
                let _ = a.pause();
            }
        }
    };
    let restart = move |_| {
        if let Some(a) = audio.read().as_ref() {
            a.set_current_time(0.0);
            pos.set(0.0);
        }
    };

    rsx! {
        div { class: "panel deck",
            div { class: "panel-head",
                span { class: "title", if compact { "Listening to {version.label}" } else { "Deck · {version.label}" } }
                span { class: "push", if playing() { span { class: "led on" } } else { span { class: "led" } } }
            }
            div { class: "panel-body",
                audio {
                    src: "{src}",
                    preload: "metadata",
                    onmounted: move |e| {
                        use dioxus::web::WebEventExt;
                        let el: web_sys::Element = e.data().as_web_event();
                        if let Ok(a) = el.dyn_into::<web_sys::HtmlMediaElement>() {
                            a.set_volume(volume());
                            audio.set(Some(a));
                        }
                    },
                    ontimeupdate: move |_| {
                        if let Some(a) = audio.read().as_ref() {
                            pos.set(a.current_time());
                        }
                    },
                    onloadedmetadata: move |_| {
                        if let Some(a) = audio.read().as_ref() {
                            let d = a.duration();
                            if d.is_finite() && d > 0.0 {
                                length.set(d);
                            }
                        }
                    },
                    onplay: move |_| playing.set(true),
                    onpause: move |_| playing.set(false),
                    onended: move |_| playing.set(false),
                }
                div { class: "deck-top",
                    div { class: "transport",
                        button { class: "tbtn", title: "Back to start", onclick: restart, "⏮" }
                        button {
                            class: if playing() { "tbtn play on" } else { "tbtn play" },
                            title: if playing() { "Pause" } else { "Play" },
                            onclick: toggle,
                            if playing() { "⏸" } else { "▶" }
                        }
                    }
                    Lcd { label: "TIME", value: format!("{} / {}", fmt_time(pos()), fmt_time(length())), big: !compact }
                    if !compact {
                        div { class: "deck-title",
                            h2 { "{title}" }
                            if let Some(m) = &version.song_metadata {
                                div { class: "tags",
                                    for g in m.genres.iter() { span { key: "{g}", class: "tag", "{g}" } }
                                    if m.is_explicit == Some(true) { span { class: "tag neg", "explicit" } }
                                }
                            }
                        }
                    }
                    div { class: "volume", title: "Volume",
                        "🔊"
                        input {
                            r#type: "range", min: "0", max: "1", step: "0.05", value: "{volume}",
                            oninput: move |e| {
                                if let Ok(v) = e.value().parse::<f64>() {
                                    volume.set(v);
                                    if let Some(a) = audio.read().as_ref() {
                                        a.set_volume(v);
                                    }
                                }
                            },
                        }
                    }
                }
                Lane {
                    chunks,
                    playhead: Some(frac),
                    on_seek: move |f: f64| {
                        if let Some(a) = audio.read().as_ref() {
                            let t = f * length();
                            a.set_current_time(t);
                            pos.set(t);
                        }
                    },
                }
            }
        }
    }
}

#[component]
fn Recording(version: VersionInfo, currency: String) -> Element {
    rsx! {
        div { class: "panel",
            div { class: "panel-head", span { class: "led rec" } span { class: "title", "Recording {version.label}" } }
            div { class: "panel-body making",
                div { class: "eq", for i in 0..7 { i { key: "{i}" } } }
                h2 { "The band is playing your song… 🎶" }
                p { class: "muted", "This usually takes 20 seconds to a couple of minutes. You can stay here and listen to other takes meanwhile." }
                div { class: "row",
                    Lcd { label: "LENGTH", value: format_duration(version.duration_ms) }
                    Lcd { label: "COST ≈", value: format_money(version.estimated_cost, &currency), tone: "amber" }
                }
            }
        }
    }
}

#[component]
fn TakeInfo(
    version: VersionInfo,
    parent_label: Option<String>,
    descendants: usize,
    currency: String,
    on_remix: EventHandler<VersionInfo>,
    on_changed: EventHandler<()>,
    on_select: EventHandler<String>,
) -> Element {
    let mut name = use_signal(|| version.name.clone());
    let mut notes = use_signal(|| version.notes.clone());
    let mut err = use_signal(|| None::<String>);
    let vid = use_signal(|| version.id.clone());
    let p = &version.params;
    let chunks = version_chunks(&version);

    let save = move |req: UpdateVersionReq| {
        spawn(async move {
            match api::update_version(&vid.peek(), req).await {
                Ok(_) => on_changed.call(()),
                Err(e) => err.set(Some(e.message)),
            }
        });
    };
    let fav = version.favorite;
    let v_for_remix = version.clone();
    let delete = move |_| {
        let extra = if descendants > 0 { format!(" and the {descendants} take(s) made from it") } else { String::new() };
        if !util::confirm(&format!("Delete this take{extra}? This can't be undone.")) {
            return;
        }
        spawn(async move {
            match api::delete_version(&vid.peek()).await {
                Ok(_) => on_changed.call(()),
                Err(e) => err.set(Some(e.message)),
            }
        });
    };

    rsx! {
        div { class: "panel",
            div { class: "panel-head", span { class: "title", "Take {version.label}" } span { class: "push", "{util::fmt_datetime(&version.created_at)}" } }
            div { class: "panel-body stack",
                div { class: "row",
                    button { class: "btn primary big", onclick: move |_| on_remix.call(v_for_remix.clone()), "🎛 Remix this take" }
                    button {
                        class: "btn",
                        title: "Star your favourite takes",
                        onclick: move |_| save(UpdateVersionReq { favorite: Some(!fav), ..Default::default() }),
                        if fav { "★ Starred" } else { "☆ Star" }
                    }
                    a { class: "btn", href: "/api/versions/{version.id}/download", download: "", "⬇ Export" }
                    div { class: "spacer" }
                    button { class: "btn danger small", onclick: delete, "🗑 Delete take" }
                }
                if let Some(e) = err() { p { class: "error", "{e}" } }

                div { class: "info-grid",
                    div { class: "field",
                        label { "Nickname" }
                        input {
                            r#type: "text",
                            placeholder: "e.g. louder chorus",
                            maxlength: "60",
                            value: "{name}",
                            oninput: move |e| name.set(e.value()),
                            onblur: move |_| save(UpdateVersionReq { name: Some(name()), ..Default::default() }),
                        }
                    }
                    dl { class: "kv",
                        dt { "made from" }
                        dd {
                            match (&version.parent_id, &parent_label) {
                                (Some(pid), Some(l)) => {
                                    let pid = pid.clone();
                                    rsx! { a { href: "#", onclick: move |e| { e.prevent_default(); on_select.call(pid.clone()) }, "{l}" } }
                                }
                                _ => rsx! { "a fresh start" },
                            }
                        }
                        dt { "length" } dd { "{format_duration(version.duration_ms)}" }
                        dt { "cost" }
                        dd {
                            "{format_money(version.charged_cost.unwrap_or(version.estimated_cost), &currency)}"
                            if let Some(c) = version.measured_credits { span { class: "muted tiny", " ({c} credits)" } }
                        }
                        dt { "model" } dd { "{p.model_id}" }
                        if let Some(seed) = p.seed { dt { "seed" } dd { class: "mono", "{seed}" } }
                        dt { "editable parts" } dd { if version.song_id.is_some() { "yes 🔒🎯" } else { "no" } }
                    }
                }

                if let Some(desc) = version.song_metadata.as_ref().and_then(|m| m.description.clone()).filter(|d| !d.is_empty()) {
                    p { class: "muted", "{desc}" }
                }

                div { class: "field",
                    label { "How it was made" }
                    if p.mode == Mode::Prompt {
                        div { class: "quote", "{p.prompt}" }
                        p { class: "tiny muted",
                            if p.force_instrumental { "Instrumental · " }
                            if !p.generation_mode.is_empty() { "Mode: {p.generation_mode} · " }
                            "Format: {p.output_format}"
                        }
                    }
                    div { class: "sections-list",
                        for (i, c) in chunks.iter().enumerate() {
                            {
                                let color = clip_color(i, c);
                                let lyr = lyric_lines(c.text()).join("\n");
                                let tag = match c {
                                    PlanChunk::Keep(k) => format!("🔒 kept from {}", if k.source.from_label.is_empty() { "earlier take" } else { k.source.from_label.as_str() }),
                                    PlanChunk::Generate(g) => match &g.conditioning {
                                        Some(r) => format!("🎯 similar to {}", if r.from_label.is_empty() { "earlier take" } else { r.from_label.as_str() }),
                                        None => g.positive_styles.iter().take(4).cloned().collect::<Vec<_>>().join(", "),
                                    },
                                };
                                rsx! {
                                    div { key: "{i}", class: "s", style: "--clip: {color}",
                                        div { style: "min-width: 120px",
                                            b { "{clip_name(i, c)}" }
                                            div { class: "tiny muted mono", "{format_duration(c.duration_ms())}" }
                                        }
                                        div {
                                            div { class: "tiny", "{tag}" }
                                            if !lyr.is_empty() { div { class: "lyr", "{lyr}" } }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                div { class: "field",
                    label { "My notes" }
                    textarea {
                        placeholder: "What do you like about this take? What would you change?",
                        value: "{notes}",
                        oninput: move |e| notes.set(e.value()),
                        onblur: move |_| save(UpdateVersionReq { notes: Some(notes()), ..Default::default() }),
                    }
                }
            }
        }
    }
}

#[component]
fn ErrorBanner(err: GenError, on_fix: EventHandler<bool>, on_dismiss: EventHandler<()>) -> Element {
    let has_suggestion = err.prompt_suggestion.is_some() || err.plan_suggestion.is_some();
    rsx! {
        div { class: "banner error",
            div { class: "stack", style: "flex: 1",
                b {
                    "😕 That take didn't work."
                    if !matches!(err.code.as_deref(), Some("disk") | Some("interrupted") | Some("maybe_charged")) { " No budget was used for it." }
                }
                span { "{err.message}" }
                if let Some(s) = &err.prompt_suggestion {
                    div { class: "quote", "💡 {s}" }
                }
                if err.plan_suggestion.is_some() && err.prompt_suggestion.is_none() {
                    span { class: "tiny", "💡 ElevenLabs suggested a different arrangement." }
                }
                div { class: "row",
                    if has_suggestion {
                        button { class: "btn primary small", onclick: move |_| on_fix.call(true), "✨ Use the suggestion" }
                    }
                    button { class: "btn small", onclick: move |_| on_fix.call(false), "✏️ Fix it myself" }
                    button { class: "btn ghost small", onclick: move |_| on_dismiss.call(()), "Dismiss" }
                }
            }
        }
    }
}
