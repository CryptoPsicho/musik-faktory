//! ElevenLabs Music API client, plus a mock backend for testing without
//! spending credits.

use musik_shared::*;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::Duration;

pub const BASE_URL: &str = "https://api.elevenlabs.io";

pub struct ComposeOutput {
    pub audio: Vec<u8>,
    pub ext: String,
    pub content_type: String,
    pub song_id: Option<String>,
    pub plan: Option<Value>,
    pub metadata: Option<SongMetadata>,
}

#[derive(Debug, Clone)]
pub struct ElError {
    pub http_status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
    pub prompt_suggestion: Option<String>,
    pub plan_suggestion: Option<Value>,
    /// The request may have reached ElevenLabs and been billed (timeouts,
    /// dropped connections, 5xx, broken 200 responses). 4xx = refused = free.
    pub maybe_charged: bool,
}

impl ElError {
    fn other(msg: impl Into<String>) -> Self {
        ElError { http_status: None, code: None, message: msg.into(), prompt_suggestion: None, plan_suggestion: None, maybe_charged: false }
    }

    fn charged(mut self) -> Self {
        self.maybe_charged = true;
        self
    }

    /// ElevenLabs says the key itself is wrong (as opposed to "valid key, but
    /// not allowed to read account info", which is fine for Music-only keys).
    pub fn is_invalid_key(&self) -> bool {
        self.code.as_deref() == Some("invalid_api_key")
            || (self.http_status == Some(401) && self.message.to_lowercase().contains("invalid api key"))
    }
}

impl std::fmt::Display for ElError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub struct Subscription {
    pub tier: String,
    pub status: String,
    pub credits_used: i64,
    pub credits_limit: i64,
    pub next_reset_unix: Option<i64>,
    pub currency: Option<String>,
}

pub enum Backend {
    Real { http: reqwest::Client, base: String },
    Mock(Mutex<MockState>),
}

#[derive(Default)]
pub struct MockState {
    pub credits_used: i64,
    pub delay_ms: u64,
}

impl Backend {
    pub fn real() -> Self {
        let base = std::env::var("ELEVENLABS_BASE_URL").unwrap_or_else(|_| BASE_URL.to_string());
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            // Long songs can take a few minutes to compose.
            .timeout(Duration::from_secs(15 * 60))
            .user_agent(concat!("musik-faktory/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Backend::Real { http, base }
    }

    pub fn mock(delay_ms: u64) -> Self {
        Backend::Mock(Mutex::new(MockState { credits_used: 0, delay_ms }))
    }

    pub fn is_mock(&self) -> bool {
        matches!(self, Backend::Mock(_))
    }

    pub async fn compose(&self, key: &str, p: &GenParams) -> Result<ComposeOutput, ElError> {
        match self {
            Backend::Real { http, base } => compose_real(http, base, key, p).await,
            Backend::Mock(state) => mock::compose(state, p).await,
        }
    }

    pub async fn plan(&self, key: &str, req: &PlanReq) -> Result<Value, ElError> {
        match self {
            Backend::Real { http, base } => {
                let mut body = json!({
                    "prompt": req.prompt,
                    "music_length_ms": req.music_length_ms,
                    "model_id": req.model_id,
                });
                if let Some(src) = &req.source {
                    body["source_composition_plan"] = chunks_to_el_plan(src);
                }
                let resp = http
                    .post(format!("{base}/v1/music/plan"))
                    .header("xi-api-key", key)
                    .json(&body)
                    .send()
                    .await
                    .map_err(net_err)?;
                if !resp.status().is_success() {
                    return Err(api_error(resp).await);
                }
                resp.json::<Value>().await.map_err(net_err)
            }
            Backend::Mock(_) => Ok(mock::plan(req)),
        }
    }

    pub async fn subscription(&self, key: &str) -> Result<Subscription, ElError> {
        match self {
            Backend::Real { http, base } => {
                let resp = http
                    .get(format!("{base}/v1/user/subscription"))
                    .header("xi-api-key", key)
                    .timeout(Duration::from_secs(20))
                    .send()
                    .await
                    .map_err(net_err)?;
                if !resp.status().is_success() {
                    return Err(api_error(resp).await);
                }
                let v: Value = resp.json().await.map_err(net_err)?;
                Ok(Subscription {
                    tier: v["tier"].as_str().unwrap_or("?").to_string(),
                    status: v["status"].as_str().unwrap_or("").to_string(),
                    credits_used: v["character_count"].as_i64().unwrap_or(0),
                    credits_limit: v["character_limit"].as_i64().unwrap_or(0),
                    next_reset_unix: v["next_character_count_reset_unix"].as_i64(),
                    currency: v["currency"].as_str().map(|s| s.to_string()),
                })
            }
            Backend::Mock(state) => {
                let s = state.lock().unwrap();
                Ok(Subscription {
                    tier: "mock".into(),
                    status: "active".into(),
                    credits_used: s.credits_used,
                    credits_limit: 100_000,
                    next_reset_unix: None,
                    currency: Some("usd".into()),
                })
            }
        }
    }

    pub async fn finetunes(&self, key: &str) -> Result<Vec<Finetune>, ElError> {
        match self {
            Backend::Real { http, base } => {
                let mut out = Vec::new();
                let mut cursor: Option<String> = None;
                // Follow the pages (a handful at most).
                for _ in 0..5 {
                    let mut q = vec![("page_size", "150".to_string()), ("sort", "name".into()), ("sort_direction", "asc".into())];
                    if let Some(c) = &cursor {
                        q.push(("cursor", c.clone()));
                    }
                    let resp = http
                        .get(format!("{base}/v1/music/finetunes"))
                        .query(&q)
                        .header("xi-api-key", key)
                        .timeout(Duration::from_secs(20))
                        .send()
                        .await
                        .map_err(net_err)?;
                    if !resp.status().is_success() {
                        return Err(api_error(resp).await);
                    }
                    let v: Value = resp.json().await.map_err(net_err)?;
                    for f in v["finetunes"].as_array().into_iter().flatten() {
                        let ready = f["status"].as_str().map(|s| s == "ready" || s == "completed" || s == "trained").unwrap_or(true);
                        if let (true, Some(id)) = (ready, f["id"].as_str()) {
                            out.push(Finetune {
                                id: id.to_string(),
                                name: f["name"].as_str().unwrap_or("?").to_string(),
                                primary_genre: f["primary_genre"].as_str().unwrap_or("").to_string(),
                                model_id: f["model_id"].as_str().unwrap_or("").to_string(),
                            });
                        }
                    }
                    cursor = v["next_cursor"].as_str().map(|s| s.to_string());
                    if !v["has_more"].as_bool().unwrap_or(false) || cursor.is_none() {
                        break;
                    }
                }
                Ok(out)
            }
            Backend::Mock(_) => Ok(vec![
                Finetune { id: "mock-ft-1".into(), name: "Mock 80s Nu-Disco".into(), primary_genre: "disco".into(), model_id: "music_v2_5".into() },
                Finetune { id: "mock-ft-2".into(), name: "Mock Lo-fi Study".into(), primary_genre: "lo-fi".into(), model_id: "music_v2".into() },
            ]),
        }
    }
}

fn net_err(e: reqwest::Error) -> ElError {
    let err = if e.is_timeout() {
        ElError::other("ElevenLabs took too long to answer. Please try again.")
    } else {
        ElError::other(format!("Could not reach ElevenLabs ({e}). Is the internet connection working?"))
    };
    // Only a failed connect or a request that could not even be built (bad
    // key characters, bad URL) proves nothing reached ElevenLabs.
    if e.is_connect() || e.is_builder() {
        err
    } else {
        err.charged()
    }
}

/// Build the JSON body for `POST /v1/music/detailed`.
pub fn compose_body(p: &GenParams) -> Value {
    let mut b = json!({ "model_id": p.model_id });
    match p.mode {
        Mode::Prompt => {
            b["prompt"] = json!(p.prompt);
            // Always explicit: the model's own choice tends to be longer.
            b["music_length_ms"] = json!(p.music_length_ms);
            if p.force_instrumental {
                b["force_instrumental"] = json!(true);
            }
            if !p.generation_mode.is_empty() {
                b["generation_mode"] = json!(p.generation_mode);
            }
            if !p.lyrics_text.trim().is_empty() {
                b["lyrics_text"] = json!(p.lyrics_text);
            }
        }
        Mode::Plan => {
            b["composition_plan"] = chunks_to_el_plan(&p.chunks);
            if let Some(seed) = p.seed {
                b["seed"] = json!(seed);
            }
        }
    }
    if !p.finetune_id.is_empty() {
        b["finetune_id"] = json!(p.finetune_id);
        b["finetune_strength"] = json!(p.finetune_strength);
    }
    if p.use_phonetic_names {
        b["use_phonetic_names"] = json!(true);
    }
    if p.sign_with_c2pa {
        b["sign_with_c2pa"] = json!(true);
    }
    if p.store_for_inpainting {
        b["store_for_inpainting"] = json!(true);
    }
    b
}

async fn compose_real(http: &reqwest::Client, base: &str, key: &str, p: &GenParams) -> Result<ComposeOutput, ElError> {
    let resp = http
        .post(format!("{base}/v1/music/detailed"))
        .query(&[("output_format", p.output_format.as_str())])
        .header("xi-api-key", key)
        .json(&compose_body(p))
        .send()
        .await
        .map_err(net_err)?;
    if !resp.status().is_success() {
        return Err(api_error(resp).await);
    }
    let song_id = resp.headers().get("song-id").and_then(|h| h.to_str().ok()).map(|s| s.to_string());
    let ctype = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .to_string();
    // From here on ElevenLabs has made (and billed) the song.
    let body = resp.bytes().await.map_err(|e| net_err(e).charged())?;

    let mut out = ComposeOutput {
        audio: Vec::new(),
        ext: String::new(),
        content_type: String::new(),
        song_id: if p.store_for_inpainting { song_id } else { None },
        plan: None,
        metadata: None,
    };
    let mut audio_ct = String::new();
    let mut filename = String::new();
    if ctype.starts_with("multipart/") {
        let boundary = boundary_of(&ctype).ok_or_else(|| ElError::other("Unexpected answer from ElevenLabs (no boundary).").charged())?;
        for part in parse_multipart(&body, &boundary) {
            let pct = part.header("content-type").unwrap_or_default();
            if pct.contains("json") {
                if let Ok(v) = serde_json::from_slice::<Value>(&part.body) {
                    out.plan = v.get("composition_plan").cloned().filter(|p| !p.is_null());
                    out.metadata = v.get("song_metadata").and_then(|m| serde_json::from_value(m.clone()).ok());
                }
            } else {
                audio_ct = pct;
                filename = part
                    .header("content-disposition")
                    .and_then(|d| d.split("filename=").nth(1).map(|f| f.trim_matches(|c| c == '"' || c == ';' || c == ' ').to_string()))
                    .unwrap_or_default();
                out.audio = part.body;
            }
        }
    } else {
        // Plain audio body (should not happen for /detailed, but be lenient).
        audio_ct = ctype;
        out.audio = body.to_vec();
    }
    if out.audio.is_empty() {
        return Err(ElError::other("ElevenLabs answered without any audio, so no song could be saved.").charged());
    }
    let (audio, ext, ct) = finalize_audio(out.audio, &p.output_format, &audio_ct, &filename);
    out.audio = audio;
    out.ext = ext;
    out.content_type = ct;
    Ok(out)
}

/// Pick a file extension / content type and wrap raw PCM into a WAV file so
/// the browser can play it.
pub fn finalize_audio(audio: Vec<u8>, output_format: &str, content_type: &str, filename: &str) -> (Vec<u8>, String, String) {
    if let Some(rate) = output_format.strip_prefix("pcm_").and_then(|r| r.parse::<u32>().ok()) {
        if !audio.starts_with(b"RIFF") {
            return (wav_from_pcm16(&audio, rate), "wav".into(), "audio/wav".into());
        }
        return (audio, "wav".into(), "audio/wav".into());
    }
    let from_name = filename.rsplit('.').next().filter(|e| e.len() <= 4 && *e != filename).map(|e| e.to_lowercase());
    let ext = from_name.unwrap_or_else(|| {
        if output_format.starts_with("opus") || content_type.contains("ogg") || content_type.contains("opus") {
            "ogg".into()
        } else if content_type.contains("wav") {
            "wav".into()
        } else {
            "mp3".into()
        }
    });
    let ct = match ext.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        _ => "application/octet-stream",
    };
    (audio, ext, ct.to_string())
}

pub fn wav_from_pcm16(pcm: &[u8], rate: u32) -> Vec<u8> {
    let channels: u16 = 1;
    let bits: u16 = 16;
    let byte_rate = rate * channels as u32 * bits as u32 / 8;
    let mut w = Vec::with_capacity(44 + pcm.len());
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&byte_rate.to_le_bytes());
    w.extend_from_slice(&(channels * bits / 8).to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(pcm);
    w
}

fn boundary_of(content_type: &str) -> Option<String> {
    content_type.split(';').map(|s| s.trim()).find_map(|s| {
        s.strip_prefix("boundary=").map(|b| b.trim_matches('"').to_string())
    })
}

pub struct Part {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Part {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// Minimal multipart/mixed parser (the response is small: one JSON part and
/// one audio part).
pub fn parse_multipart(body: &[u8], boundary: &str) -> Vec<Part> {
    let delim = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let Some(mut pos) = find(body, &delim, 0) else { return parts };
    loop {
        pos += delim.len();
        if body.get(pos..pos + 2) == Some(b"--") {
            break; // closing delimiter
        }
        let Some(next) = find(body, &delim, pos) else { break };
        let mut seg = &body[pos..next];
        if seg.starts_with(b"\r\n") {
            seg = &seg[2..];
        } else if seg.starts_with(b"\n") {
            seg = &seg[1..];
        }
        if seg.ends_with(b"\r\n") {
            seg = &seg[..seg.len() - 2];
        } else if seg.ends_with(b"\n") {
            seg = &seg[..seg.len() - 1];
        }
        let (head, content) = match find(seg, b"\r\n\r\n", 0) {
            Some(i) => (&seg[..i], &seg[i + 4..]),
            None => match find(seg, b"\n\n", 0) {
                Some(i) => (&seg[..i], &seg[i + 2..]),
                None => (&seg[..0], seg),
            },
        };
        let headers = String::from_utf8_lossy(head)
            .lines()
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
            .collect();
        parts.push(Part { headers, body: content.to_vec() });
        pos = next;
    }
    parts
}

/// Turn an ElevenLabs error response into a kid-readable error.
async fn api_error(resp: reqwest::Response) -> ElError {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    error_from_body(status, &v, &text)
}

pub fn error_from_body(status: u16, v: &Value, raw: &str) -> ElError {
    let detail = v.get("detail").cloned().unwrap_or(Value::Null);
    let code = detail.get("status").and_then(|s| s.as_str()).map(|s| s.to_string());
    let api_msg = detail
        .get("message")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
        .or_else(|| detail.as_str().map(|s| s.to_string()))
        .or_else(|| {
            detail.as_array().map(|errs| {
                errs.iter()
                    .filter_map(|e| e.get("msg").and_then(|m| m.as_str()))
                    .collect::<Vec<_>>()
                    .join("; ")
            })
        })
        .unwrap_or_else(|| raw.chars().take(300).collect());
    let data = detail.get("data").cloned().unwrap_or(Value::Null);
    let prompt_suggestion = data.get("prompt_suggestion").and_then(|s| s.as_str()).map(|s| s.to_string());
    let plan_suggestion = data.get("composition_plan_suggestion").cloned().filter(|p| !p.is_null());

    let has_suggestion = prompt_suggestion.is_some() || plan_suggestion.is_some();
    let tail = if has_suggestion { " Try the suggestion below!" } else { " Try describing it differently." };
    let message = match (status, code.as_deref()) {
        (_, Some("bad_prompt")) => {
            format!("ElevenLabs can't make this one. Songs can't copy real artists, bands or song lyrics.{tail}")
        }
        (_, Some("bad_composition_plan")) => {
            format!("ElevenLabs can't use this plan. Styles and lyrics can't copy real artists or songs.{tail}")
        }
        (_, Some("quota_exceeded")) => "The ElevenLabs account has run out of credits. Please ask a parent.".to_string(),
        (401, _) => format!("The ElevenLabs key was refused. Please ask a parent to check it. ({api_msg})"),
        (403, _) => format!("The ElevenLabs key is not allowed to do this. Please ask a parent. ({api_msg})"),
        (429, _) => "ElevenLabs is busy right now. Wait a minute and try again.".to_string(),
        (422, _) => format!("ElevenLabs didn't accept these settings: {api_msg}"),
        _ => format!("ElevenLabs error {status}: {api_msg}"),
    };
    ElError { http_status: Some(status), code, message, prompt_suggestion, plan_suggestion, maybe_charged: status >= 500 }
}

/// Test hook for mock mode: a prompt containing this marker simulates
/// "ElevenLabs made and billed the song, but the answer got lost".
pub const MOCK_GLITCH: &str = "__mock_glitch__";

/// Fake backend: makes simple synthesized tunes so the whole app can be
/// tried without an API key or spending credits.
mod mock {
    use super::*;

    fn hash(s: &str) -> u64 {
        // FNV-1a
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    fn words(prompt: &str, n: usize) -> Vec<String> {
        prompt
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 3)
            .take(n)
            .map(|w| w.to_lowercase())
            .collect()
    }

    pub fn plan(req: &PlanReq) -> Value {
        if let Some(src) = &req.source {
            let extra = words(&req.prompt, 3);
            let chunks: Vec<PlanChunk> = src
                .iter()
                .map(|c| match c {
                    PlanChunk::Generate(g) => {
                        let mut g = g.clone();
                        g.positive_styles.extend(extra.iter().cloned());
                        g.positive_styles.dedup();
                        PlanChunk::Generate(g)
                    }
                    k => k.clone(),
                })
                .collect();
            return chunks_to_el_plan(&chunks);
        }
        let total = req.music_length_ms.clamp(MIN_TOTAL_MS, API_MAX_TOTAL_MS);
        let n = ((total / 12_000).max(1) as usize).min(5);
        let names = ["Intro", "Verse", "Chorus", "Verse 2", "Outro"];
        let names: Vec<&str> = match n {
            1 => vec!["Chorus"],
            2 => vec!["Verse", "Chorus"],
            3 => vec!["Intro", "Verse", "Chorus"],
            4 => vec!["Intro", "Verse", "Chorus", "Outro"],
            _ => names.to_vec(),
        };
        let each = ((total / n as u32) / 500 * 500).max(MIN_CHUNK_MS);
        let mut styles = words(&req.prompt, 6);
        styles.push("mock mode".into());
        let chunks: Vec<PlanChunk> = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let dur = if i == n - 1 { total.saturating_sub(each * (n as u32 - 1)).max(MIN_CHUNK_MS) } else { each };
                let lyrics = if *name == "Intro" || *name == "Outro" { "" } else { "\nLa la la, this is a pretend song\nMade by the mock robot all day long" };
                PlanChunk::Generate(GenChunk {
                    text: format!("[{name}]{lyrics}"),
                    duration_ms: dur,
                    positive_styles: if i == 0 { styles.clone() } else { vec![format!("{} energy", if *name == "Chorus" { "big" } else { "calm" })] },
                    ..GenChunk::default()
                })
            })
            .collect();
        chunks_to_el_plan(&chunks)
    }

    fn mentions_copyright(p: &GenParams) -> bool {
        let mut all = p.prompt.to_lowercase();
        for c in &p.chunks {
            if let PlanChunk::Generate(g) = c {
                all.push_str(&g.text.to_lowercase());
                all.push_str(&g.positive_styles.join(" ").to_lowercase());
            }
        }
        all.contains("copyright")
    }

    pub async fn compose(state: &Mutex<MockState>, p: &GenParams) -> Result<ComposeOutput, ElError> {
        let delay = state.lock().unwrap().delay_ms;
        tokio::time::sleep(Duration::from_millis(delay)).await;
        if p.prompt.contains(MOCK_GLITCH) {
            // Simulates "ElevenLabs made and billed the song, but the answer got lost".
            state.lock().unwrap().credits_used += (params_duration_ms(p) as f64 / 60_000.0 * 900.0).round() as i64;
            return Err(ElError::other("Pretend network glitch after the song was made.").charged());
        }
        if mentions_copyright(p) {
            return Err(error_from_body(
                400,
                &json!({"detail": {"status": if p.mode == Mode::Plan { "bad_composition_plan" } else { "bad_prompt" },
                    "message": "copyrighted material",
                    "data": {"prompt_suggestion": "A cheerful original pop song with bright synths and handclaps",
                             "composition_plan_suggestion": plan(&PlanReq { prompt: "cheerful original pop".into(), music_length_ms: params_duration_ms(p), source: None, model_id: p.model_id.clone() })}}}),
                "",
            ));
        }
        let chunks: Vec<PlanChunk> = match p.mode {
            Mode::Plan => p.chunks.clone(),
            Mode::Prompt => {
                let v = plan(&PlanReq { prompt: p.prompt.clone(), music_length_ms: p.music_length_ms, source: None, model_id: p.model_id.clone() });
                chunks_from_el_plan(&v).unwrap_or_default()
            }
        };
        let seed = p.seed.unwrap_or(0) as u64;
        let sections: Vec<(u32, u64)> = chunks
            .iter()
            .map(|c| (c.duration_ms(), hash(c.text()) ^ hash(&p.prompt) ^ seed))
            .collect();
        let wav = synth(&sections, p.force_instrumental);
        let total = total_duration_ms(&chunks);
        {
            let mut s = state.lock().unwrap();
            s.credits_used += (total as f64 / 60_000.0 * 900.0).round() as i64;
        }
        let title = {
            let w = words(&p.prompt, 3);
            if w.is_empty() { "Mock Song".to_string() } else { format!("Mock: {}", w.join(" ")) }
        };
        Ok(ComposeOutput {
            audio: wav,
            ext: "wav".into(),
            content_type: "audio/wav".into(),
            song_id: p.store_for_inpainting.then(|| format!("mock-{}", crate::store::new_id())),
            plan: Some(chunks_to_el_plan(&chunks)),
            metadata: Some(SongMetadata {
                title: Some(title),
                description: Some("A pretend song made in mock mode (no credits used).".into()),
                genres: vec!["mock".into()],
                languages: vec!["en".into()],
                is_explicit: Some(false),
            }),
        })
    }

    /// Tiny synthesizer: a pentatonic melody + bass per section.
    fn synth(sections: &[(u32, u64)], instrumental: bool) -> Vec<u8> {
        let rate = 22_050u32;
        let mut pcm: Vec<u8> = Vec::new();
        let scale = [0, 2, 4, 7, 9, 12, 14, 16];
        for (si, (dur_ms, h)) in sections.iter().enumerate() {
            let samples = (*dur_ms as u64 * rate as u64 / 1000) as usize;
            let mut rng = *h | 1;
            let root = 50 + (h % 12) as i32; // MIDI note
            let beat = (rate as f64 * (0.18 + (h >> 8) as f64 % 7.0 * 0.02)) as usize;
            let mut note_freq = 0.0f64;
            let mut phase = 0.0f64;
            let mut bass_phase = 0.0f64;
            for i in 0..samples {
                if i % beat == 0 {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    let deg = scale[(rng % scale.len() as u64) as usize];
                    let midi = root + 12 + deg + if si % 2 == 1 { 5 } else { 0 };
                    note_freq = 440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0);
                }
                let t_in_note = (i % beat) as f64 / rate as f64;
                let env = (-(t_in_note) * 6.0).exp() * (t_in_note * 200.0).min(1.0);
                phase += note_freq / rate as f64;
                let bass_f = 440.0 * 2f64.powf((root as f64 - 12.0 - 69.0) / 12.0);
                bass_phase += bass_f / rate as f64;
                let lead = (phase * std::f64::consts::TAU).sin() + if instrumental { 0.0 } else { 0.3 * (phase * 2.0 * std::f64::consts::TAU).sin() };
                let bass = if (bass_phase % 1.0) < 0.5 { 0.15 } else { -0.15 };
                let fade = {
                    let edge = (rate as f64 * 0.02) as usize;
                    let a = (i.min(samples - 1 - i.min(samples - 1)) as f64 / edge as f64).min(1.0);
                    a.max(0.0)
                };
                let s = ((lead * env * 0.35 + bass * 0.6) * fade).clamp(-1.0, 1.0);
                pcm.extend_from_slice(&((s * 32_000.0) as i16).to_le_bytes());
            }
        }
        wav_from_pcm16(&pcm, rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_is_split_into_json_and_audio() {
        let body = b"--abc123\r\nContent-Type: application/json\r\n\r\n{\"song_metadata\":{\"title\":\"Hi\",\"genres\":[]}}\r\n--abc123\r\nContent-Type: audio/mpeg\r\nContent-Disposition: attachment; filename=\"song.mp3\"\r\n\r\nID3\x00\x01binary\r\n--abc123--\r\n";
        let parts = parse_multipart(body, "abc123");
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].header("content-type").unwrap(), "application/json");
        assert_eq!(parts[1].body, b"ID3\x00\x01binary");
        assert_eq!(boundary_of("multipart/mixed; boundary=\"abc123\"").unwrap(), "abc123");
    }

    #[test]
    fn bad_prompt_error_keeps_suggestion() {
        let v = json!({"detail": {"status": "bad_prompt", "message": "x", "data": {"prompt_suggestion": "An epic rock ballad"}}});
        let e = error_from_body(400, &v, "");
        assert_eq!(e.code.as_deref(), Some("bad_prompt"));
        assert_eq!(e.prompt_suggestion.as_deref(), Some("An epic rock ballad"));
        assert!(e.message.contains("suggestion"));
        assert!(!e.maybe_charged);
        let none = error_from_body(400, &json!({"detail": {"status": "bad_prompt", "message": "harmful"}}), "");
        assert!(!none.message.contains("suggestion"));
        assert!(error_from_body(502, &Value::Null, "bad gateway").maybe_charged);
    }

    #[test]
    fn prompt_body_always_has_length_and_no_seed() {
        let p = GenParams { prompt: "x".into(), music_length_ms: 20_000, seed: Some(5), ..Default::default() };
        let b = compose_body(&p);
        assert_eq!(b["music_length_ms"], 20_000);
        assert!(b.get("seed").is_none());
        assert!(b.get("composition_plan").is_none());
    }

    #[test]
    fn pcm_is_wrapped_as_wav() {
        let (a, ext, ct) = finalize_audio(vec![0, 0, 1, 0], "pcm_44100", "audio/pcm", "");
        assert!(a.starts_with(b"RIFF"));
        assert_eq!((ext.as_str(), ct.as_str()), ("wav", "audio/wav"));
        let (_, ext, _) = finalize_audio(vec![1], "auto", "audio/mpeg", "x.mp3");
        assert_eq!(ext, "mp3");
    }
}
