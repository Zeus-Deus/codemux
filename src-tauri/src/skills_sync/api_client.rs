// Skills sync — HTTP client for /api/skills.
//
// Mirrors `crate::settings_sync` for shape: free async functions
// that take a bearer token and return decoded payloads. The wire
// types match the Stage 1 server-side definition in
// `~/codemux-api/api/src/index.ts`. Field names are camelCase on
// the wire to match the Rust `SkillWire` shape the server expects.
//
// Errors are surfaced as `String` — the sync engine wraps them
// with context. Network failures vs HTTP-error-status are not
// distinguished at this layer; the caller's retry strategy is the
// same either way (mark dirty, retry on next sync trigger).

use serde::{Deserialize, Serialize};

use crate::auth::api_base_url;

/// Wire-format skill row, returned by GET / POST / PUT.
/// `remote_id` is a stringified BIGSERIAL (Stage 1 contract).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillWire {
    pub remote_id: String,
    pub encrypted_name: String,
    pub nonce_name: String,
    pub encrypted_content: String,
    pub nonce_content: String,
    pub provider: String,
    pub scope: String,
    pub updated_at: String,
}

/// Request body for POST /api/skills and PUT /api/skills/:id.
/// No `remote_id` — the server assigns it on POST and pulls it
/// from the URL on PUT.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillUpload {
    pub encrypted_name: String,
    pub nonce_name: String,
    pub encrypted_content: String,
    pub nonce_content: String,
    pub provider: String,
    pub scope: String,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    skills: Vec<SkillWire>,
}

#[derive(Debug, Deserialize)]
struct CreateUpdateResponse {
    skill: SkillWire,
}

/// `GET /api/skills` — list every encrypted skill the user has
/// synced, newest-updated first. Returned ciphertext is opaque to
/// this layer; the engine decrypts on the way through.
pub async fn list_skills(token: &str) -> Result<Vec<SkillWire>, String> {
    let base = api_base_url();
    let resp = reqwest::Client::new()
        .get(format!("{base}/api/skills"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("list_skills: HTTP {}", resp.status()));
    }
    let body: ListResponse = resp
        .json()
        .await
        .map_err(|e| format!("parse list_skills: {e}"))?;
    Ok(body.skills)
}

/// `POST /api/skills` — create a new skill. Returns the row with
/// the server-assigned `remote_id`.
pub async fn create_skill(
    token: &str,
    upload: &SkillUpload,
) -> Result<SkillWire, String> {
    let base = api_base_url();
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/skills"))
        .bearer_auth(token)
        .json(upload)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let msg = resp.text().await.unwrap_or_default();
        return Err(format!("create_skill: HTTP {status}: {msg}"));
    }
    let body: CreateUpdateResponse = resp
        .json()
        .await
        .map_err(|e| format!("parse create_skill: {e}"))?;
    Ok(body.skill)
}

/// `PUT /api/skills/:id` — replace the ciphertext + metadata for
/// an existing skill. 404 means "another device wiped this skill
/// from the server" (or the local mapping is stale); the caller
/// should drop the mapping entry and retry as a create.
pub async fn update_skill(
    token: &str,
    remote_id: &str,
    upload: &SkillUpload,
) -> Result<SkillWire, String> {
    let base = api_base_url();
    let resp = reqwest::Client::new()
        .put(format!("{base}/api/skills/{remote_id}"))
        .bearer_auth(token)
        .json(upload)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("update_skill({remote_id}): HTTP {}", resp.status()));
    }
    let body: CreateUpdateResponse = resp
        .json()
        .await
        .map_err(|e| format!("parse update_skill: {e}"))?;
    Ok(body.skill)
}

/// `DELETE /api/skills/:id` — drop one skill. 404 is treated as
/// success (idempotent intent).
pub async fn delete_skill(token: &str, remote_id: &str) -> Result<(), String> {
    let base = api_base_url();
    let resp = reqwest::Client::new()
        .delete(format!("{base}/api/skills/{remote_id}"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    let status = resp.status();
    if status.is_success() || status.as_u16() == 404 {
        return Ok(());
    }
    Err(format!("delete_skill({remote_id}): HTTP {status}"))
}

/// `DELETE /api/skills` — wipe every skill for the user. Used by
/// Stage 4's reset-sync flow. Idempotent — wiping an empty store
/// is fine.
pub async fn wipe_skills(token: &str) -> Result<(), String> {
    let base = api_base_url();
    let resp = reqwest::Client::new()
        .delete(format!("{base}/api/skills"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("wipe_skills: HTTP {}", resp.status()));
    }
    Ok(())
}
