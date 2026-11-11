//! Thin client for the Musik Faktory server API.

use gloo_net::http::{Request, RequestBuilder};
use musik_shared::*;
use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ApiErr {
    pub status: u16,
    pub message: String,
    pub code: Option<String>,
    pub prompt_suggestion: Option<String>,
    pub plan_suggestion: Option<Vec<PlanChunk>>,
}

impl ApiErr {
    pub fn unauthorized(&self) -> bool {
        self.status == 401
    }
}

fn builder(method: &str, path: &str) -> RequestBuilder {
    match method {
        "POST" => Request::post(path),
        "PUT" => Request::put(path),
        "PATCH" => Request::patch(path),
        "DELETE" => Request::delete(path),
        _ => Request::get(path),
    }
}

async fn call<B: Serialize, T: DeserializeOwned>(method: &str, path: &str, body: Option<&B>, token: Option<&str>) -> Result<T, ApiErr> {
    let mut b = builder(method, path);
    if let Some(t) = token {
        b = b.header("Authorization", &format!("Bearer {t}"));
    }
    let sent = match body {
        Some(body) => match b.json(body) {
            Ok(req) => req.send().await,
            Err(e) => return Err(ApiErr { message: e.to_string(), ..Default::default() }),
        },
        None => b.send().await,
    };
    let resp = sent.map_err(|e| ApiErr {
        message: format!("Can't reach the Musik Faktory server. Is it running? ({e})"),
        ..Default::default()
    })?;
    let status = resp.status();
    if resp.ok() {
        return resp.json::<T>().await.map_err(|e| ApiErr { status, message: format!("Unexpected answer: {e}"), ..Default::default() });
    }
    let text = resp.text().await.unwrap_or_default();
    Err(match serde_json::from_str::<ApiError>(&text) {
        Ok(e) => ApiErr {
            status,
            message: e.error,
            code: e.code,
            prompt_suggestion: e.prompt_suggestion,
            plan_suggestion: e.plan_suggestion,
        },
        Err(_) => ApiErr {
            status,
            message: if text.is_empty() { format!("Server error {status}") } else { text },
            ..Default::default()
        },
    })
}

async fn get<T: DeserializeOwned>(path: &str, token: Option<&str>) -> Result<T, ApiErr> {
    call::<(), T>("GET", path, None, token).await
}

// ---------------- kid API

pub async fn status() -> Result<AppStatus, ApiErr> {
    get("/api/status", None).await
}
pub async fn tracks() -> Result<Vec<TrackSummary>, ApiErr> {
    get("/api/tracks", None).await
}
pub async fn create_track(title: String) -> Result<TrackDetail, ApiErr> {
    call("POST", "/api/tracks", Some(&CreateTrackReq { title, emoji: String::new() }), None).await
}
pub async fn get_track(id: &str) -> Result<TrackDetail, ApiErr> {
    get(&format!("/api/tracks/{id}"), None).await
}
pub async fn update_track(id: &str, req: UpdateTrackReq) -> Result<TrackDetail, ApiErr> {
    call("PATCH", &format!("/api/tracks/{id}"), Some(&req), None).await
}
pub async fn delete_track(id: &str) -> Result<serde_json::Value, ApiErr> {
    call::<(), _>("DELETE", &format!("/api/tracks/{id}"), None, None).await
}
pub async fn dismiss_error(id: &str) -> Result<TrackDetail, ApiErr> {
    call::<(), _>("DELETE", &format!("/api/tracks/{id}/error"), None, None).await
}
pub async fn generate(track_id: &str, req: &GenerateReq) -> Result<VersionInfo, ApiErr> {
    call("POST", &format!("/api/tracks/{track_id}/versions"), Some(req), None).await
}
pub async fn update_version(id: &str, req: UpdateVersionReq) -> Result<VersionInfo, ApiErr> {
    call("PATCH", &format!("/api/versions/{id}"), Some(&req), None).await
}
pub async fn delete_version(id: &str) -> Result<serde_json::Value, ApiErr> {
    call::<(), _>("DELETE", &format!("/api/versions/{id}"), None, None).await
}
pub async fn plan(req: &PlanReq) -> Result<PlanResp, ApiErr> {
    call("POST", "/api/plan", Some(req), None).await
}
pub async fn finetunes() -> Result<Vec<Finetune>, ApiErr> {
    get("/api/finetunes", None).await
}

// ---------------- parent API

pub async fn admin_setup(req: &SetupReq) -> Result<LoginResp, ApiErr> {
    call("POST", "/api/admin/setup", Some(req), None).await
}
pub async fn admin_login(password: String) -> Result<LoginResp, ApiErr> {
    call("POST", "/api/admin/login", Some(&LoginReq { password }), None).await
}
pub async fn admin_logout(token: &str) -> Result<serde_json::Value, ApiErr> {
    call::<(), _>("POST", "/api/admin/logout", None, Some(token)).await
}
pub async fn admin_view(token: &str) -> Result<AdminView, ApiErr> {
    get("/api/admin/settings", Some(token)).await
}
pub async fn admin_save_settings(token: &str, s: &Settings) -> Result<AdminView, ApiErr> {
    call("PUT", "/api/admin/settings", Some(s), Some(token)).await
}
pub async fn admin_set_key(token: &str, api_key: String) -> Result<ApiKeyResp, ApiErr> {
    call("PUT", "/api/admin/api-key", Some(&ApiKeyReq { api_key }), Some(token)).await
}
pub async fn admin_clear_key(token: &str) -> Result<AdminView, ApiErr> {
    call::<(), _>("DELETE", "/api/admin/api-key", None, Some(token)).await
}
pub async fn admin_password(token: &str, current: String, new: String) -> Result<LoginResp, ApiErr> {
    call("PUT", "/api/admin/password", Some(&PasswordReq { current, new }), Some(token)).await
}
pub async fn admin_account(token: &str) -> Result<AccountInfo, ApiErr> {
    get("/api/admin/account", Some(token)).await
}
pub async fn admin_ledger(token: &str) -> Result<Vec<LedgerEntry>, ApiErr> {
    get("/api/admin/ledger", Some(token)).await
}
pub async fn admin_adjust(token: &str, amount: f64, note: String) -> Result<Vec<LedgerEntry>, ApiErr> {
    call("POST", "/api/admin/ledger", Some(&AdjustReq { amount, note }), Some(token)).await
}
