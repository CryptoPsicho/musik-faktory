//! Generation parameters and the composition-plan model.
//!
//! The app always works with the chunk-based plan format of `music_v2` /
//! `music_v2_5`. Plans coming back from ElevenLabs in the older section-based
//! format are converted on the way in (see [`chunks_from_el_plan`]).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Models offered in the UI. `music_v1` is deprecated by ElevenLabs
/// (removal announced for 2027-01-31) and its plan format differs, so it is
/// not offered.
pub const MODELS: &[(&str, &str)] = &[
    ("music_v2_5", "Music v2.5 (best)"),
    ("music_v2", "Music v2"),
];

pub const DEFAULT_MODEL: &str = "music_v2_5";

/// Output formats accepted by `POST /v1/music`. μ-law / A-law telephony
/// formats are left out on purpose: they make no sense for music.
pub const OUTPUT_FORMATS: &[(&str, &str)] = &[
    ("auto", "Automatic (recommended)"),
    ("mp3_48000_192", "MP3 48 kHz · 192 kbps"),
    ("mp3_48000_128", "MP3 48 kHz · 128 kbps"),
    ("mp3_48000_240", "MP3 48 kHz · 240 kbps"),
    ("mp3_48000_320", "MP3 48 kHz · 320 kbps"),
    ("mp3_44100_192", "MP3 44.1 kHz · 192 kbps"),
    ("mp3_44100_128", "MP3 44.1 kHz · 128 kbps"),
    ("mp3_44100_96", "MP3 44.1 kHz · 96 kbps"),
    ("mp3_44100_64", "MP3 44.1 kHz · 64 kbps"),
    ("mp3_44100_32", "MP3 44.1 kHz · 32 kbps"),
    ("mp3_24000_48", "MP3 24 kHz · 48 kbps"),
    ("mp3_22050_32", "MP3 22 kHz · 32 kbps"),
    ("opus_48000_192", "Opus 48 kHz · 192 kbps"),
    ("opus_48000_128", "Opus 48 kHz · 128 kbps"),
    ("opus_48000_96", "Opus 48 kHz · 96 kbps"),
    ("opus_48000_64", "Opus 48 kHz · 64 kbps"),
    ("opus_48000_32", "Opus 48 kHz · 32 kbps"),
    ("pcm_48000", "WAV (PCM) 48 kHz"),
    ("pcm_44100", "WAV (PCM) 44.1 kHz"),
    ("pcm_32000", "WAV (PCM) 32 kHz"),
    ("pcm_24000", "WAV (PCM) 24 kHz"),
    ("pcm_22050", "WAV (PCM) 22 kHz"),
    ("pcm_16000", "WAV (PCM) 16 kHz"),
];

/// `generation_mode` hint for prompt-based generation. ElevenLabs lists it in
/// the OpenAPI spec but hides it from the official SDKs, so the UI labels it
/// as experimental.
pub const GENERATION_MODES: &[(&str, &str)] = &[
    ("", "Let the AI decide"),
    ("track", "Full track"),
    ("loop", "Loop"),
    ("ambience", "Ambience / background"),
];

pub const ADHERENCE: &[(&str, &str)] = &[
    ("high", "Stick close to the rest"),
    ("medium", "A bit of freedom"),
    ("low", "Go wild"),
];

pub const CONDITION_STRENGTHS: &[(&str, &str)] = &[
    ("low", "A little similar"),
    ("medium", "Similar"),
    ("high", "Very similar"),
    ("xhigh", "Almost the same"),
];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Describe the song with words; ElevenLabs does the rest.
    #[default]
    Prompt,
    /// Build the song section by section (composition plan).
    Plan,
}

/// Every knob of a single generation. A version stores the exact parameters
/// it was generated with, so it can be reproduced or used as a starting point.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct GenParams {
    pub mode: Mode,
    pub model_id: String,
    pub output_format: String,

    // ---- prompt mode ----
    pub prompt: String,
    /// Always sent explicitly: we never let the model pick the length.
    pub music_length_ms: u32,
    pub force_instrumental: bool,
    /// Experimental: "", "track", "loop", "ambience".
    pub generation_mode: String,
    /// Experimental: lyrics to use together with the prompt.
    pub lyrics_text: String,

    // ---- plan mode ----
    pub chunks: Vec<PlanChunk>,
    /// Only valid with a composition plan. `None` = the server picks a random
    /// seed and records it, so a version can always be re-rolled the same way.
    pub seed: Option<u32>,

    // ---- both modes ----
    pub finetune_id: String,
    pub finetune_strength: f32,
    pub use_phonetic_names: bool,
    pub sign_with_c2pa: bool,
    /// Keep the song on ElevenLabs so its sections can be reused later
    /// ("keep this part, redo that part").
    pub store_for_inpainting: bool,
}

impl Default for GenParams {
    fn default() -> Self {
        Self {
            mode: Mode::Prompt,
            model_id: DEFAULT_MODEL.into(),
            output_format: "auto".into(),
            prompt: String::new(),
            music_length_ms: 30_000,
            force_instrumental: false,
            generation_mode: String::new(),
            lyrics_text: String::new(),
            chunks: Vec::new(),
            seed: None,
            finetune_id: String::new(),
            finetune_strength: 1.0,
            use_phonetic_names: false,
            sign_with_c2pa: false,
            store_for_inpainting: true,
        }
    }
}

/// A slice of a song stored on ElevenLabs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct AudioRange {
    pub song_id: String,
    pub start_ms: u32,
    pub end_ms: u32,
    /// Display only: the version label the slice comes from ("v0").
    pub from_label: String,
}

impl AudioRange {
    pub fn duration_ms(&self) -> u32 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// One section of a song in a composition plan.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanChunk {
    /// Generate new audio for this section.
    Generate(GenChunk),
    /// Copy this section unchanged from an earlier version (inpainting).
    Keep(KeepChunk),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct GenChunk {
    /// `[Section name]` on the first line, then lyrics, `{inline cues}`.
    pub text: String,
    pub duration_ms: u32,
    pub positive_styles: Vec<String>,
    pub negative_styles: Vec<String>,
    /// "low" | "medium" | "high"
    pub context_adherence: String,
    /// Sound similar to this slice of an earlier version.
    pub conditioning: Option<AudioRange>,
    /// "low" | "medium" | "high" | "xhigh"
    pub condition_strength: String,
    /// UI helper: where this section sits in the parent version, so it can
    /// be switched to "keep" or "similar". Not sent to ElevenLabs.
    pub origin: Option<AudioRange>,
}

impl Default for GenChunk {
    fn default() -> Self {
        Self {
            text: "[Verse]\n".into(),
            duration_ms: 15_000,
            positive_styles: Vec::new(),
            negative_styles: Vec::new(),
            context_adherence: "high".into(),
            conditioning: None,
            condition_strength: "high".into(),
            origin: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct KeepChunk {
    pub source: AudioRange,
    /// Display only: what that part contained (section name + lyrics).
    pub text: String,
    /// Display only: kept so the section can be switched back to "generate".
    pub styles: Vec<String>,
}

impl PlanChunk {
    pub fn duration_ms(&self) -> u32 {
        match self {
            PlanChunk::Generate(g) => g.duration_ms,
            PlanChunk::Keep(k) => k.source.duration_ms(),
        }
    }

    /// The text shown for this chunk (section name + lyrics).
    pub fn text(&self) -> &str {
        match self {
            PlanChunk::Generate(g) => &g.text,
            PlanChunk::Keep(k) => &k.text,
        }
    }

    /// Section name from a leading `[Name]` line, if any.
    pub fn section_name(&self) -> String {
        section_name(self.text())
    }
}

pub fn section_name(text: &str) -> String {
    let first = text.trim_start().lines().next().unwrap_or("").trim();
    if first.starts_with('[') {
        if let Some(end) = first.find(']') {
            return first[1..end].trim().to_string();
        }
    }
    String::new()
}

/// Lyrics lines of a chunk text (everything except the `[Section]` header).
pub fn lyric_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if let Some(first) = lines.first() {
        let t = first.trim();
        if t.starts_with('[') && t.contains(']') {
            let rest = t[t.find(']').unwrap() + 1..].trim().to_string();
            if rest.is_empty() {
                lines.remove(0);
            } else {
                lines[0] = rest;
            }
        }
    }
    lines.into_iter().filter(|l| !l.trim().is_empty()).collect()
}

pub fn total_duration_ms(chunks: &[PlanChunk]) -> u32 {
    // Saturating: a huge (bogus) chunk must never wrap around to a small total.
    chunks.iter().fold(0u32, |acc, c| acc.saturating_add(c.duration_ms()))
}

/// Effective length of the song these parameters will produce.
pub fn params_duration_ms(p: &GenParams) -> u32 {
    match p.mode {
        Mode::Prompt => p.music_length_ms,
        Mode::Plan => total_duration_ms(&p.chunks),
    }
}

fn el_range(r: &AudioRange) -> Value {
    json!({ "start_ms": r.start_ms, "end_ms": r.end_ms })
}

/// Convert our chunks into the ElevenLabs `composition_plan` JSON.
pub fn chunks_to_el_plan(chunks: &[PlanChunk]) -> Value {
    let list: Vec<Value> = chunks
        .iter()
        .map(|c| match c {
            PlanChunk::Generate(g) => {
                let mut v = json!({
                    "text": g.text,
                    "duration_ms": g.duration_ms,
                    "positive_styles": g.positive_styles,
                    "negative_styles": g.negative_styles,
                    "context_adherence": if g.context_adherence.is_empty() { "high" } else { g.context_adherence.as_str() },
                });
                if let Some(r) = &g.conditioning {
                    v["conditioning_ref"] = json!({ "song_id": r.song_id, "range": el_range(r) });
                    let strength = if g.condition_strength.is_empty() { "high" } else { g.condition_strength.as_str() };
                    v["condition_strength"] = json!(strength);
                }
                v
            }
            PlanChunk::Keep(k) => json!({ "song_id": k.source.song_id, "range": el_range(&k.source) }),
        })
        .collect();
    json!({ "chunks": list })
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default()
}

fn as_u32(v: Option<&Value>) -> u32 {
    v.and_then(|v| v.as_u64()).unwrap_or(0).min(u32::MAX as u64) as u32
}

fn parse_range(song_id: &str, r: Option<&Value>) -> AudioRange {
    AudioRange {
        song_id: song_id.to_string(),
        start_ms: as_u32(r.and_then(|r| r.get("start_ms"))),
        end_ms: as_u32(r.and_then(|r| r.get("end_ms"))),
        from_label: String::new(),
    }
}

/// Parse a composition plan returned by ElevenLabs (chunk format, or the
/// older section format) into our chunk model.
pub fn chunks_from_el_plan(plan: &Value) -> Option<Vec<PlanChunk>> {
    if let Some(chunks) = plan.get("chunks").and_then(|c| c.as_array()) {
        let mut out = Vec::new();
        for c in chunks {
            if let (Some(song_id), true) = (c.get("song_id").and_then(|s| s.as_str()), c.get("text").is_none()) {
                out.push(PlanChunk::Keep(KeepChunk {
                    source: parse_range(song_id, c.get("range")),
                    text: String::new(),
                    styles: Vec::new(),
                }));
                continue;
            }
            let conditioning = c.get("conditioning_ref").and_then(|r| {
                r.get("song_id").and_then(|s| s.as_str()).map(|sid| parse_range(sid, r.get("range")))
            });
            out.push(PlanChunk::Generate(GenChunk {
                text: c.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
                duration_ms: as_u32(c.get("duration_ms")),
                positive_styles: str_list(c.get("positive_styles")),
                negative_styles: str_list(c.get("negative_styles")),
                context_adherence: c
                    .get("context_adherence")
                    .and_then(|s| s.as_str())
                    .unwrap_or("high")
                    .to_string(),
                conditioning,
                condition_strength: c
                    .get("condition_strength")
                    .and_then(|s| s.as_str())
                    .unwrap_or("high")
                    .to_string(),
                origin: None,
            }));
        }
        return Some(out);
    }
    if let Some(sections) = plan.get("sections").and_then(|s| s.as_array()) {
        let global_pos = str_list(plan.get("positive_global_styles"));
        let global_neg = str_list(plan.get("negative_global_styles"));
        let mut out = Vec::new();
        for (i, s) in sections.iter().enumerate() {
            let name = s.get("section_name").and_then(|n| n.as_str()).unwrap_or("Section");
            let mut text = format!("[{name}]");
            for line in str_list(s.get("lines")) {
                text.push('\n');
                text.push_str(&line);
            }
            let mut pos = if i == 0 { global_pos.clone() } else { Vec::new() };
            pos.extend(str_list(s.get("positive_local_styles")));
            let mut neg = global_neg.clone();
            neg.extend(str_list(s.get("negative_local_styles")));
            neg.dedup();
            out.push(PlanChunk::Generate(GenChunk {
                text,
                duration_ms: as_u32(s.get("duration_ms")),
                positive_styles: pos,
                negative_styles: neg,
                ..GenChunk::default()
            }));
        }
        return Some(out);
    }
    None
}

/// Mark every chunk of a parent's plan with where it sits in the parent's
/// audio, so the editor can offer "keep this part" / "sound similar".
pub fn chunks_with_origin(chunks: &[PlanChunk], song_id: Option<&str>, label: &str) -> Vec<PlanChunk> {
    let mut pos = 0u32;
    chunks
        .iter()
        .map(|c| {
            let dur = c.duration_ms();
            let origin = song_id.map(|sid| AudioRange {
                song_id: sid.to_string(),
                start_ms: pos,
                end_ms: pos + dur,
                from_label: label.to_string(),
            });
            pos += dur;
            match c {
                PlanChunk::Generate(g) => PlanChunk::Generate(GenChunk {
                    origin,
                    conditioning: None,
                    ..g.clone()
                }),
                // A "keep" part of the parent is, from the child's point of
                // view, just a section of the parent's audio.
                PlanChunk::Keep(k) => PlanChunk::Generate(GenChunk {
                    text: if k.text.is_empty() { "[Kept part]".into() } else { k.text.clone() },
                    duration_ms: dur,
                    positive_styles: k.styles.clone(),
                    origin,
                    ..GenChunk::default()
                }),
            }
        })
        .collect()
}

/// Scale chunk durations so the plan fits into `max_ms` (each generated
/// chunk keeps at least 3 s). Kept chunks are left untouched.
pub fn fit_chunks(chunks: &mut [PlanChunk], max_ms: u32) {
    let total = total_duration_ms(chunks);
    if total <= max_ms {
        return;
    }
    let fixed: u32 = chunks
        .iter()
        .filter_map(|c| match c {
            PlanChunk::Keep(k) => Some(k.source.duration_ms()),
            _ => None,
        })
        .sum();
    let flexible = total - fixed;
    if flexible == 0 || max_ms <= fixed {
        return;
    }
    let ratio = (max_ms - fixed) as f64 / flexible as f64;
    for c in chunks.iter_mut() {
        if let PlanChunk::Generate(g) = c {
            let scaled = ((g.duration_ms as f64 * ratio) / 500.0).floor() * 500.0;
            g.duration_ms = (scaled as u32).max(crate::rules::MIN_CHUNK_MS);
        }
    }
}
