//! HTTP API handlers.

use crate::auth::{hash_password, verify_password, Sessions};
use crate::budget;
use crate::eleven::{Backend, ElError};
use crate::store::{new_id, Db, Ledger, Store, Track, Version};
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Json, Router};
use chrono::Utc;
use musik_shared::*;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;
use tower_http::services::ServeFile;

pub struct AppState {
    pub db: Mutex<Db>,
    pub store: Store,
    pub backend: Backend,
    pub sessions: Sessions,
    pub env_api_key: Option<String>,
    /// Serializes password checks: the lockout can't be raced and at most one
    /// (memory-hungry) Argon2 verification runs at a time. The permit travels
    /// into the blocking task, so an aborted request can't release it early.
    pub login_gate: Arc<tokio::sync::Semaphore>,
    /// A song is being recorded *or its cost is still being measured*. New
    /// songs wait, so two cost measurements never overlap.
    pub recording: std::sync::atomic::AtomicBool,
    /// Host names (besides IP literals and localhost) the server answers to.
    pub allowed_hosts: Vec<String>,
}

pub type Shared = Arc<AppState>;

impl AppState {
    fn api_key(&self, db: &Db) -> Option<String> {
        self.env_api_key.clone().or_else(|| db.api_key.clone()).filter(|k| !k.trim().is_empty())
    }

    /// Key to use for a call; mock mode works without one.
    fn key_or_err(&self, db: &Db) -> Result<String, AppErr> {
        match self.api_key(db) {
            Some(k) if !looks_like_api_key(&k) => Err(AppErr::new(
                StatusCode::FORBIDDEN,
                "The saved ElevenLabs key looks broken. Please ask a parent to paste it again.",
            )
            .code("bad_api_key")),
            Some(k) => Ok(k),
            None if self.backend.is_mock() => Ok(String::new()),
            None => Err(AppErr::new(StatusCode::FORBIDDEN, "There is no ElevenLabs key yet. Please ask a parent to add one.").code("no_api_key")),
        }
    }

    fn save(&self, db: &Db) -> Result<(), AppErr> {
        self.store.save(db).map_err(|e| {
            tracing::error!("saving database: {e:#}");
            AppErr::new(StatusCode::INTERNAL_SERVER_ERROR, format!("Could not save: {e}"))
        })
    }
}

// ---------------------------------------------------------------- errors

#[derive(Debug)]
pub struct AppErr {
    status: StatusCode,
    body: ApiError,
}

impl AppErr {
    pub fn new(status: StatusCode, msg: impl Into<String>) -> Self {
        AppErr { status, body: ApiError { error: msg.into(), ..Default::default() } }
    }
    fn code(mut self, code: &str) -> Self {
        self.body.code = Some(code.into());
        self
    }
    fn bad(msg: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, msg)
    }
    fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, format!("{what} not found."))
    }
}

impl From<ElError> for AppErr {
    fn from(e: ElError) -> Self {
        let status = match e.http_status {
            Some(429) => StatusCode::TOO_MANY_REQUESTS,
            Some(_) | None => StatusCode::BAD_GATEWAY,
        };
        AppErr {
            status,
            body: ApiError {
                error: e.message,
                code: e.code,
                prompt_suggestion: e.prompt_suggestion,
                plan_suggestion: e.plan_suggestion.as_ref().and_then(chunks_from_el_plan),
            },
        }
    }
}

impl IntoResponse for AppErr {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

type R<T> = Result<Json<T>, AppErr>;

// ---------------------------------------------------------------- host guard

/// Is this Host header one we serve? IP literals, localhost names, ChromeOS
/// Linux names and configured host names are fine. Anything else is refused,
/// which defeats DNS-rebinding attacks from web pages the child visits.
pub fn host_allowed(host: &str, allowed: &[String]) -> bool {
    let h = host.trim().to_ascii_lowercase();
    // Strip the port ("[::1]:8787", "1.2.3.4:8787", "name:8787").
    let name = if let Some(rest) = h.strip_prefix('[') {
        rest.split(']').next().unwrap_or("").to_string()
    } else {
        match h.rsplit_once(':') {
            Some((n, p)) if p.chars().all(|c| c.is_ascii_digit()) => n.to_string(),
            _ => h.clone(),
        }
    };
    let name = name.trim_end_matches('.');
    if name.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    name == "localhost"
        || name.ends_with(".localhost")
        || name.ends_with(".linux.test")
        || allowed.iter().any(|a| {
            let a = a.trim().trim_end_matches('.').to_ascii_lowercase();
            a == name || a.strip_prefix("*.").map(|suffix| name.ends_with(&format!(".{suffix}"))).unwrap_or(false)
        })
}

pub async fn host_guard(State(s): State<Shared>, req: Request, next: axum::middleware::Next) -> Response {
    let host = req.headers().get(header::HOST).and_then(|h| h.to_str().ok()).map(|h| h.to_string());
    match host {
        Some(h) if !host_allowed(&h, &s.allowed_hosts) => {
            tracing::warn!("refused request for unknown host {h:?} (add it with --allowed-host)");
            AppErr::new(StatusCode::MISDIRECTED_REQUEST, "Unknown host name. Start the server with --allowed-host <name> to allow it.").into_response()
        }
        _ => next.run(req).await,
    }
}

// ---------------------------------------------------------------- router

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/api/health", get(|| async { "ok" }))
        .route("/api/status", get(app_status))
        .route("/api/tracks", get(list_tracks).post(create_track))
        .route("/api/tracks/{id}", get(get_track).patch(update_track).delete(delete_track))
        .route("/api/tracks/{id}/error", delete(dismiss_error))
        .route("/api/tracks/{id}/versions", post(generate))
        .route("/api/versions/{id}", patch(update_version).delete(delete_version))
        .route("/api/versions/{id}/audio", get(version_audio))
        .route("/api/versions/{id}/download", get(version_download))
        .route("/api/plan", post(make_plan))
        .route("/api/finetunes", get(finetunes))
        .route("/api/admin/setup", post(admin_setup))
        .route("/api/admin/login", post(admin_login))
        .route("/api/admin/logout", post(admin_logout))
        .route("/api/admin/settings", get(admin_view).put(admin_save_settings))
        .route("/api/admin/api-key", put(admin_set_key).delete(admin_clear_key))
        .route("/api/admin/password", put(admin_password))
        .route("/api/admin/account", get(admin_account))
        .route("/api/admin/ledger", get(admin_ledger).post(admin_adjust))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts() {
        let allowed = vec!["musicbox".to_string(), "*.ts.net".to_string()];
        for ok in ["localhost:8787", "127.0.0.1:8787", "[::1]:8787", "192.168.1.20", "penguin.linux.test:8787", "MusicBox:8787", "pc.tail1.ts.net"] {
            assert!(host_allowed(ok, &allowed), "{ok}");
        }
        for bad in ["evil.example.com", "localhost.evil.com:8787", "musicbox.evil.com", "ts.net.evil.com"] {
            assert!(!host_allowed(bad, &allowed), "{bad}");
        }
    }

    fn test_state() -> Shared {
        let dir = std::env::temp_dir().join(format!("musik-test-{}", new_id()));
        let (store, mut db) = Store::open(&dir).unwrap();
        db.admin_password_hash = Some("x".into());
        db.settings.weekly_limit = Some(1.0);
        db.settings.monthly_limit = None;
        Arc::new(AppState {
            db: Mutex::new(db),
            store,
            backend: Backend::mock(300),
            sessions: Default::default(),
            env_api_key: None,
            login_gate: Arc::new(tokio::sync::Semaphore::new(1)),
            recording: Default::default(),
            allowed_hosts: vec![],
        })
    }

    async fn new_track(s: &Shared) -> String {
        create_track(State(s.clone()), Json(CreateTrackReq { title: "t".into(), emoji: String::new() })).await.unwrap().0.id
    }

    fn prompt(text: &str, ms: u32) -> GenerateReq {
        GenerateReq { parent_id: None, params: GenParams { prompt: text.into(), music_length_ms: ms, ..Default::default() }, name: String::new() }
    }

    async fn wait_idle(s: &Shared) {
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let db = s.db.lock().unwrap();
            if !studio_busy(s, &db) && !db.ledger.iter().any(|l| l.pending) {
                return;
            }
        }
        panic!("generation did not finish");
    }

    #[tokio::test]
    async fn money_flow_reserve_then_charge() {
        let s = test_state();
        let tid = new_track(&s).await;
        let _ = generate(State(s.clone()), Path(tid.clone()), Json(prompt("happy song", 60_000))).await.unwrap();
        {
            let db = s.db.lock().unwrap();
            let b = budget::status(&db);
            assert!((b.week.as_ref().unwrap().reserved - 0.15).abs() < 1e-9, "reserved while recording");
            assert!((b.remaining.unwrap() - 0.85).abs() < 1e-9);
        }
        // A second song can't start while one is recording...
        assert!(generate(State(s.clone()), Path(tid.clone()), Json(prompt("x", 10_000))).await.is_err());
        // ...nor while the finished one's cost is still being measured.
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if s.db.lock().unwrap().tracks[0].versions[0].status == VersionStatus::Ready {
                break;
            }
        }
        assert_eq!(s.db.lock().unwrap().tracks[0].versions[0].status, VersionStatus::Ready);
        let early = generate(State(s.clone()), Path(tid.clone()), Json(prompt("x", 10_000))).await;
        assert_eq!(early.err().map(|e| e.status), Some(StatusCode::CONFLICT), "busy while measuring");
        // Wait for the measured-cost step too (it runs after the take is ready).
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        wait_idle(&s).await;
        let db = s.db.lock().unwrap();
        let b = budget::status(&db);
        assert_eq!(b.week.as_ref().unwrap().reserved, 0.0);
        assert!((b.week.as_ref().unwrap().spent - 0.15).abs() < 1e-6, "charged: {:?}", db.ledger);
        assert_eq!(db.ledger[0].measured_credits, Some(900));
        assert_eq!(db.tracks[0].versions[0].status, VersionStatus::Ready);
    }

    #[tokio::test]
    async fn refused_prompt_is_free_but_lost_answer_is_charged() {
        let s = test_state();
        let tid = new_track(&s).await;
        let _ = generate(State(s.clone()), Path(tid.clone()), Json(prompt("a copyright song", 30_000))).await.unwrap();
        wait_idle(&s).await;
        {
            let db = s.db.lock().unwrap();
            assert!(db.ledger.is_empty(), "refused = free");
            assert_eq!(db.tracks[0].last_error.as_ref().unwrap().code.as_deref(), Some("bad_prompt"));
        }
        let _ = generate(State(s.clone()), Path(tid.clone()), Json(prompt(crate::eleven::MOCK_GLITCH, 30_000))).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
        wait_idle(&s).await;
        let db = s.db.lock().unwrap();
        assert_eq!(db.ledger.len(), 1, "billed but lost = charged");
        assert_eq!(db.ledger[0].measured_credits, Some(450));
        assert!((db.ledger[0].cost - 0.075).abs() < 1e-6);
        assert_eq!(db.tracks[0].last_error.as_ref().unwrap().code.as_deref(), Some("maybe_charged"));
        assert!(db.tracks[0].versions.is_empty());
    }

    #[tokio::test]
    async fn restart_keeps_the_reservation_as_a_charge() {
        let s = test_state();
        let tid = new_track(&s).await;
        let _ = generate(State(s.clone()), Path(tid.clone()), Json(prompt("song", 20_000))).await.unwrap();
        // Simulate a crash: take the state as it was saved while recording.
        let mut db = s.db.lock().unwrap().clone();
        assert_eq!(recover_interrupted(&mut db, &s.store), 1);
        assert!(db.ledger.iter().all(|l| !l.pending));
        assert!((budget::status(&db).week.unwrap().spent - 0.05).abs() < 1e-9);
        assert!(db.tracks[0].versions.is_empty());
        assert_eq!(db.tracks[0].last_error.as_ref().unwrap().code.as_deref(), Some("interrupted"));
    }

    #[test]
    fn measured_cost_needs_a_plausible_amount() {
        let st = Settings::default(); // $0.15 and 900 credits per minute
        // 60 s ≈ 900 credits: a matching measurement is used...
        assert!((measured_cost(900, 60_000, &st).unwrap() - 0.15).abs() < 1e-9);
        // ...a cheaper one (kept parts) too...
        assert!(measured_cost(400, 60_000, &st).is_some());
        // ...but a counter that barely moved, or jumped, falls back to the estimate.
        assert!(measured_cost(40, 60_000, &st).is_none());
        assert!(measured_cost(0, 60_000, &st).is_none());
        assert!(measured_cost(9_000, 60_000, &st).is_none());
    }

    #[test]
    fn mask_handles_unicode() {
        assert_eq!(mask("sk_abcdefghijkl"), "sk_a…ijkl");
        assert_eq!(mask("ééééééééééé"), "éééé…éééé");
        assert!(check_api_key("sk_abc€defghijk").is_err());
        assert!(check_api_key("sk_0123456789abcdef").is_ok());
    }
}

// ---------------------------------------------------------------- kid API

fn any_generating(db: &Db) -> bool {
    db.tracks.iter().any(|t| t.versions.iter().any(|v| v.status == VersionStatus::Generating))
}

fn studio_busy(s: &AppState, db: &Db) -> bool {
    any_generating(db) || s.recording.load(std::sync::atomic::Ordering::Acquire)
}

/// Clears the `recording` flag when the generation task ends, however it ends.
struct RecordingGuard(Shared);

impl Drop for RecordingGuard {
    fn drop(&mut self) {
        self.0.recording.store(false, std::sync::atomic::Ordering::Release);
    }
}

async fn app_status(State(s): State<Shared>) -> Json<AppStatus> {
    let db = s.db.lock().unwrap();
    Json(AppStatus {
        setup_needed: db.admin_password_hash.is_none(),
        api_key_configured: s.api_key(&db).is_some() || s.backend.is_mock(),
        mock_mode: s.backend.is_mock(),
        generation_enabled: db.settings.generation_enabled,
        max_track_seconds: db.settings.max_track_seconds,
        default_model: db.settings.default_model.clone(),
        store_for_inpainting: db.settings.store_for_inpainting,
        currency: db.settings.currency.clone(),
        price_per_minute: db.settings.price_per_minute,
        budget: budget::status(&db),
        busy: studio_busy(&s, &db),
    })
}

async fn list_tracks(State(s): State<Shared>) -> Json<Vec<TrackSummary>> {
    let db = s.db.lock().unwrap();
    let mut tracks: Vec<&Track> = db.tracks.iter().collect();
    tracks.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
    Json(tracks.into_iter().map(|t| t.summary()).collect())
}

const EMOJIS: &[&str] = &["🎵", "🎸", "🎹", "🥁", "🎺", "🎻", "🎤", "🎧", "🌈", "🚀", "🦄", "🐱", "🌊", "⭐", "🍓", "🐉"];

async fn create_track(State(s): State<Shared>, Json(req): Json<CreateTrackReq>) -> R<TrackDetail> {
    let mut db = s.db.lock().unwrap();
    let title: String = req.title.trim().chars().take(80).collect();
    let emoji = if req.emoji.trim().is_empty() {
        EMOJIS[rand::random::<usize>() % EMOJIS.len()].to_string()
    } else {
        req.emoji.trim().chars().take(4).collect()
    };
    let now = Utc::now();
    let t = Track {
        id: new_id(),
        title: if title.is_empty() { "My new song".into() } else { title },
        emoji,
        created_at: now,
        updated_at: now,
        versions: vec![],
        next_branch: 0,
        last_error: None,
    };
    let detail = t.detail();
    db.tracks.push(t);
    s.save(&db)?;
    Ok(Json(detail))
}

fn track_mut<'a>(db: &'a mut Db, id: &str) -> Result<&'a mut Track, AppErr> {
    db.tracks.iter_mut().find(|t| t.id == id).ok_or_else(|| AppErr::not_found("Track"))
}

async fn get_track(State(s): State<Shared>, Path(id): Path<String>) -> R<TrackDetail> {
    let db = s.db.lock().unwrap();
    db.tracks.iter().find(|t| t.id == id).map(|t| Json(t.detail())).ok_or_else(|| AppErr::not_found("Track"))
}

async fn update_track(State(s): State<Shared>, Path(id): Path<String>, Json(req): Json<UpdateTrackReq>) -> R<TrackDetail> {
    let mut db = s.db.lock().unwrap();
    let t = track_mut(&mut db, &id)?;
    if let Some(title) = req.title {
        let title: String = title.trim().chars().take(80).collect();
        if !title.is_empty() {
            t.title = title;
        }
    }
    if let Some(e) = req.emoji {
        t.emoji = e.trim().chars().take(4).collect();
    }
    t.updated_at = Utc::now();
    let d = t.detail();
    s.save(&db)?;
    Ok(Json(d))
}

async fn delete_track(State(s): State<Shared>, Path(id): Path<String>) -> R<serde_json::Value> {
    let files: Vec<String> = {
        let mut db = s.db.lock().unwrap();
        let t = track_mut(&mut db, &id)?;
        if t.versions.iter().any(|v| v.status == VersionStatus::Generating) {
            return Err(AppErr::new(StatusCode::CONFLICT, "Wait until the song being made is finished."));
        }
        let files = t.versions.iter().filter_map(|v| v.audio_file.clone()).collect();
        db.tracks.retain(|t| t.id != id);
        s.save(&db)?;
        files
    };
    for f in files {
        let _ = tokio::fs::remove_file(s.store.audio_path(&f)).await;
    }
    Ok(Json(serde_json::json!({"deleted": id})))
}

async fn dismiss_error(State(s): State<Shared>, Path(id): Path<String>) -> R<TrackDetail> {
    let mut db = s.db.lock().unwrap();
    let t = track_mut(&mut db, &id)?;
    t.last_error = None;
    let d = t.detail();
    s.save(&db)?;
    Ok(Json(d))
}

async fn generate(State(s): State<Shared>, Path(track_id): Path<String>, Json(req): Json<GenerateReq>) -> R<VersionInfo> {
    let (version, key) = {
        let mut db = s.db.lock().unwrap();
        if db.admin_password_hash.is_none() {
            return Err(AppErr::new(StatusCode::FORBIDDEN, "A parent needs to set up the app first.").code("setup_needed"));
        }
        if !db.settings.generation_enabled {
            return Err(AppErr::new(StatusCode::FORBIDDEN, "Making new songs is paused right now. Ask a parent.").code("paused"));
        }
        let key = s.key_or_err(&db)?;
        if studio_busy(&s, &db) {
            return Err(AppErr::new(StatusCode::CONFLICT, "Another song is still being made. Wait until it's finished.").code("busy"));
        }

        let mut params = req.params.clone();
        match params.mode {
            Mode::Prompt => {
                params.chunks.clear();
                // ElevenLabs rejects a seed together with a prompt.
                params.seed = None;
            }
            Mode::Plan => {
                params.force_instrumental = false;
                params.generation_mode.clear();
                params.lyrics_text.clear();
                if params.seed.is_none() {
                    // Record the seed so this version can be re-rolled the same way.
                    params.seed = Some(rand::random::<u32>() % MAX_SEED);
                }
            }
        }
        if params.finetune_id.is_empty() {
            params.finetune_strength = 1.0;
        }
        let errs = validate_params(&params, db.settings.max_track_seconds.saturating_mul(1000));
        if !errs.is_empty() {
            return Err(AppErr::bad(errs.join(" ")).code("invalid"));
        }

        let settings = db.settings.clone();
        let duration = params_duration_ms(&params);
        let estimate = estimate_cost(duration, settings.price_per_minute);
        if let Err((msg, code)) = budget::check(&db, estimate) {
            return Err(AppErr::new(StatusCode::PAYMENT_REQUIRED, msg).code(code));
        }

        let track = track_mut(&mut db, &track_id)?;
        if let Some(pid) = &req.parent_id {
            if track.version(pid).map(|v| v.status) != Some(VersionStatus::Ready) {
                return Err(AppErr::bad("The version you started from doesn't exist anymore."));
            }
        }
        // Kept / "sound similar" parts may only come from this track's stored
        // songs, and only from inside them.
        for c in &params.chunks {
            let range = match c {
                PlanChunk::Keep(k) => Some(&k.source),
                PlanChunk::Generate(g) => g.conditioning.as_ref(),
            };
            if let Some(r) = range {
                let source = track
                    .versions
                    .iter()
                    .find(|v| v.song_id.as_deref() == Some(r.song_id.as_str()))
                    .ok_or_else(|| AppErr::bad("A section refers to a version that can't be reused (it wasn't stored for editing)."))?;
                if r.end_ms > source.duration_ms {
                    return Err(AppErr::bad(format!("A section refers to a part after the end of {}.", source.label)));
                }
            }
        }

        let old_branch = track.next_branch;
        let old_error = track.last_error.clone();
        let (depth, branch, label) = track.allocate_label(req.parent_id.as_deref());
        let v = Version {
            id: new_id(),
            parent_id: req.parent_id.clone(),
            label,
            depth,
            branch,
            name: req.name.trim().chars().take(60).collect(),
            notes: String::new(),
            favorite: false,
            created_at: Utc::now(),
            status: VersionStatus::Generating,
            params,
            duration_ms: duration,
            estimated_cost: estimate,
            charged_cost: None,
            measured_credits: None,
            song_id: None,
            el_plan: None,
            song_metadata: None,
            audio_file: None,
            content_type: None,
        };
        track.versions.push(v.clone());
        track.updated_at = Utc::now();
        track.last_error = None;
        let track_title = track.title.clone();
        // Reserve the money in the ledger right away: if the app is closed
        // while recording, the (possibly billed) cost is not forgotten.
        db.ledger.push(Ledger {
            id: new_id(),
            at: Utc::now(),
            kind: LedgerKind::Generation,
            track_id: Some(track_id.clone()),
            version_id: Some(v.id.clone()),
            track_title,
            version_label: v.label.clone(),
            seconds: duration as f64 / 1000.0,
            estimated_cost: estimate,
            measured_credits: None,
            cost: estimate,
            note: "recording".into(),
            pending: true,
        });
        if let Err(e) = s.save(&db) {
            // Roll back, otherwise a phantom "recording" take blocks the studio.
            db.ledger.retain(|l| l.version_id.as_deref() != Some(v.id.as_str()));
            if let Ok(t) = track_mut(&mut db, &track_id) {
                t.versions.retain(|x| x.id != v.id);
                t.next_branch = old_branch;
                t.last_error = old_error;
            }
            return Err(e);
        }
        // Set while still holding the DB lock, so a parallel request sees it.
        s.recording.store(true, std::sync::atomic::Ordering::Release);
        (v, key)
    };
    tracing::info!("generating {} ({} ms, est. {:.3})", version.label, version.duration_ms, version.estimated_cost);
    tokio::spawn(run_generation(s.clone(), track_id, version.id.clone(), key));
    Ok(Json(version.info()))
}

/// Money from a measured credit delta, if it looks like it belongs to this
/// song: not tiny (counter not updated yet, or only someone else's usage) and
/// not huge (someone else used the account too). Kept parts may be cheaper,
/// hence the generous lower bound.
fn measured_cost(m: i64, duration_ms: u32, st: &Settings) -> Option<f64> {
    let est_credits = estimate_credits(duration_ms, st.credits_per_minute);
    let m = m as f64;
    (st.credits_per_minute > 0.0 && m >= est_credits * 0.25 && m <= est_credits * 3.0 + 50.0)
        .then(|| m / st.credits_per_minute * st.price_per_minute)
}

async fn credits_after(s: &Shared, key: &str, before: Option<i64>) -> Option<i64> {
    let b = before?;
    // The credit counter can lag a little behind.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    s.backend.subscription(key).await.ok().map(|a| a.credits_used - b)
}

fn ledger_entry<'a>(db: &'a mut Db, version_id: &str) -> Option<&'a mut Ledger> {
    db.ledger.iter_mut().find(|l| l.version_id.as_deref() == Some(version_id) && l.kind == LedgerKind::Generation)
}

async fn run_generation(s: Shared, track_id: String, version_id: String, key: String) {
    let _busy = RecordingGuard(s.clone());
    let found = {
        let db = s.db.lock().unwrap();
        db.tracks
            .iter()
            .find(|t| t.id == track_id)
            .and_then(|t| t.version(&version_id))
            .map(|v| (v.params.clone(), v.duration_ms, db.settings.clone()))
    };
    let Some((params, duration, settings)) = found else { return };
    let est_cost = estimate_cost(duration, settings.price_per_minute);

    let before = if settings.measure_actual_cost {
        s.backend.subscription(&key).await.ok().map(|x| x.credits_used)
    } else {
        None
    };
    let result = s.backend.compose(&key, &params).await;

    match result {
        Ok(out) => {
            let file = format!("{version_id}.{}", out.ext);
            // Write to a temporary name first: a crash mid-write must never
            // leave a truncated file that looks like a finished take.
            let part = s.store.audio_path(&format!("{file}.part"));
            let write_err = match tokio::fs::write(&part, &out.audio).await {
                Ok(()) => tokio::fs::rename(&part, s.store.audio_path(&file)).await.err(),
                Err(e) => Some(e),
            };
            if write_err.is_some() {
                let _ = tokio::fs::remove_file(&part).await;
            }
            // Step 1: store the song and charge the estimate immediately.
            let label = {
                let mut db = s.db.lock().unwrap();
                let mut label = String::new();
                if let Some(t) = db.tracks.iter_mut().find(|t| t.id == track_id) {
                    if let Some(e) = &write_err {
                        tracing::error!("writing audio: {e}");
                        if let Some(v) = t.version(&version_id) {
                            label = v.label.clone();
                            let gen_err = GenError {
                                at: Utc::now().to_rfc3339(),
                                message: format!("The song was made (and paid) but could not be saved on this computer: {e}"),
                                code: Some("disk".into()),
                                prompt_suggestion: None,
                                plan_suggestion: None,
                                params: v.params.clone(),
                                parent_id: v.parent_id.clone(),
                            };
                            t.versions.retain(|v| v.id != version_id);
                            t.last_error = Some(gen_err);
                        }
                    } else if let Some(v) = t.versions.iter_mut().find(|v| v.id == version_id) {
                        label = v.label.clone();
                        v.status = VersionStatus::Ready;
                        v.song_id = out.song_id.clone();
                        v.el_plan = out.plan.clone();
                        v.song_metadata = out.metadata.clone();
                        v.audio_file = Some(file.clone());
                        v.content_type = Some(out.content_type.clone());
                        v.charged_cost = Some(est_cost);
                    }
                    t.updated_at = Utc::now();
                }
                // Money was spent either way.
                if let Some(l) = ledger_entry(&mut db, &version_id) {
                    l.pending = false;
                    l.cost = est_cost;
                    l.at = Utc::now();
                    l.note = if s.backend.is_mock() { "mock".into() } else { String::new() };
                }
                if let Err(e) = s.store.save(&db) {
                    tracing::error!("saving database: {e:#}");
                }
                label
            };
            // Step 2: replace the estimate with what was really used.
            let measured = credits_after(&s, &key, before).await;
            if let Some(m) = measured {
                let mut db = s.db.lock().unwrap();
                let charged = measured_cost(m, duration, &settings);
                if let Some(l) = ledger_entry(&mut db, &version_id) {
                    l.measured_credits = Some(m);
                    if let Some(c) = charged {
                        l.cost = c;
                    }
                }
                if let Some(v) = db.tracks.iter_mut().flat_map(|t| t.versions.iter_mut()).find(|v| v.id == version_id) {
                    v.measured_credits = Some(m);
                    if let Some(c) = charged {
                        v.charged_cost = Some(c);
                    }
                }
                if let Err(e) = s.store.save(&db) {
                    tracing::error!("saving database: {e:#}");
                }
                tracing::info!("finished {label}: measured {m} credits, charged {:.4}", charged.unwrap_or(est_cost));
            } else {
                tracing::info!("finished {label}: charged estimate {est_cost:.4}");
            }
        }
        Err(e) => {
            tracing::warn!("generation failed: {} ({:?}, maybe charged: {})", e.message, e.code, e.maybe_charged);
            // Refused requests (4xx) and requests that never left are free.
            // Otherwise ElevenLabs may have billed it. The credit counter can
            // lag, so "no change yet" proves nothing: use a plausible measured
            // amount, else the estimate (a parent can refund it).
            let charge: Option<(f64, Option<i64>, &str)> = if e.maybe_charged {
                let m = credits_after(&s, &key, before).await;
                match m.and_then(|m| measured_cost(m, duration, &settings)) {
                    Some(c) => Some((c, m, "failed after sending (measured)")),
                    None => Some((est_cost, m, "failed after sending: may have been charged; refund with an adjustment if not")),
                }
            } else {
                None
            };
            let mut db = s.db.lock().unwrap();
            match charge {
                Some((cost, m, note)) => {
                    if let Some(l) = ledger_entry(&mut db, &version_id) {
                        l.pending = false;
                        l.cost = cost;
                        l.measured_credits = m;
                        l.at = Utc::now();
                        l.note = note.into();
                    }
                }
                None => db.ledger.retain(|l| !(l.version_id.as_deref() == Some(version_id.as_str()) && l.pending)),
            }
            if let Some(t) = db.tracks.iter_mut().find(|t| t.id == track_id) {
                let parent_id = t.version(&version_id).and_then(|v| v.parent_id.clone());
                t.versions.retain(|v| v.id != version_id);
                t.last_error = Some(GenError {
                    at: Utc::now().to_rfc3339(),
                    message: if charge.is_some() {
                        format!("{} This attempt may have used some budget.", e.message)
                    } else {
                        e.message.clone()
                    },
                    code: if charge.is_some() { Some("maybe_charged".into()) } else { e.code.clone() },
                    prompt_suggestion: e.prompt_suggestion.clone(),
                    plan_suggestion: e.plan_suggestion.as_ref().and_then(chunks_from_el_plan),
                    params,
                    parent_id,
                });
            }
            if let Err(e) = s.store.save(&db) {
                tracing::error!("saving database: {e:#}");
            }
        }
    }
}

fn find_version<'a>(db: &'a Db, id: &str) -> Option<(&'a Track, &'a Version)> {
    db.tracks.iter().find_map(|t| t.version(id).map(|v| (t, v)))
}

async fn update_version(State(s): State<Shared>, Path(id): Path<String>, Json(req): Json<UpdateVersionReq>) -> R<VersionInfo> {
    let mut db = s.db.lock().unwrap();
    let v = db
        .tracks
        .iter_mut()
        .flat_map(|t| t.versions.iter_mut())
        .find(|v| v.id == id)
        .ok_or_else(|| AppErr::not_found("Version"))?;
    if let Some(n) = req.name {
        v.name = n.trim().chars().take(60).collect();
    }
    if let Some(n) = req.notes {
        v.notes = n.chars().take(2000).collect();
    }
    if let Some(f) = req.favorite {
        v.favorite = f;
    }
    let info = v.info();
    s.save(&db)?;
    Ok(Json(info))
}

async fn delete_version(State(s): State<Shared>, Path(id): Path<String>) -> R<serde_json::Value> {
    let files: Vec<String> = {
        let mut db = s.db.lock().unwrap();
        let t = db
            .tracks
            .iter_mut()
            .find(|t| t.version(&id).is_some())
            .ok_or_else(|| AppErr::not_found("Version"))?;
        let ids: HashSet<String> = t.subtree_ids(&id).into_iter().collect();
        if t.versions.iter().any(|v| ids.contains(&v.id) && v.status == VersionStatus::Generating) {
            return Err(AppErr::new(StatusCode::CONFLICT, "Wait until the song being made is finished."));
        }
        let files = t.versions.iter().filter(|v| ids.contains(&v.id)).filter_map(|v| v.audio_file.clone()).collect();
        t.versions.retain(|v| !ids.contains(&v.id));
        t.updated_at = Utc::now();
        s.save(&db)?;
        files
    };
    for f in &files {
        let _ = tokio::fs::remove_file(s.store.audio_path(f)).await;
    }
    Ok(Json(serde_json::json!({"deleted": files.len()})))
}

async fn serve_audio(s: &Shared, id: &str, req: Request, download: bool) -> Result<Response, AppErr> {
    let (path, filename) = {
        let db = s.db.lock().unwrap();
        let (t, v) = find_version(&db, id).ok_or_else(|| AppErr::not_found("Version"))?;
        let file = v.audio_file.clone().ok_or_else(|| AppErr::not_found("Audio"))?;
        let ext = file.rsplit('.').next().unwrap_or("mp3").to_string();
        let name = if v.name.is_empty() { format!("{} - {}", t.title, v.label) } else { format!("{} - {} ({})", t.title, v.label, v.name) };
        (s.store.audio_path(&file), format!("{name}.{ext}"))
    };
    let res = ServeFile::new(path).oneshot(req).await.map_err(|e| AppErr::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut res = res.map(Body::new);
    if download {
        let ascii: String = filename.chars().map(|c| if c.is_ascii_alphanumeric() || " .-_()".contains(c) { c } else { '_' }).collect();
        let encoded: String = filename
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() || b".-_".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")) {
            res.headers_mut().insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(res)
}

async fn version_audio(State(s): State<Shared>, Path(id): Path<String>, req: Request) -> Result<Response, AppErr> {
    serve_audio(&s, &id, req, false).await
}

async fn version_download(State(s): State<Shared>, Path(id): Path<String>, req: Request) -> Result<Response, AppErr> {
    serve_audio(&s, &id, req, true).await
}

async fn make_plan(State(s): State<Shared>, Json(mut req): Json<PlanReq>) -> R<PlanResp> {
    let key = {
        let db = s.db.lock().unwrap();
        if db.admin_password_hash.is_none() {
            return Err(AppErr::new(StatusCode::FORBIDDEN, "A parent needs to set up the app first."));
        }
        // Keep the plan inside the parent's length limit too.
        req.music_length_ms = req.music_length_ms.clamp(MIN_TOTAL_MS, db.settings.max_track_seconds.saturating_mul(1000).max(MIN_TOTAL_MS));
        s.key_or_err(&db)?
    };
    if req.prompt.trim().is_empty() {
        return Err(AppErr::bad("Write what the plan should be about first."));
    }
    if req.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(AppErr::bad("That description is too long."));
    }
    if !MODELS.iter().any(|(m, _)| *m == req.model_id) {
        req.model_id = DEFAULT_MODEL.into();
    }
    // Plans are free (no credits), so no budget check here.
    let v = s.backend.plan(&key, &req).await?;
    let chunks = chunks_from_el_plan(&v).ok_or_else(|| AppErr::new(StatusCode::BAD_GATEWAY, "ElevenLabs sent back a plan this app doesn't understand."))?;
    Ok(Json(PlanResp { chunks }))
}

async fn finetunes(State(s): State<Shared>) -> R<Vec<Finetune>> {
    let key = {
        let db = s.db.lock().unwrap();
        s.key_or_err(&db)?
    };
    Ok(Json(s.backend.finetunes(&key).await?))
}

// ---------------------------------------------------------------- parent API

fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
}

fn require_admin(s: &AppState, headers: &HeaderMap) -> Result<String, AppErr> {
    match bearer(headers) {
        Some(t) if s.sessions.check(&t) => Ok(t),
        _ => Err(AppErr::new(StatusCode::UNAUTHORIZED, "Please log in as a parent.").code("login_required")),
    }
}

fn mask(key: &str) -> String {
    let c: Vec<char> = key.trim().chars().collect();
    if c.len() <= 8 {
        "•".repeat(c.len())
    } else {
        format!("{}…{}", c[..4].iter().collect::<String>(), c[c.len() - 4..].iter().collect::<String>())
    }
}

pub fn looks_like_api_key(key: &str) -> bool {
    (10..=200).contains(&key.len()) && key.bytes().all(|b| (0x21..0x7f).contains(&b))
}

fn check_api_key(key: &str) -> Result<(), AppErr> {
    if !looks_like_api_key(key) {
        return Err(AppErr::bad("That doesn't look like an ElevenLabs API key."));
    }
    Ok(())
}

fn admin_view_of(s: &AppState, db: &Db) -> AdminView {
    AdminView {
        settings: db.settings.clone(),
        api_key_masked: s.api_key(db).map(|k| mask(&k)),
        api_key_from_env: s.env_api_key.is_some(),
        mock_mode: s.backend.is_mock(),
        data_dir: s.store.dir.display().to_string(),
        budget: budget::status(db),
    }
}

fn check_settings(st: &Settings) -> Result<(), AppErr> {
    let finite_pos = |x: f64| x.is_finite() && x >= 0.0;
    if !(3..=600).contains(&st.max_track_seconds) {
        return Err(AppErr::bad("Maximum track length must be between 3 and 600 seconds."));
    }
    if !finite_pos(st.price_per_minute) || !finite_pos(st.credits_per_minute) {
        return Err(AppErr::bad("Prices must be zero or positive numbers."));
    }
    if st.weekly_limit.map(|l| !finite_pos(l)).unwrap_or(false) || st.monthly_limit.map(|l| !finite_pos(l)).unwrap_or(false) {
        return Err(AppErr::bad("Limits must be zero or positive numbers."));
    }
    if st.currency.chars().count() > 5 {
        return Err(AppErr::bad("Currency should be a short symbol like $ or €."));
    }
    if !MODELS.iter().any(|(m, _)| *m == st.default_model) {
        return Err(AppErr::bad("Unknown default model."));
    }
    if (st.weekly_limit.is_some() || st.monthly_limit.is_some()) && st.price_per_minute <= 0.0 {
        return Err(AppErr::bad("The price per minute must be above 0, otherwise the spending limits can't work."));
    }
    if st.measure_actual_cost && st.credits_per_minute <= 0.0 {
        return Err(AppErr::bad("Credits per minute must be above 0 to measure the real cost."));
    }
    Ok(())
}

async fn admin_setup(State(s): State<Shared>, Json(req): Json<SetupReq>) -> R<LoginResp> {
    let mut db = s.db.lock().unwrap();
    if db.admin_password_hash.is_some() {
        return Err(AppErr::new(StatusCode::CONFLICT, "The app is already set up. Log in instead."));
    }
    if req.password.chars().count() < 6 {
        return Err(AppErr::bad("Choose a password with at least 6 characters."));
    }
    if let Some(st) = &req.settings {
        check_settings(st)?;
        db.settings = st.clone();
    }
    if !req.api_key.trim().is_empty() {
        check_api_key(req.api_key.trim())?;
    }
    db.admin_password_hash = Some(hash_password(&req.password));
    if !req.api_key.trim().is_empty() {
        db.api_key = Some(req.api_key.trim().to_string());
    }
    s.save(&db)?;
    Ok(Json(LoginResp { token: s.sessions.create() }))
}

/// Check a password against the parent hash with the lockout applied.
/// Everything (lockout check, Argon2, counting) happens inside the blocking
/// task that owns the gate permit, so an aborted request can neither release
/// the gate early nor skip the failure count.
/// Returns the hash that was verified.
async fn check_parent_password(s: &Shared, password: String) -> Result<String, AppErr> {
    let hash = s.db.lock().unwrap().admin_password_hash.clone();
    let Some(hash) = hash else {
        return Err(AppErr::new(StatusCode::CONFLICT, "The app is not set up yet.").code("setup_needed"));
    };
    let permit = s.login_gate.clone().acquire_owned().await.map_err(|_| AppErr::new(StatusCode::SERVICE_UNAVAILABLE, "Shutting down."))?;
    let st = s.clone();
    let checked = hash.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        if let Some(wait) = st.sessions.locked_for() {
            return Err(wait);
        }
        let ok = verify_password(&password, &checked);
        if ok {
            st.sessions.record_success();
        } else {
            st.sessions.record_failure();
        }
        Ok(ok)
    })
    .await
    .unwrap_or(Ok(false));
    match outcome {
        Err(wait) => Err(AppErr::new(StatusCode::TOO_MANY_REQUESTS, format!("Too many wrong passwords. Try again in {wait} seconds."))),
        Ok(false) => Err(AppErr::new(StatusCode::UNAUTHORIZED, "Wrong password.")),
        Ok(true) => Ok(hash),
    }
}

async fn admin_login(State(s): State<Shared>, Json(req): Json<LoginReq>) -> R<LoginResp> {
    let verified = check_parent_password(&s, req.password).await?;
    // Create the session only if that password is still the current one, and
    // while holding the DB lock: a password change (which revokes all
    // sessions under the same lock) can't slip in between.
    let db = s.db.lock().unwrap();
    if db.admin_password_hash.as_deref() != Some(verified.as_str()) {
        return Err(AppErr::new(StatusCode::UNAUTHORIZED, "Wrong password."));
    }
    let token = s.sessions.create();
    drop(db);
    Ok(Json(LoginResp { token }))
}

async fn admin_logout(State(s): State<Shared>, headers: HeaderMap) -> R<serde_json::Value> {
    if let Some(t) = bearer(&headers) {
        s.sessions.revoke(&t);
    }
    Ok(Json(serde_json::json!({"ok": true})))
}

async fn admin_view(State(s): State<Shared>, headers: HeaderMap) -> R<AdminView> {
    require_admin(&s, &headers)?;
    let db = s.db.lock().unwrap();
    Ok(Json(admin_view_of(&s, &db)))
}

async fn admin_save_settings(State(s): State<Shared>, headers: HeaderMap, Json(st): Json<Settings>) -> R<AdminView> {
    require_admin(&s, &headers)?;
    check_settings(&st)?;
    let mut db = s.db.lock().unwrap();
    db.settings = st;
    s.save(&db)?;
    Ok(Json(admin_view_of(&s, &db)))
}

async fn admin_set_key(State(s): State<Shared>, headers: HeaderMap, Json(req): Json<ApiKeyReq>) -> R<ApiKeyResp> {
    require_admin(&s, &headers)?;
    let key = req.api_key.trim().to_string();
    if key.is_empty() {
        return Err(AppErr::bad("Paste an API key."));
    }
    check_api_key(&key)?;
    let check = match s.backend.subscription(&key).await {
        Err(e) if e.is_invalid_key() => {
            return Err(AppErr::bad("ElevenLabs says this key is invalid. Please copy it again (it starts with sk_)."));
        }
        Ok(sub) => format!(
            "Key works. Plan: {}, credits used this period: {} of {}.",
            sub.tier, sub.credits_used, sub.credits_limit
        ),
        Err(e) if matches!(e.http_status, Some(401) | Some(403)) => format!(
            "Saved, but the key can't read account info ({}). That's fine if you limited it to Music only: costs will then be estimated instead of measured.",
            e.message
        ),
        Err(e) => format!("Saved, but the test call failed: {}", e.message),
    };
    let mut db = s.db.lock().unwrap();
    db.api_key = Some(key.clone());
    s.save(&db)?;
    let shown = s.api_key(&db).map(|k| mask(&k)).unwrap_or_default();
    Ok(Json(ApiKeyResp { api_key_masked: shown, check }))
}

async fn admin_clear_key(State(s): State<Shared>, headers: HeaderMap) -> R<AdminView> {
    require_admin(&s, &headers)?;
    let mut db = s.db.lock().unwrap();
    db.api_key = None;
    s.save(&db)?;
    Ok(Json(admin_view_of(&s, &db)))
}

async fn admin_password(State(s): State<Shared>, headers: HeaderMap, Json(req): Json<PasswordReq>) -> R<LoginResp> {
    require_admin(&s, &headers)?;
    if req.new.chars().count() < 6 {
        return Err(AppErr::bad("Choose a password with at least 6 characters."));
    }
    match check_parent_password(&s, req.current.clone()).await {
        // 403, not 401: the session is fine, only the typed password is wrong.
        Err(e) if e.status == StatusCode::UNAUTHORIZED => {
            return Err(AppErr::new(StatusCode::FORBIDDEN, "The current password is wrong."));
        }
        Err(e) => return Err(e),
        Ok(_) => {}
    }
    let new_hash = tokio::task::spawn_blocking(move || hash_password(&req.new)).await.map_err(|e| AppErr::bad(e.to_string()))?;
    let mut db = s.db.lock().unwrap();
    db.admin_password_hash = Some(new_hash);
    s.save(&db)?;
    // Under the same lock as admin_login's session creation.
    s.sessions.revoke_all();
    let token = s.sessions.create();
    drop(db);
    Ok(Json(LoginResp { token }))
}

async fn admin_account(State(s): State<Shared>, headers: HeaderMap) -> R<AccountInfo> {
    require_admin(&s, &headers)?;
    let key = {
        let db = s.db.lock().unwrap();
        s.key_or_err(&db)?
    };
    let sub = s.backend.subscription(&key).await?;
    Ok(Json(AccountInfo {
        tier: sub.tier,
        status: sub.status,
        credits_used: sub.credits_used,
        credits_limit: sub.credits_limit,
        next_reset: sub
            .next_reset_unix
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|t| t.to_rfc3339()),
        currency: sub.currency,
    }))
}

async fn admin_ledger(State(s): State<Shared>, headers: HeaderMap) -> R<Vec<LedgerEntry>> {
    require_admin(&s, &headers)?;
    let db = s.db.lock().unwrap();
    let mut entries: Vec<LedgerEntry> = db.ledger.iter().map(|e| e.entry()).collect();
    entries.reverse();
    Ok(Json(entries))
}

async fn admin_adjust(State(s): State<Shared>, headers: HeaderMap, Json(req): Json<AdjustReq>) -> R<Vec<LedgerEntry>> {
    require_admin(&s, &headers)?;
    if !req.amount.is_finite() || req.amount.abs() > 10_000.0 || req.amount == 0.0 {
        return Err(AppErr::bad("Enter an amount (negative gives budget back)."));
    }
    let mut db = s.db.lock().unwrap();
    db.ledger.push(Ledger {
        id: new_id(),
        at: Utc::now(),
        kind: LedgerKind::Adjustment,
        track_id: None,
        version_id: None,
        track_title: String::new(),
        version_label: String::new(),
        seconds: 0.0,
        estimated_cost: 0.0,
        measured_credits: None,
        cost: req.amount,
        note: req.note.chars().take(200).collect(),
        pending: false,
    });
    s.save(&db)?;
    let mut entries: Vec<LedgerEntry> = db.ledger.iter().map(|e| e.entry()).collect();
    entries.reverse();
    Ok(Json(entries))
}

/// On startup: generations that were running when the server stopped will
/// never finish. If their audio was already saved, keep the take; otherwise
/// remove it. Either way their reserved cost is charged (ElevenLabs probably
/// billed it); a parent can give it back with an adjustment.
pub fn recover_interrupted(db: &mut Db, store: &Store) -> usize {
    let mut n = 0;
    let mut finalized: Vec<(String, String)> = Vec::new(); // (version id, note)
    for t in db.tracks.iter_mut() {
        let stale: Vec<Version> = t.versions.iter().filter(|v| v.status == VersionStatus::Generating).cloned().collect();
        for v in stale {
            n += 1;
            let saved = ["mp3", "wav", "ogg", "opus"]
                .iter()
                .map(|ext| format!("{}.{ext}", v.id))
                .find(|f| std::fs::metadata(store.audio_path(f)).map(|m| m.len() > 0).unwrap_or(false));
            for ext in ["mp3", "wav", "ogg", "opus"] {
                let _ = std::fs::remove_file(store.audio_path(&format!("{}.{ext}.part", v.id)));
            }
            if let Some(file) = saved {
                if let Some(x) = t.versions.iter_mut().find(|x| x.id == v.id) {
                    x.status = VersionStatus::Ready;
                    x.content_type = Some(crate::eleven::finalize_audio(Vec::new(), "", "", &file).2);
                    x.audio_file = Some(file);
                    x.charged_cost = Some(x.estimated_cost);
                }
                finalized.push((v.id.clone(), "recovered after a restart (estimate)".into()));
            } else {
                t.versions.retain(|x| x.id != v.id);
                t.last_error = Some(GenError {
                    at: Utc::now().to_rfc3339(),
                    message: "The app was closed while this song was being made, so it got lost. You can try again.".into(),
                    code: Some("interrupted".into()),
                    prompt_suggestion: None,
                    plan_suggestion: None,
                    params: v.params.clone(),
                    parent_id: v.parent_id.clone(),
                });
                finalized.push((v.id.clone(), "interrupted: may have been charged; refund with an adjustment if not".into()));
            }
        }
    }
    for (vid, note) in finalized {
        if let Some(l) = ledger_entry(db, &vid) {
            l.pending = false;
            l.note = note;
        }
    }
    // Any other leftover reservation (should not happen) becomes final too.
    for l in db.ledger.iter_mut().filter(|l| l.pending) {
        l.pending = false;
        l.note = "interrupted (estimate)".into();
    }
    n
}
