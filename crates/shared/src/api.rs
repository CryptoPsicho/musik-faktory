//! Request and response bodies of the Musik Faktory HTTP API.

use crate::plan::*;
use serde::{Deserialize, Serialize};

/// Parent-controlled settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Shown next to amounts, e.g. "$" or "€".
    pub currency: String,
    /// What one minute of generated music costs you (ElevenLabs API: $0.15).
    pub price_per_minute: f64,
    /// Credits ElevenLabs charges per minute of music (about 900). Used to
    /// turn the measured credit usage into money.
    pub credits_per_minute: f64,
    pub weekly_limit: Option<f64>,
    pub monthly_limit: Option<f64>,
    /// Longest track that may be generated, in seconds.
    pub max_track_seconds: u32,
    /// Kill switch: when false nothing can be generated.
    pub generation_enabled: bool,
    /// Week resets on Sunday instead of Monday.
    pub week_starts_sunday: bool,
    /// Model preselected in the editor.
    pub default_model: String,
    /// Store songs on ElevenLabs so sections can be kept/reused in later versions.
    pub store_for_inpainting: bool,
    /// Read the ElevenLabs credit counter before/after each generation and
    /// charge what was really used (falls back to the estimate).
    pub measure_actual_cost: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            currency: "$".into(),
            price_per_minute: 0.15,
            credits_per_minute: 900.0,
            weekly_limit: Some(1.50),
            monthly_limit: Some(5.00),
            max_track_seconds: 60,
            generation_enabled: true,
            week_starts_sunday: false,
            default_model: DEFAULT_MODEL.into(),
            store_for_inpainting: true,
            measure_actual_cost: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PeriodStatus {
    pub limit: f64,
    pub spent: f64,
    /// Cost of generations still running (already counted against the limit).
    pub reserved: f64,
    pub remaining: f64,
    /// RFC 3339 timestamp of the next reset.
    pub resets_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct BudgetStatus {
    pub week: Option<PeriodStatus>,
    pub month: Option<PeriodStatus>,
    /// Smallest remaining amount over all active limits (None = no limits).
    pub remaining: Option<f64>,
    /// How many seconds of music `remaining` buys.
    pub remaining_seconds: Option<u32>,
}

/// Public status used by the kid's UI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AppStatus {
    pub setup_needed: bool,
    pub api_key_configured: bool,
    pub mock_mode: bool,
    pub generation_enabled: bool,
    pub max_track_seconds: u32,
    pub default_model: String,
    pub store_for_inpainting: bool,
    pub currency: String,
    pub price_per_minute: f64,
    pub budget: BudgetStatus,
    /// A generation is running right now.
    pub busy: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TrackSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub version_count: usize,
    pub latest_label: Option<String>,
    pub favorite_label: Option<String>,
    pub generating: bool,
    pub emoji: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VersionStatus {
    Generating,
    Ready,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct SongMetadata {
    pub title: Option<String>,
    pub description: Option<String>,
    pub genres: Vec<String>,
    pub languages: Vec<String>,
    pub is_explicit: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VersionInfo {
    pub id: String,
    pub parent_id: Option<String>,
    /// "v0", "v1", "v1.a", ...
    pub label: String,
    pub depth: u32,
    pub branch: String,
    pub name: String,
    pub notes: String,
    pub favorite: bool,
    pub created_at: String,
    pub status: VersionStatus,
    pub params: GenParams,
    pub duration_ms: u32,
    pub estimated_cost: f64,
    pub charged_cost: Option<f64>,
    pub measured_credits: Option<i64>,
    /// ElevenLabs song id (only when stored for inpainting).
    pub song_id: Option<String>,
    /// The plan ElevenLabs actually used (also for prompt-mode versions).
    pub plan: Option<Vec<PlanChunk>>,
    pub song_metadata: Option<SongMetadata>,
    pub audio_url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GenError {
    pub at: String,
    pub message: String,
    pub code: Option<String>,
    pub prompt_suggestion: Option<String>,
    pub plan_suggestion: Option<Vec<PlanChunk>>,
    /// What was attempted, so the kid can fix it and try again.
    pub params: GenParams,
    pub parent_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TrackDetail {
    pub id: String,
    pub title: String,
    pub emoji: String,
    pub created_at: String,
    pub updated_at: String,
    pub versions: Vec<VersionInfo>,
    pub last_error: Option<GenError>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CreateTrackReq {
    pub title: String,
    #[serde(default)]
    pub emoji: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UpdateTrackReq {
    pub title: Option<String>,
    pub emoji: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GenerateReq {
    pub parent_id: Option<String>,
    pub params: GenParams,
    #[serde(default)]
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct UpdateVersionReq {
    pub name: Option<String>,
    pub notes: Option<String>,
    pub favorite: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlanReq {
    pub prompt: String,
    pub music_length_ms: u32,
    /// Existing plan to change ("make the chorus happier").
    pub source: Option<Vec<PlanChunk>>,
    pub model_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlanResp {
    pub chunks: Vec<PlanChunk>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Finetune {
    pub id: String,
    pub name: String,
    pub primary_genre: String,
    /// Model the finetune was trained for; it only works with that model.
    #[serde(default)]
    pub model_id: String,
}

/// Error body returned by every failing endpoint.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ApiError {
    pub error: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub prompt_suggestion: Option<String>,
    #[serde(default)]
    pub plan_suggestion: Option<Vec<PlanChunk>>,
}

// ---------------- parent / admin ----------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SetupReq {
    pub password: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub settings: Option<Settings>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoginReq {
    pub password: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoginResp {
    pub token: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AdminView {
    pub settings: Settings,
    pub api_key_masked: Option<String>,
    pub api_key_from_env: bool,
    pub mock_mode: bool,
    pub data_dir: String,
    pub budget: BudgetStatus,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApiKeyReq {
    pub api_key: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ApiKeyResp {
    pub api_key_masked: String,
    /// Result of a test call against ElevenLabs.
    pub check: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PasswordReq {
    pub current: String,
    pub new: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AccountInfo {
    pub tier: String,
    pub status: String,
    pub credits_used: i64,
    pub credits_limit: i64,
    pub next_reset: Option<String>,
    pub currency: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LedgerKind {
    Generation,
    Adjustment,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LedgerEntry {
    pub id: String,
    pub at: String,
    pub kind: LedgerKind,
    pub track_title: String,
    pub version_label: String,
    pub seconds: f64,
    pub estimated_cost: f64,
    pub measured_credits: Option<i64>,
    /// What was counted against the limits.
    pub cost: f64,
    pub note: String,
    /// Still recording: counted as "reserved" until the song is finished.
    #[serde(default)]
    pub pending: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AdjustReq {
    /// Positive = extra charge, negative = gift budget back.
    pub amount: f64,
    pub note: String,
}
