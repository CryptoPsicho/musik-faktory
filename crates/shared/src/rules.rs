//! Validation rules (mirroring the ElevenLabs API limits) and cost estimates.

use crate::plan::*;

pub const MIN_CHUNK_MS: u32 = 3_000;
pub const MAX_CHUNK_MS: u32 = 120_000;
pub const MIN_TOTAL_MS: u32 = 3_000;
/// Hard ceiling of the API (10 minutes). The parent's limit is usually far lower.
pub const API_MAX_TOTAL_MS: u32 = 600_000;
pub const MAX_CHUNKS: usize = 30;
pub const MAX_STYLES: usize = 50;
pub const MAX_CHUNK_TEXT: usize = 6_132;
pub const MAX_LINES: usize = 30;
pub const MAX_LINE_CHARS: usize = 200;
pub const MAX_PROMPT_CHARS: usize = 4_100;
pub const MAX_LYRICS_CHARS: usize = 4_000;
pub const MAX_CONDITIONING_MS: u32 = 30_000;
pub const MAX_SEED: u32 = 2_147_483_647;
pub const MAX_SECTION_NAME: usize = 100;

/// Estimated price of `duration_ms` of music.
pub fn estimate_cost(duration_ms: u32, price_per_minute: f64) -> f64 {
    duration_ms as f64 / 60_000.0 * price_per_minute
}

/// Estimated credits for `duration_ms` of music.
pub fn estimate_credits(duration_ms: u32, credits_per_minute: f64) -> f64 {
    duration_ms as f64 / 60_000.0 * credits_per_minute
}

pub fn format_money(amount: f64, currency: &str) -> String {
    let a = if amount.abs() < 0.005 { 0.0 } else { amount };
    if currency.chars().count() <= 1 {
        format!("{currency}{a:.2}")
    } else {
        format!("{a:.2} {currency}")
    }
}

pub fn format_duration(ms: u32) -> String {
    let s = ms.saturating_add(500) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Check generation parameters. Returns a list of kid-readable problems
/// (empty = fine). `max_total_ms` is the parent's track length limit.
pub fn validate_params(p: &GenParams, max_total_ms: u32) -> Vec<String> {
    let mut errs = Vec::new();
    let max_total_ms = max_total_ms.min(API_MAX_TOTAL_MS);

    if !MODELS.iter().any(|(id, _)| *id == p.model_id) {
        errs.push(format!("Unknown model \"{}\".", p.model_id));
    }
    if !OUTPUT_FORMATS.iter().any(|(id, _)| *id == p.output_format) {
        errs.push(format!("Unknown audio format \"{}\".", p.output_format));
    }
    if !p.finetune_id.is_empty() && !(p.finetune_strength > 0.0 && p.finetune_strength <= 2.0) {
        errs.push("The sound-style strength must be between 0.1 and 2.".into());
    }

    let total = params_duration_ms(p);
    if total < MIN_TOTAL_MS {
        errs.push("The song must be at least 3 seconds long.".into());
    }
    if total > max_total_ms {
        errs.push(format!(
            "The song is {} long, but the maximum allowed is {}.",
            format_duration(total),
            format_duration(max_total_ms)
        ));
    }

    match p.mode {
        Mode::Prompt => {
            if p.prompt.trim().is_empty() {
                errs.push("Describe your song first.".into());
            }
            if p.prompt.chars().count() > MAX_PROMPT_CHARS {
                errs.push(format!("The description is too long (max {MAX_PROMPT_CHARS} characters)."));
            }
            if p.lyrics_text.chars().count() > MAX_LYRICS_CHARS {
                errs.push(format!("The lyrics are too long (max {MAX_LYRICS_CHARS} characters)."));
            }
            if !GENERATION_MODES.iter().any(|(id, _)| *id == p.generation_mode) {
                errs.push("Unknown generation mode.".into());
            }
        }
        Mode::Plan => {
            if p.chunks.is_empty() {
                errs.push("Add at least one section.".into());
            }
            if p.chunks.len() > MAX_CHUNKS {
                errs.push(format!("Too many sections (max {MAX_CHUNKS})."));
            }
            if let Some(seed) = p.seed {
                if seed > MAX_SEED {
                    errs.push("The seed number is too big.".into());
                }
            }
            for (i, c) in p.chunks.iter().enumerate() {
                let n = i + 1;
                match c {
                    PlanChunk::Generate(g) => {
                        if g.duration_ms < MIN_CHUNK_MS || g.duration_ms > MAX_CHUNK_MS {
                            errs.push(format!("Section {n}: length must be between 3 s and 2 min."));
                        }
                        if g.text.chars().count() > MAX_CHUNK_TEXT {
                            errs.push(format!("Section {n}: text is too long."));
                        }
                        if g.text.trim_start().starts_with('[') {
                            let name_len = section_name(&g.text).chars().count();
                            if name_len == 0 || name_len > MAX_SECTION_NAME {
                                errs.push(format!("Section {n}: the name must be 1 to {MAX_SECTION_NAME} characters."));
                            }
                        }
                        let lines = lyric_lines(&g.text);
                        if lines.len() > MAX_LINES {
                            errs.push(format!("Section {n}: max {MAX_LINES} lyric lines."));
                        }
                        if lines.iter().any(|l| l.chars().count() > MAX_LINE_CHARS) {
                            errs.push(format!("Section {n}: a lyric line is longer than {MAX_LINE_CHARS} characters."));
                        }
                        if g.positive_styles.len() > MAX_STYLES || g.negative_styles.len() > MAX_STYLES {
                            errs.push(format!("Section {n}: max {MAX_STYLES} styles."));
                        }
                        if !ADHERENCE.iter().any(|(id, _)| *id == g.context_adherence) {
                            errs.push(format!("Section {n}: unknown adherence."));
                        }
                        if let Some(r) = &g.conditioning {
                            if r.song_id.is_empty() || r.end_ms <= r.start_ms {
                                errs.push(format!("Section {n}: the \"sound similar\" reference is broken."));
                            }
                            if r.duration_ms() > MAX_CONDITIONING_MS {
                                errs.push(format!("Section {n}: \"sound similar\" can use at most 30 s of the old version."));
                            }
                            if !CONDITION_STRENGTHS.iter().any(|(id, _)| *id == g.condition_strength) {
                                errs.push(format!("Section {n}: unknown similarity strength."));
                            }
                        }
                    }
                    PlanChunk::Keep(k) => {
                        if k.source.song_id.is_empty() || k.source.end_ms <= k.source.start_ms {
                            errs.push(format!("Section {n}: the kept part is broken."));
                        } else if k.source.duration_ms() > MAX_CHUNK_MS {
                            errs.push(format!("Section {n}: a kept part can be at most 2 min long."));
                        }
                    }
                }
            }
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn gen(ms: u32) -> PlanChunk {
        PlanChunk::Generate(GenChunk { duration_ms: ms, ..Default::default() })
    }

    #[test]
    fn prompt_length_is_capped_by_parent_limit() {
        let mut p = GenParams { prompt: "happy song".into(), music_length_ms: 61_000, ..Default::default() };
        assert!(!validate_params(&p, 60_000).is_empty());
        p.music_length_ms = 60_000;
        assert!(validate_params(&p, 60_000).is_empty());
    }

    #[test]
    fn plan_total_includes_kept_parts() {
        let p = GenParams {
            mode: Mode::Plan,
            chunks: vec![
                gen(20_000),
                PlanChunk::Keep(KeepChunk {
                    source: AudioRange { song_id: "s".into(), start_ms: 0, end_ms: 45_000, ..Default::default() },
                    ..Default::default()
                }),
            ],
            ..Default::default()
        };
        assert_eq!(params_duration_ms(&p), 65_000);
        assert!(!validate_params(&p, 60_000).is_empty());
        assert!(validate_params(&p, 90_000).is_empty());
    }

    #[test]
    fn fit_chunks_respects_limit() {
        let mut c = vec![gen(40_000), gen(40_000), gen(40_000)];
        fit_chunks(&mut c, 60_000);
        assert!(total_duration_ms(&c) <= 60_000);
        assert!(c.iter().all(|c| c.duration_ms() >= MIN_CHUNK_MS));
    }

    #[test]
    fn el_plan_roundtrip_keeps_structure() {
        let chunks = vec![
            PlanChunk::Generate(GenChunk {
                text: "[Chorus]\nLa la".into(),
                duration_ms: 10_000,
                positive_styles: vec!["pop".into()],
                conditioning: Some(AudioRange { song_id: "abc".into(), start_ms: 0, end_ms: 10_000, ..Default::default() }),
                ..Default::default()
            }),
            PlanChunk::Keep(KeepChunk {
                source: AudioRange { song_id: "abc".into(), start_ms: 10_000, end_ms: 20_000, ..Default::default() },
                ..Default::default()
            }),
        ];
        let el = chunks_to_el_plan(&chunks);
        assert_eq!(el["chunks"][1], json!({"song_id": "abc", "range": {"start_ms": 10000, "end_ms": 20000}}));
        assert_eq!(el["chunks"][0]["conditioning_ref"]["song_id"], "abc");
        let back = chunks_from_el_plan(&el).unwrap();
        assert_eq!(back.len(), 2);
        assert!(matches!(back[1], PlanChunk::Keep(_)));
        assert_eq!(back[0].section_name(), "Chorus");
    }

    #[test]
    fn v1_section_plan_is_converted() {
        let v1 = json!({
            "positive_global_styles": ["pop"], "negative_global_styles": ["metal"],
            "sections": [{"section_name": "Verse 1", "positive_local_styles": ["soft"], "negative_local_styles": [],
                          "duration_ms": 10000, "lines": ["Hello", "World"]}]
        });
        let c = chunks_from_el_plan(&v1).unwrap();
        match &c[0] {
            PlanChunk::Generate(g) => {
                assert_eq!(g.text, "[Verse 1]\nHello\nWorld");
                assert_eq!(g.positive_styles, vec!["pop", "soft"]);
                assert_eq!(g.negative_styles, vec!["metal"]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn origin_ranges_are_cumulative() {
        let c = chunks_with_origin(&[gen(10_000), gen(5_000)], Some("sid"), "v0");
        match (&c[0], &c[1]) {
            (PlanChunk::Generate(a), PlanChunk::Generate(b)) => {
                assert_eq!(a.origin.as_ref().unwrap().end_ms, 10_000);
                assert_eq!(b.origin.as_ref().unwrap().start_ms, 10_000);
                assert_eq!(b.origin.as_ref().unwrap().end_ms, 15_000);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn huge_kept_part_cannot_wrap_the_total() {
        let keep = |ms: u32| {
            PlanChunk::Keep(KeepChunk {
                source: AudioRange { song_id: "s".into(), start_ms: 0, end_ms: ms, ..Default::default() },
                ..Default::default()
            })
        };
        let p = GenParams { mode: Mode::Plan, chunks: vec![keep(u32::MAX), keep(u32::MAX), gen(10_000)], ..Default::default() };
        assert_eq!(params_duration_ms(&p), u32::MAX);
        assert!(!validate_params(&p, 60_000).is_empty());
    }

    #[test]
    fn section_names_are_limited() {
        let long = format!("[{}]\nla", "x".repeat(101));
        let p = GenParams {
            mode: Mode::Plan,
            chunks: vec![PlanChunk::Generate(GenChunk { text: long, duration_ms: 10_000, ..Default::default() })],
            ..Default::default()
        };
        assert!(validate_params(&p, 60_000).iter().any(|e| e.contains("name")));
    }

    #[test]
    fn lyric_lines_skip_header() {
        assert_eq!(lyric_lines("[Verse] hey\nyou\n\n"), vec!["hey", "you"]);
        assert_eq!(section_name("  [Bridge]\nla"), "Bridge");
    }
}
