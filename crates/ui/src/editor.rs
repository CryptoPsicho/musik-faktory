//! The composer console: describe a song, or arrange it section by section,
//! then press REC to generate a new take.

use crate::studio::{clip_color, clip_name, Fader, Lane, Lcd, Toggle};
use crate::{api, util, Ctx};
use dioxus::prelude::*;
use musik_shared::*;

/// What the composer starts from.
#[derive(Clone, PartialEq, Debug)]
pub struct EditorSeed {
    pub parent: Option<VersionInfo>,
    pub params: GenParams,
}

pub fn fresh_seed(st: &AppStatus) -> EditorSeed {
    let max_ms = st.max_track_seconds.max(3) * 1000;
    EditorSeed {
        parent: None,
        params: GenParams {
            model_id: if st.default_model.is_empty() { DEFAULT_MODEL.into() } else { st.default_model.clone() },
            store_for_inpainting: st.store_for_inpainting,
            music_length_ms: 30_000.min(max_ms),
            ..GenParams::default()
        },
    }
}

/// Start a new take from an existing one: same settings, and its sections
/// marked with where they are in the old take (so they can be kept).
pub fn seed_from_version(v: &VersionInfo, st: &AppStatus) -> EditorSeed {
    let mut p = v.params.clone();
    let plan = v.plan.clone().unwrap_or_else(|| p.chunks.clone());
    p.chunks = chunks_with_origin(&plan, v.song_id.as_deref(), &v.label);
    let max_ms = st.max_track_seconds.max(3) * 1000;
    p.music_length_ms = p.music_length_ms.min(max_ms);
    EditorSeed { parent: Some(v.clone()), params: p }
}

pub fn seed_from_error(err: &GenError, versions: &[VersionInfo], use_suggestion: bool) -> EditorSeed {
    let mut p = err.params.clone();
    if use_suggestion {
        if let Some(s) = &err.prompt_suggestion {
            p.mode = Mode::Prompt;
            p.prompt = s.clone();
        } else if let Some(c) = &err.plan_suggestion {
            p.mode = Mode::Plan;
            p.chunks = c.clone();
        }
    }
    let parent = err.parent_id.as_ref().and_then(|id| versions.iter().find(|v| &v.id == id)).cloned();
    match &parent {
        // Prompt-mode attempt from a parent: offer the parent's parts again.
        Some(par) if p.chunks.is_empty() => {
            p.chunks = chunks_with_origin(&par.plan.clone().unwrap_or_default(), par.song_id.as_deref(), &par.label);
        }
        Some(_) => {}
        // Parent gone (deleted): nothing can be kept or imitated any more.
        None => {
            p.chunks = p
                .chunks
                .iter()
                .map(|c| match c {
                    PlanChunk::Keep(k) => PlanChunk::Generate(GenChunk {
                        text: if k.text.is_empty() { "[Part]".into() } else { k.text.clone() },
                        duration_ms: k.source.duration_ms().clamp(MIN_CHUNK_MS, MAX_CHUNK_MS),
                        positive_styles: k.styles.clone(),
                        ..GenChunk::default()
                    }),
                    PlanChunk::Generate(g) => PlanChunk::Generate(GenChunk { conditioning: None, origin: None, ..g.clone() }),
                })
                .collect();
        }
    }
    EditorSeed { parent, params: p }
}

const IDEAS: &[&str] = &[
    "A happy pop song about my cat who loves pizza",
    "Calm piano music for doing homework",
    "An epic movie soundtrack for a dragon battle",
    "A funny song about Monday mornings",
    "Chill lo-fi beats with rain sounds",
    "A birthday song for my best friend",
    "A spooky Halloween dance track",
    "A video game theme for a space adventure",
];

const STYLE_SUGGESTIONS: &[&str] = &[
    "pop", "rock", "hip hop", "electronic", "dance", "lo-fi", "jazz", "funk", "disco", "orchestral", "cinematic", "chiptune",
    "reggae", "country", "acoustic", "piano", "acoustic guitar", "electric guitar", "drums", "bass", "synth", "strings",
    "violin", "trumpet", "ukulele", "808s", "choir", "female vocals", "male vocals", "kids choir", "rap", "whispered vocals",
    "happy", "dreamy", "epic", "funny", "calm", "energetic", "spooky", "mysterious", "slow", "fast", "90 BPM", "120 BPM",
    "140 BPM", "big drums", "catchy melody", "build-up", "drop", "fade out", "great production quality",
];

const AVOID_SUGGESTIONS: &[&str] = &["vocals", "drums", "distortion", "screaming", "sad", "slow", "dark", "a cappella", "abrupt ending", "noise"];

fn split_text(text: &str) -> (String, String) {
    let name = section_name(text);
    let t = text.trim_start();
    if t.starts_with('[') {
        if let Some(i) = t.find(']') {
            let rest = &t[i + 1..];
            let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n')).unwrap_or(rest.trim_start_matches(' '));
            return (name, rest.to_string());
        }
    }
    (name, text.to_string())
}

fn join_text(name: &str, body: &str) -> String {
    if name.trim().is_empty() {
        body.to_string()
    } else {
        format!("[{}]\n{}", name.trim(), body)
    }
}

/// Clamp a "sound similar" reference to the API maximum (30 s).
fn conditioning_from(origin: &AudioRange) -> AudioRange {
    let mut r = origin.clone();
    if r.duration_ms() > MAX_CONDITIONING_MS {
        r.end_ms = r.start_ms + MAX_CONDITIONING_MS;
    }
    r
}

#[derive(Clone, Copy, PartialEq)]
enum PartMode {
    Keep,
    Similar,
    New,
}

fn part_mode(c: &PlanChunk) -> PartMode {
    match c {
        PlanChunk::Keep(_) => PartMode::Keep,
        PlanChunk::Generate(g) if g.conditioning.is_some() => PartMode::Similar,
        _ => PartMode::New,
    }
}

fn origin_of(c: &PlanChunk) -> Option<AudioRange> {
    match c {
        PlanChunk::Keep(k) => Some(k.source.clone()),
        PlanChunk::Generate(g) => g.origin.clone(),
    }
}

fn set_part_mode(c: &PlanChunk, mode: PartMode) -> PlanChunk {
    let Some(origin) = origin_of(c) else { return c.clone() };
    match (c, mode) {
        (PlanChunk::Generate(g), PartMode::Keep) => PlanChunk::Keep(KeepChunk {
            source: origin,
            text: g.text.clone(),
            styles: g.positive_styles.clone(),
        }),
        (PlanChunk::Keep(k), PartMode::Similar | PartMode::New) => PlanChunk::Generate(GenChunk {
            text: if k.text.is_empty() { "[Part]".into() } else { k.text.clone() },
            duration_ms: k.source.duration_ms().clamp(MIN_CHUNK_MS, MAX_CHUNK_MS),
            positive_styles: k.styles.clone(),
            conditioning: (mode == PartMode::Similar).then(|| conditioning_from(&origin)),
            origin: Some(origin),
            ..GenChunk::default()
        }),
        (PlanChunk::Generate(g), m) => PlanChunk::Generate(GenChunk {
            conditioning: (m == PartMode::Similar).then(|| conditioning_from(&origin)),
            ..g.clone()
        }),
        (PlanChunk::Keep(_), PartMode::Keep) => c.clone(),
    }
}

#[component]
pub fn Composer(track_id: String, seed: EditorSeed, on_close: EventHandler<()>, on_started: EventHandler<VersionInfo>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut params = use_signal(|| seed.params.clone());
    let mut sel = use_signal(|| if seed.params.chunks.is_empty() { None } else { Some(0usize) });
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<api::ApiErr>);
    let mut nickname = use_signal(String::new);
    let mut bot_text = use_signal(String::new);
    let mut bot_busy = use_signal(|| false);
    let mut bot_msg = use_signal(|| None::<String>);
    // None inside = the listing failed (unknown), not "no sound packs".
    let finetunes = use_resource(|| async { api::finetunes().await.ok() });
    let track_id = use_signal(|| track_id.clone());
    let parent = seed.parent.clone();
    let parent_id = parent.as_ref().map(|p| p.id.clone());
    let parent_label = parent.as_ref().map(|p| p.label.clone());
    let parent_seed = parent.as_ref().and_then(|p| p.params.seed);
    let can_reuse = parent.as_ref().map(|p| p.song_id.is_some()).unwrap_or(false);

    let st = ctx.status();
    let max_ms = st.max_track_seconds.max(3) * 1000;
    let p = params.read().clone();
    let dur = params_duration_ms(&p);
    let est = estimate_cost(dur, st.price_per_minute);
    let problems = validate_params(&p, max_ms);
    let over_budget = st.budget.remaining.map(|r| est > r + 1e-9).unwrap_or(false);
    let blocked = if !st.generation_enabled {
        Some("A parent has paused making new songs.".to_string())
    } else if st.setup_needed {
        Some("A parent needs to set up the app first.".to_string())
    } else if !st.api_key_configured {
        Some("There's no ElevenLabs key yet. Ask a parent.".to_string())
    } else if st.busy {
        Some("Another take is still recording. Wait for it to finish.".to_string())
    } else if over_budget {
        Some(format!(
            "Not enough budget: this costs about {}, and {} is left. Make it shorter or ask a parent.",
            format_money(est, &st.currency),
            format_money(st.budget.remaining.unwrap_or(0.0), &st.currency)
        ))
    } else {
        None
    };
    let can_rec = problems.is_empty() && blocked.is_none() && !busy();
    // A sound pack only works with the model it was trained for. One that no
    // longer fits (e.g. carried over from an older take) is not sent.
    let finetune_ok = |p: &GenParams, list: Option<&Vec<Finetune>>| -> bool {
        p.finetune_id.is_empty()
            || list.map(|l| l.iter().any(|f| f.id == p.finetune_id && (f.model_id.is_empty() || f.model_id == p.model_id))).unwrap_or(true)
    };
    let ft_valid = finetune_ok(&p, finetunes.read().as_ref().and_then(|o| o.as_ref()));

    let record = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        error.set(None);
        let mut send = params();
        if !finetune_ok(&send, finetunes.peek().as_ref().and_then(|o| o.as_ref())) {
            send.finetune_id.clear();
            send.finetune_strength = 1.0;
        }
        let req = GenerateReq { parent_id: parent_id.clone(), params: send, name: nickname() };
        spawn(async move {
            match api::generate(&track_id.peek(), &req).await {
                Ok(v) => on_started.call(v),
                Err(e) => {
                    error.set(Some(e));
                    ctx.refresh_status();
                }
            }
            busy.set(false);
        });
    };

    let mut ask_bot = move |change: bool| {
        if bot_busy() {
            return;
        }
        let text = bot_text().trim().to_string();
        let current = params();
        let text = if text.is_empty() { current.prompt.trim().to_string() } else { text };
        if text.is_empty() {
            bot_msg.set(Some("Tell the producer bot what you want first.".into()));
            return;
        }
        // New arrangement: the length on the bot's fader. Change: keep the current length.
        let total = if change && !current.chunks.is_empty() { total_duration_ms(&current.chunks) } else { current.music_length_ms };
        let req = PlanReq {
            prompt: text,
            music_length_ms: total.clamp(MIN_TOTAL_MS, max_ms),
            source: if change && !current.chunks.is_empty() { Some(current.chunks.clone()) } else { None },
            model_id: current.model_id.clone(),
        };
        bot_busy.set(true);
        bot_msg.set(None);
        spawn(async move {
            match api::plan(&req).await {
                Ok(resp) => {
                    let old = params.peek().chunks.clone();
                    let mut new_chunks = resp.chunks;
                    // When changing an arrangement, a section that kept its name
                    // and length still "is" the old part: keep the link to the
                    // parent's audio (each old part used once). A brand-new
                    // arrangement has no link to the old take.
                    let mut used = vec![false; old.len()];
                    for c in new_chunks.iter_mut() {
                        match c {
                            PlanChunk::Generate(g) => {
                                g.origin = None;
                                g.conditioning = None;
                                if !change {
                                    continue;
                                }
                                let name = section_name(&g.text).to_lowercase();
                                let hit = old.iter().enumerate().position(|(j, prev)| {
                                    !used[j]
                                        && matches!(prev, PlanChunk::Generate(_))
                                        && prev.duration_ms() == g.duration_ms
                                        && prev.section_name().to_lowercase() == name
                                        && origin_of(prev).is_some()
                                });
                                if let Some(j) = hit {
                                    used[j] = true;
                                    g.origin = origin_of(&old[j]);
                                    // A "new, but similar" part stays similar.
                                    if let PlanChunk::Generate(prev) = &old[j] {
                                        g.conditioning = prev.conditioning.clone();
                                        g.condition_strength = prev.condition_strength.clone();
                                    }
                                }
                            }
                            // A kept part that survived the change: restore its
                            // name, lyrics and label (the bot only returns the range).
                            PlanChunk::Keep(k) => {
                                let hit = old.iter().enumerate().position(|(j, prev)| {
                                    !used[j]
                                        && matches!(prev, PlanChunk::Keep(p) if p.source.song_id == k.source.song_id
                                            && p.source.start_ms == k.source.start_ms
                                            && p.source.end_ms == k.source.end_ms)
                                });
                                if let Some(j) = hit {
                                    used[j] = true;
                                    if let PlanChunk::Keep(p) = &old[j] {
                                        k.text = p.text.clone();
                                        k.styles = p.styles.clone();
                                        k.source.from_label = p.source.from_label.clone();
                                    }
                                }
                            }
                        }
                    }
                    let n = new_chunks.len();
                    params.with_mut(|p| {
                        p.chunks = new_chunks;
                        p.mode = Mode::Plan;
                    });
                    sel.set(if n > 0 { Some(0) } else { None });
                    let word = if n == 1 { "section" } else { "sections" };
                    bot_msg.set(Some(format!("🤖 Done! {n} {word}. Click a clip to change it.")));
                }
                Err(e) => {
                    let mut msg = e.message.clone();
                    if let Some(s) = &e.prompt_suggestion {
                        msg.push_str(&format!(" Suggestion: {s}"));
                        bot_text.set(s.clone());
                    }
                    bot_msg.set(Some(msg));
                }
            }
            bot_busy.set(false);
        });
    };

    let mode = p.mode;
    rsx! {
        div { class: "panel composer",
            div { class: "panel-head",
                span { class: "led amber" }
                span { class: "title",
                    match &parent_label {
                        Some(l) => format!("Composer · new take from {l}"),
                        None => "Composer · fresh start".to_string(),
                    }
                }
                span { class: "push", button { class: "btn small ghost", onclick: move |_| on_close.call(()), "✕ Close" } }
            }
            div { class: "panel-body",
                div { class: "row",
                    div { class: "seg-ctl mode",
                        button {
                            class: if mode == Mode::Prompt { "on" } else { "" },
                            onclick: move |_| params.with_mut(|p| p.mode = Mode::Prompt),
                            "💬 Describe it"
                        }
                        button {
                            class: if mode == Mode::Plan { "on" } else { "" },
                            onclick: move |_| {
                                params.with_mut(|p| p.mode = Mode::Plan);
                                if sel().is_none() && !params.peek().chunks.is_empty() {
                                    sel.set(Some(0));
                                }
                            },
                            "🧩 Arrange it"
                        }
                    }
                    span { class: "muted small",
                        if mode == Mode::Prompt {
                            "Say what you want; the AI arranges it."
                        } else {
                            "Build the song part by part, like a real producer."
                        }
                    }
                }

                if mode == Mode::Prompt {
                    DescribeDeck { params, max_ms }
                } else {
                    div { class: "bot",
                        b { "🤖 Producer bot " span { class: "tiny muted", "(free – doesn't use budget)" } }
                        textarea {
                            rows: "2",
                            placeholder: "e.g. “a funky disco song about robots” — or for changes: “make the chorus louder and add a guitar solo”",
                            value: "{bot_text}",
                            oninput: move |e| bot_text.set(e.value()),
                        }
                        div { class: "two",
                            div { class: "field",
                                label { "Length of a new arrangement" }
                                Fader {
                                    value: (p.music_length_ms.min(max_ms) / 1000) as f64,
                                    min: 3.0,
                                    max: (max_ms / 1000) as f64,
                                    step: 1.0,
                                    display: format_duration(p.music_length_ms.min(max_ms)),
                                    on_change: move |v: f64| params.with_mut(|p| p.music_length_ms = (v as u32) * 1000),
                                }
                            }
                        }
                        div { class: "row",
                            button { class: "btn cyan small", disabled: bot_busy(), onclick: move |_| ask_bot(false), "✨ Write a new arrangement" }
                            button {
                                class: "btn small",
                                disabled: bot_busy() || p.chunks.is_empty(),
                                onclick: move |_| ask_bot(true),
                                "🪄 Change my arrangement"
                            }
                            if bot_busy() { div { class: "eq small", for i in 0..5 { i { key: "{i}" } } } }
                        }
                        if let Some(m) = bot_msg() { span { class: "small", "{m}" } }
                    }

                    Lane {
                        chunks: p.chunks.clone(),
                        scale_ms: max_ms,
                        limit_ms: Some(max_ms),
                        selected: sel(),
                        tall: true,
                        on_select: move |i: usize| sel.set(Some(i)),
                        on_add: move |_| {
                            let n = params.peek().chunks.len();
                            let used = total_duration_ms(&params.peek().chunks);
                            let room = max_ms.saturating_sub(used).clamp(MIN_CHUNK_MS, 20_000);
                            params.with_mut(|p| {
                                p.chunks.push(PlanChunk::Generate(GenChunk {
                                    text: if n == 0 { "[Intro]\n".into() } else { "[Verse]\n".into() },
                                    duration_ms: room,
                                    ..GenChunk::default()
                                }))
                            });
                            sel.set(Some(n));
                        },
                    }
                    div { class: "row",
                        span { class: "tiny muted", "Click a clip to edit it. 🔒 = kept from the old take, 🎯 = new but similar." }
                        div { class: "spacer" }
                        if total_duration_ms(&p.chunks) > max_ms {
                            button {
                                class: "btn small danger",
                                onclick: move |_| params.with_mut(|p| fit_chunks(&mut p.chunks, max_ms)),
                                "↔ Squeeze to fit {format_duration(max_ms)}"
                            }
                        }
                    }
                    if let Some(i) = sel().filter(|i| *i < p.chunks.len()) {
                        Inspector { key: "{i}", params, idx: i, sel, max_ms, can_reuse, parent_label: parent_label.clone().unwrap_or_default() }
                    }
                    div { class: "field",
                        label { "Seed (same seed + same settings ≈ similar result)" }
                        div { class: "row",
                            div { class: "seg-ctl",
                                button {
                                    class: if p.seed.is_none() { "on" } else { "" },
                                    onclick: move |_| params.with_mut(|p| p.seed = None),
                                    "🎲 Surprise me"
                                }
                                button {
                                    class: if p.seed.is_some() { "on" } else { "" },
                                    onclick: move |_| params.with_mut(|p| if p.seed.is_none() { p.seed = Some(util::random_u32(MAX_SEED)) }),
                                    "📌 Fixed"
                                }
                            }
                            if let Some(seed) = p.seed {
                                input {
                                    r#type: "number",
                                    min: "0",
                                    max: "{MAX_SEED}",
                                    value: "{seed}",
                                    style: "width: 150px",
                                    oninput: move |e| {
                                        if let Ok(v) = e.value().parse::<u32>() {
                                            params.with_mut(|p| p.seed = Some(v.min(MAX_SEED)))
                                        }
                                    },
                                }
                                button { class: "btn small", title: "New random seed", onclick: move |_| params.with_mut(|p| p.seed = Some(util::random_u32(MAX_SEED))), "🎲" }
                            }
                            if let (Some(ps), Some(l)) = (parent_seed, parent_label.clone()) {
                                if p.seed != Some(ps) {
                                    button { class: "btn small ghost", onclick: move |_| params.with_mut(|p| p.seed = Some(ps)), "Use {l}'s seed" }
                                }
                            }
                        }
                    }
                }

                // ---------------- studio rack (all remaining settings)
                details { class: "rack",
                    summary { "Studio rack · model, quality, sound packs" }
                    div { class: "rack-body",
                        div { class: "field",
                            label { "Model" }
                            select {
                                value: "{p.model_id}",
                                onchange: move |e| {
                                    let model = e.value();
                                    let ft_ok = finetunes
                                        .read()
                                        .as_ref()
                                        .and_then(|o| o.as_ref())
                                        .map(|l| l.iter().any(|f| f.id == params.peek().finetune_id && (f.model_id.is_empty() || f.model_id == model)))
                                        .unwrap_or(true);
                                    params.with_mut(|p| {
                                        p.model_id = model;
                                        // A sound pack only works with the model it was trained for.
                                        if !ft_ok {
                                            p.finetune_id.clear();
                                        }
                                    })
                                },
                                for (id, name) in MODELS.iter() { option { key: "{id}", value: "{id}", selected: p.model_id == *id, "{name}" } }
                            }
                        }
                        div { class: "field",
                            label { "Audio quality" }
                            select {
                                value: "{p.output_format}",
                                onchange: move |e| params.with_mut(|p| p.output_format = e.value()),
                                for (id, name) in OUTPUT_FORMATS.iter() { option { key: "{id}", value: "{id}", selected: p.output_format == *id, "{name}" } }
                            }
                            span { class: "tiny muted", "Some formats need a bigger ElevenLabs plan." }
                        }
                        div { class: "field",
                            label { "Sound pack (finetune)" }
                            select {
                                value: "{p.finetune_id}",
                                onchange: move |e| params.with_mut(|p| p.finetune_id = e.value()),
                                option { value: "", selected: p.finetune_id.is_empty(), "None" }
                                if let Some(Some(list)) = finetunes.read().as_ref() {
                                    for f in list.iter().filter(|f| f.model_id.is_empty() || f.model_id == p.model_id) {
                                        option { key: "{f.id}", value: "{f.id}", selected: p.finetune_id == f.id, "{f.name}" }
                                    }
                                }
                            }
                            if !p.finetune_id.is_empty() && ft_valid {
                                Fader {
                                    value: p.finetune_strength as f64,
                                    min: 0.1, max: 2.0, step: 0.1,
                                    display: format!("{:.1}×", p.finetune_strength),
                                    on_change: move |v: f64| params.with_mut(|p| p.finetune_strength = v as f32),
                                }
                            }
                        }
                        div { class: "field",
                            label { "Extras" }
                            div { class: "stack",
                                Toggle {
                                    on: p.store_for_inpainting,
                                    label: "Remember for editing parts later",
                                    title: "Stores the song at ElevenLabs so later takes can keep (🔒) or imitate (🎯) its parts",
                                    on_change: move |v| params.with_mut(|p| p.store_for_inpainting = v),
                                }
                                Toggle {
                                    on: p.use_phonetic_names,
                                    label: "Pronounce names carefully",
                                    on_change: move |v| params.with_mut(|p| p.use_phonetic_names = v),
                                }
                                Toggle {
                                    on: p.sign_with_c2pa,
                                    label: "Sign as AI-made (C2PA, MP3 only)",
                                    on_change: move |v| params.with_mut(|p| p.sign_with_c2pa = v),
                                }
                            }
                        }
                    }
                }

                // ---------------- output section
                if !problems.is_empty() {
                    ul { class: "problems", for (i, pr) in problems.iter().enumerate() { li { key: "{i}", "{pr}" } } }
                }
                if let Some(b) = &blocked {
                    div { class: "banner warn", "{b}" }
                }
                if let Some(e) = error() {
                    div { class: "banner error",
                        span { "{e.message}" }
                        if let Some(s) = e.prompt_suggestion.clone() {
                            button {
                                class: "btn small",
                                onclick: move |_| params.with_mut(|p| { p.prompt = s.clone(); p.mode = Mode::Prompt; }),
                                "✨ Use: {s}"
                            }
                        }
                    }
                }
                div { class: "out-bar",
                    Lcd { label: "LENGTH", value: format_duration(dur) }
                    Lcd { label: "COST ≈", value: format_money(est, &st.currency), tone: if over_budget { "red" } else { "amber" } }
                    if let Some(r) = st.budget.remaining {
                        Lcd { label: "BUDGET LEFT", value: format_money(r, &st.currency) }
                    }
                    input {
                        r#type: "text",
                        placeholder: "Nickname (optional)",
                        maxlength: "60",
                        style: "flex: 1; min-width: 140px",
                        value: "{nickname}",
                        oninput: move |e| nickname.set(e.value()),
                    }
                    button {
                        class: "rec-btn",
                        disabled: !can_rec,
                        title: "Record a new take",
                        onclick: record,
                        span { class: "dot" }
                        if busy() { "STARTING…" } else { "REC" }
                    }
                }
            }
        }
    }
}

/// Describe mode: prompt, ideas, length fader, instrumental, experimental bits.
#[component]
fn DescribeDeck(params: Signal<GenParams>, max_ms: u32) -> Element {
    let mut params = params;
    let p = params.read().clone();
    let secs = (p.music_length_ms / 1000).min(max_ms / 1000);
    rsx! {
        div { class: "field",
            label { "Describe your song" }
            textarea {
                class: "sheet",
                maxlength: "{MAX_PROMPT_CHARS}",
                placeholder: "What should it sound like? What's it about? Which instruments? Happy or sad?",
                value: "{p.prompt}",
                oninput: move |e| params.with_mut(|p| p.prompt = e.value()),
            }
            div { class: "idea-chips",
                span { class: "tiny muted", "Ideas:" }
                for idea in IDEAS.iter() {
                    button {
                        key: "{idea}",
                        class: "chip-btn",
                        onclick: move |_| params.with_mut(|p| p.prompt = idea.to_string()),
                        "{idea}"
                    }
                }
            }
            span { class: "tiny muted", "Tip: don't name real bands or singers — describe the style instead." }
        }
        div { class: "two",
            div { class: "field",
                label { "Length" }
                Fader {
                    value: secs as f64,
                    min: 3.0,
                    max: (max_ms / 1000) as f64,
                    step: 1.0,
                    display: format_duration(secs * 1000),
                    on_change: move |v: f64| params.with_mut(|p| p.music_length_ms = (v as u32) * 1000),
                }
                span { class: "tiny muted", "Longest allowed: {format_duration(max_ms)}" }
            }
            div { class: "field",
                label { "Voice" }
                div { class: "row",
                    Toggle {
                        on: p.force_instrumental,
                        label: "Instrumental only (no singing)",
                        on_change: move |v| params.with_mut(|p| p.force_instrumental = v),
                    }
                }
            }
        }
        details { class: "rack",
            summary { "Experimental" }
            div { class: "rack-body",
                div { class: "field",
                    label { "Kind of piece" }
                    select {
                        value: "{p.generation_mode}",
                        onchange: move |e| params.with_mut(|p| p.generation_mode = e.value()),
                        for (id, name) in GENERATION_MODES.iter() {
                            option { key: "{id}", value: "{id}", selected: p.generation_mode == *id, "{name}" }
                        }
                    }
                }
                div { class: "field",
                    label { "My own lyrics" }
                    textarea {
                        rows: "4",
                        maxlength: "{MAX_LYRICS_CHARS}",
                        placeholder: "Write the words you want sung (optional)",
                        value: "{p.lyrics_text}",
                        oninput: move |e| params.with_mut(|p| p.lyrics_text = e.value()),
                    }
                }
            }
        }
    }
}

/// Clip inspector: edit one section of the arrangement.
#[component]
fn Inspector(params: Signal<GenParams>, idx: usize, sel: Signal<Option<usize>>, max_ms: u32, can_reuse: bool, parent_label: String) -> Element {
    let mut params = params;
    let Some(chunk) = params.read().chunks.get(idx).cloned() else { return rsx! {} };
    let n = params.read().chunks.len();
    let color = clip_color(idx, &chunk);
    let name = clip_name(idx, &chunk);
    let mode = part_mode(&chunk);
    let has_origin = origin_of(&chunk).is_some() && can_reuse;
    let from = origin_of(&chunk).map(|o| if o.from_label.is_empty() { parent_label.clone() } else { o.from_label }).unwrap_or_default();

    let mut update = move |c: PlanChunk| params.with_mut(|p| p.chunks[idx] = c);
    let mut move_by = move |delta: isize| {
        let j = idx as isize + delta;
        if j < 0 || j as usize >= n {
            return;
        }
        params.with_mut(|p| p.chunks.swap(idx, j as usize));
        let mut sel = sel;
        sel.set(Some(j as usize));
    };

    rsx! {
        div { class: "inspector", style: "--clip: {color}",
            div { class: "inspector-head",
                span { class: "swatch" }
                b { "Section {idx + 1}: {name}" }
                span { class: "muted mono small", "{format_duration(chunk.duration_ms())}" }
                div { class: "spacer" }
                button { class: "btn small", disabled: idx == 0, title: "Move left", onclick: move |_| move_by(-1), "◀" }
                button { class: "btn small", disabled: idx + 1 >= n, title: "Move right", onclick: move |_| move_by(1), "▶" }
                button {
                    class: "btn small",
                    title: "Duplicate",
                    onclick: move |_| {
                        params.with_mut(|p| {
                            let c = p.chunks[idx].clone();
                            p.chunks.insert(idx + 1, c);
                        });
                        let mut sel = sel;
                        sel.set(Some(idx + 1));
                    },
                    "⧉"
                }
                button {
                    class: "btn small danger",
                    title: "Remove section",
                    onclick: move |_| {
                        params.with_mut(|p| {
                            p.chunks.remove(idx);
                        });
                        let mut sel = sel;
                        sel.set(if n > 1 { Some(idx.saturating_sub(1)) } else { None });
                    },
                    "🗑"
                }
            }

            if has_origin {
                div { class: "seg-ctl",
                    button {
                        class: if mode == PartMode::Keep { "on" } else { "" },
                        title: "Copy this part exactly from {from}",
                        onclick: move |_| { let c = set_part_mode(&params.peek().chunks[idx], PartMode::Keep); update(c) },
                        "🔒 Keep from {from}"
                    }
                    button {
                        class: if mode == PartMode::Similar { "on" } else { "" },
                        title: "Make it new, but sounding like {from}",
                        onclick: move |_| { let c = set_part_mode(&params.peek().chunks[idx], PartMode::Similar); update(c) },
                        "🎯 New, but similar"
                    }
                    button {
                        class: if mode == PartMode::New { "on" } else { "" },
                        onclick: move |_| { let c = set_part_mode(&params.peek().chunks[idx], PartMode::New); update(c) },
                        "✏️ Brand new"
                    }
                }
            }

            match chunk.clone() {
                PlanChunk::Keep(k) => rsx! {
                    p { class: "muted", "This part is copied exactly from {from} ({format_duration(k.source.start_ms)}–{format_duration(k.source.end_ms)})." }
                    if !k.text.is_empty() { div { class: "quote", "{k.text}" } }
                },
                PlanChunk::Generate(g) => {
                    let (sname, body) = split_text(&g.text);
                    let max_chunk = MAX_CHUNK_MS.min(max_ms).max(MIN_CHUNK_MS);
                    let g1 = g.clone();
                    let g2 = g.clone();
                    let g3 = g.clone();
                    let g4 = g.clone();
                    let g5 = g.clone();
                    let g6 = g.clone();
                    let g7 = g.clone();
                    rsx! {
                        div { class: "two",
                            div { class: "field",
                                label { "Section name" }
                                input {
                                    r#type: "text",
                                    maxlength: "{MAX_SECTION_NAME}",
                                    placeholder: "Verse, Chorus, Bridge, Drop…",
                                    value: "{sname}",
                                    oninput: {
                                        let body = body.clone();
                                        move |e: FormEvent| update(PlanChunk::Generate(GenChunk { text: join_text(&e.value(), &body), ..g1.clone() }))
                                    },
                                }
                            }
                            div { class: "field",
                                label { "Length" }
                                Fader {
                                    value: (g.duration_ms / 1000) as f64,
                                    min: 3.0,
                                    max: (max_chunk / 1000) as f64,
                                    step: 1.0,
                                    display: format_duration(g.duration_ms),
                                    on_change: move |v: f64| update(PlanChunk::Generate(GenChunk { duration_ms: (v as u32) * 1000, ..g2.clone() })),
                                }
                            }
                        }
                        div { class: "field",
                            label { "Lyrics & cues" }
                            textarea {
                                class: "sheet",
                                placeholder: "One line per sung line. {{guitar solo}} in curly braces for cues, (ooh) for sounds. Leave empty for no singing.",
                                value: "{body}",
                                oninput: move |e: FormEvent| update(PlanChunk::Generate(GenChunk { text: join_text(&sname, &e.value()), ..g3.clone() })),
                            }
                        }
                        div { class: "two",
                            div { class: "field",
                                label { "Sounds I want" }
                                TagInput {
                                    tags: g.positive_styles.clone(),
                                    suggestions: STYLE_SUGGESTIONS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                                    placeholder: "add a sound + Enter",
                                    negative: false,
                                    on_change: move |t: Vec<String>| update(PlanChunk::Generate(GenChunk { positive_styles: t, ..g4.clone() })),
                                }
                                if idx == 0 { span { class: "tiny muted", "The first section sets the mood for the whole song: add 6–7 sounds here." } }
                            }
                            div { class: "field",
                                label { "Sounds I don't want" }
                                TagInput {
                                    tags: g.negative_styles.clone(),
                                    suggestions: AVOID_SUGGESTIONS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                                    placeholder: "avoid… + Enter",
                                    negative: true,
                                    on_change: move |t: Vec<String>| update(PlanChunk::Generate(GenChunk { negative_styles: t, ..g5.clone() })),
                                }
                            }
                        }
                        div { class: "two",
                            div { class: "field",
                                label { "Fit with the other parts" }
                                select {
                                    value: "{g.context_adherence}",
                                    onchange: move |e| update(PlanChunk::Generate(GenChunk { context_adherence: e.value(), ..g6.clone() })),
                                    for (id, label) in ADHERENCE.iter() {
                                        option { key: "{id}", value: "{id}", selected: g.context_adherence == *id, "{label}" }
                                    }
                                }
                            }
                            if g.conditioning.is_some() {
                                div { class: "field",
                                    label { "How similar to {from}" }
                                    select {
                                        value: "{g.condition_strength}",
                                        onchange: move |e| update(PlanChunk::Generate(GenChunk { condition_strength: e.value(), ..g7.clone() })),
                                        for (id, label) in CONDITION_STRENGTHS.iter() {
                                            option { key: "{id}", value: "{id}", selected: g.condition_strength == *id, "{label}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Chips with ×, a text box (Enter or comma adds) and clickable suggestions.
fn add_tag(tags: &[String], t: &str, on_change: &EventHandler<Vec<String>>) {
    let t = t.trim().trim_matches(',').trim().to_string();
    if t.is_empty() || tags.iter().any(|x| x.eq_ignore_ascii_case(&t)) || tags.len() >= MAX_STYLES {
        return;
    }
    let mut v = tags.to_vec();
    v.push(t);
    on_change.call(v);
}

#[component]
fn TagInput(tags: Vec<String>, suggestions: Vec<String>, placeholder: String, negative: bool, on_change: EventHandler<Vec<String>>) -> Element {
    let mut draft = use_signal(String::new);
    let free: Vec<String> = suggestions.into_iter().filter(|s| !tags.iter().any(|t| t.eq_ignore_ascii_case(s))).collect();
    let tags_in = tags.clone();
    let tags_key = tags.clone();
    rsx! {
        div { class: "tags",
            for (i, t) in tags.iter().enumerate() {
                {
                    let tags_rm = tags.clone();
                    rsx! {
                        span { key: "{i}-{t}", class: if negative { "tag neg" } else { "tag" },
                            "{t}"
                            button {
                                title: "Remove",
                                onclick: move |_| {
                                    let mut v = tags_rm.clone();
                                    v.remove(i);
                                    on_change.call(v);
                                },
                                "×"
                            }
                        }
                    }
                }
            }
            input {
                class: "tag-input",
                placeholder: "{placeholder}",
                value: "{draft}",
                oninput: move |e| {
                    let v = e.value();
                    if v.ends_with(',') {
                        add_tag(&tags_in, &v, &on_change);
                        draft.set(String::new());
                    } else {
                        draft.set(v);
                    }
                },
                onkeydown: move |e| {
                    if e.key() == Key::Enter {
                        add_tag(&tags_key, &draft(), &on_change);
                        draft.set(String::new());
                    }
                },
            }
        }
        div { class: "suggest",
            for s in free.into_iter() {
                {
                    let tags_s = tags.clone();
                    let s2 = s.clone();
                    rsx! { button { key: "{s}", class: "chip-btn", onclick: move |_| add_tag(&tags_s, &s2, &on_change), "+ {s}" } }
                }
            }
        }
    }
}
