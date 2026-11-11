//! Studio-style building blocks: LCD read-outs, LED meters, the arrangement
//! lane with clips, toggles and faders.

use dioxus::prelude::*;
use musik_shared::*;

/// Clip colours (DAW style), picked by section position.
const CLIP_COLORS: &[&str] = &["#7c3aed", "#0891b2", "#db2777", "#d97706", "#059669", "#2563eb", "#c026d3", "#65a30d"];

pub fn clip_color(i: usize, chunk: &PlanChunk) -> String {
    match chunk {
        PlanChunk::Keep(_) => "#3b4157".into(),
        PlanChunk::Generate(_) => CLIP_COLORS[i % CLIP_COLORS.len()].into(),
    }
}

pub fn clip_name(i: usize, chunk: &PlanChunk) -> String {
    let n = chunk.section_name();
    if n.is_empty() {
        format!("Part {}", i + 1)
    } else {
        n
    }
}

fn hash(s: &str) -> u32 {
    s.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

/// Decorative waveform (deterministic from a seed string).
#[component]
pub fn Wave(seed: String, bars: usize) -> Element {
    let mut h = hash(&seed) | 1;
    let heights: Vec<u32> = (0..bars)
        .map(|i| {
            h ^= h << 13;
            h ^= h >> 17;
            h ^= h << 5;
            let env = ((i as f32 / bars.max(1) as f32) * std::f32::consts::PI).sin() * 0.35 + 0.65;
            ((20 + h % 80) as f32 * env) as u32
        })
        .collect();
    rsx! {
        div { class: "wave",
            for (i, ht) in heights.into_iter().enumerate() {
                i { key: "{i}", style: "height: {ht}%" }
            }
        }
    }
}

/// LCD read-out with a small label.
#[component]
pub fn Lcd(label: String, value: String, #[props(default)] tone: String, #[props(default)] big: bool) -> Element {
    let class = format!("lcd {tone} {}", if big { "big" } else { "" });
    rsx! {
        div { class: "{class}",
            div { class: "lcd-cell",
                span { class: "lbl", "{label}" }
                span { class: "val", "{value}" }
            }
        }
    }
}

/// VU-style LED meter. `fraction` 0..1 of segments lit; colours go
/// green → amber → red along the meter (or the reverse for "remaining").
#[component]
pub fn Meter(fraction: f64, #[props(default = 16)] segments: usize, #[props(default)] wide: bool, #[props(default)] remaining: bool) -> Element {
    let lit = (fraction.clamp(0.0, 1.0) * segments as f64).round() as usize;
    rsx! {
        div { class: if wide { "meter wide" } else { "meter" },
            for i in 0..segments {
                {
                    let pos = i as f64 / segments as f64;
                    // For "remaining" meters a short bar is the dangerous one.
                    let tone = if remaining {
                        if lit <= segments / 6 { "r" } else if lit <= segments / 3 { "y" } else { "g" }
                    } else if pos >= 0.85 { "r" } else if pos >= 0.6 { "y" } else { "g" };
                    let on = if i < lit { "on" } else { "" };
                    rsx! { div { key: "{i}", class: "seg {on} {tone}" } }
                }
            }
        }
    }
}

/// Button-style toggle with an LED.
#[component]
pub fn Toggle(on: bool, label: String, on_change: EventHandler<bool>, #[props(default)] title: String) -> Element {
    rsx! {
        button {
            class: if on { "toggle on" } else { "toggle" },
            title: "{title}",
            r#type: "button",
            onclick: move |_| on_change.call(!on),
            span { class: "led" }
            "{label}"
        }
    }
}

/// Horizontal fader with an LCD value.
#[component]
pub fn Fader(value: f64, min: f64, max: f64, step: f64, display: String, on_change: EventHandler<f64>) -> Element {
    rsx! {
        div { class: "fader",
            input {
                r#type: "range",
                min: "{min}",
                max: "{max}",
                step: "{step}",
                value: "{value}",
                oninput: move |e| {
                    if let Ok(v) = e.value().parse::<f64>() {
                        on_change.call(v)
                    }
                },
            }
            Lcd { label: "", value: display }
        }
    }
}

/// Arrangement lane: sections as clips, proportional to their length.
///
/// * `scale_ms` – what 100 % width means (≥ total).
/// * `limit_ms` – show the parent's limit marker when it's inside the lane.
/// * `selected` / `on_select` – make clips clickable (editor).
/// * `on_add` – show a "+" clip at the end.
/// * `playhead` – 0..1 position of the player.
/// * `on_seek` – clicking anywhere seeks (0..1).
#[component]
pub fn Lane(
    chunks: Vec<PlanChunk>,
    #[props(default)] scale_ms: u32,
    #[props(default)] limit_ms: Option<u32>,
    #[props(default)] selected: Option<usize>,
    #[props(default)] on_select: Option<EventHandler<usize>>,
    #[props(default)] on_add: Option<EventHandler<()>>,
    #[props(default)] playhead: Option<f64>,
    #[props(default)] on_seek: Option<EventHandler<f64>>,
    #[props(default)] tall: bool,
) -> Element {
    let total = total_duration_ms(&chunks);
    let scale = scale_ms.max(total).max(1);
    let mut seek_el = use_signal(|| None::<web_sys::Element>);
    let ticks = 5;
    rsx! {
        div { class: "lane-wrap",
            div { class: "ruler",
                for t in 0..=ticks {
                    span { key: "{t}", "{format_duration(scale / ticks * t)}" }
                }
            }
            div { class: if tall { "lane tall" } else { "lane" },
                if chunks.is_empty() {
                    div { class: "clip empty", "No sections yet" }
                }
                for (i, c) in chunks.iter().enumerate() {
                    {
                        let dur = c.duration_ms();
                        let width = dur as f64 / scale as f64 * 100.0;
                        let kind = match c {
                            PlanChunk::Keep(_) => " keep",
                            PlanChunk::Generate(g) if g.conditioning.is_some() => " similar",
                            _ => "",
                        };
                        let sel = if selected == Some(i) { " sel" } else { "" };
                        let click = if on_select.is_some() { " click" } else { "" };
                        let icon = match c {
                            PlanChunk::Keep(_) => "🔒 ",
                            PlanChunk::Generate(g) if g.conditioning.is_some() => "🎯 ",
                            _ => "",
                        };
                        let color = clip_color(i, c);
                        let name = clip_name(i, c);
                        let seed = format!("{i}{}", c.text());
                        rsx! {
                            div {
                                key: "{i}",
                                class: "clip{kind}{sel}{click}",
                                style: "flex: 0 0 calc({width:.3}% - 3px); --clip: {color}",
                                title: "{name} · {format_duration(dur)}",
                                onclick: move |_| {
                                    if let Some(h) = on_select {
                                        h.call(i)
                                    }
                                },
                                div { class: "cname", "{icon}{name}" }
                                Wave { seed, bars: 18 }
                                div { class: "cdur", "{format_duration(dur)}" }
                            }
                        }
                    }
                }
                if let Some(add) = on_add {
                    div { class: "clip add", title: "Add a section", onclick: move |_| add.call(()), "＋" }
                }
                if let Some(limit) = limit_ms {
                    if limit < scale {
                        div { class: "limit-marker", style: "left: {limit as f64 / scale as f64 * 100.0:.3}%",
                            span { "MAX {format_duration(limit)}" }
                        }
                        div { class: "over-zone", style: "left: {limit as f64 / scale as f64 * 100.0:.3}%" }
                    }
                }
                if let Some(p) = playhead {
                    div { class: "playhead", style: "left: {p.clamp(0.0, 1.0) * total as f64 / scale as f64 * 100.0:.3}%" }
                }
                if let Some(seek) = on_seek {
                    div {
                        class: "seek",
                        title: "Click to jump here",
                        onmounted: move |e| {
                            use dioxus::web::WebEventExt;
                            seek_el.set(Some(e.data().as_web_event()));
                        },
                        onclick: move |e| {
                            let x = e.element_coordinates().x;
                            let w = seek_el.read().as_ref().map(|el| el.client_width() as f64).unwrap_or(0.0);
                            if w > 0.0 {
                                let frac = (x / w) * scale as f64 / total.max(1) as f64;
                                seek.call(frac.clamp(0.0, 1.0));
                            }
                        },
                    }
                }
            }
        }
    }
}
