// Stage 5 programmatic smoke. Drives the SyncEngine end-to-end
// against api.codemux.org with a fresh test user, exercising the
// full Stage 1-4 pipeline through the engine's actual public API
// (not the wire-level commands `skills_smoke` already covered).
//
// What this proves over `skills_smoke`:
//
//   - SyncEngine::sync_now() walks the syncable paths AND pushes
//     diffs (skills_smoke pushes one record directly).
//   - The mapping table at `<home>/.codemux/sync/skills-mapping.json`
//     gets created and updated correctly across multiple cycles.
//   - Pull writes to the canonical `<home>/.codemux/skills/<name>/SKILL.md`
//     destination on a "fresh" device (simulated via a tempdir
//     home).
//   - Edit-then-resync produces an update on the server, not a
//     duplicate.
//   - Local export → wipe server → import re-pushes the skills.
//
// Operator flow (same shape as Stage 1's `skills_smoke`):
//
//   1. Sign up + verify a smoke user out-of-band:
//        curl -X POST .../api/auth/desktop/signup -d '{...}'
//        ssh ... UPDATE "user" SET "emailVerified"=true WHERE email='smoke-stage5@example.test';
//   2. Run this binary; it derives the AuthSecret + key, signs
//      in, runs the cycles, prints PASS/FAIL on each step.
//   3. Cleanup the test user when done.
//
// Idempotent re-runs: if signin returns 401 the binary attempts
// signup with the derived AuthSecret as the password (same
// pattern as `skills_smoke`).

use std::fs;
use std::path::PathBuf;

use codemux_lib::auth::derive_auth_secret;
use codemux_lib::skills_sync::api_client;
use codemux_lib::skills_sync::export::{
    export_all_synced_skills, import_exported_skills,
};
use codemux_lib::skills_sync::SyncEngine;
use serde::{Deserialize, Serialize};

const DEFAULT_API_URL: &str = "https://api.codemux.org";

#[derive(Serialize)]
struct SignInBody {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct SignUpBody {
    email: String,
    password: String,
    name: String,
}

#[derive(Deserialize)]
struct SignInResponse {
    token: String,
}

fn step(label: &str) {
    println!("\n[stage5] ─── {label} ───");
}

fn ok(msg: impl AsRef<str>) {
    println!("[stage5] ✓ {}", msg.as_ref());
}

fn fail(msg: impl AsRef<str>) -> ! {
    eprintln!("[stage5] ✗ {}", msg.as_ref());
    std::process::exit(1);
}

#[tokio::main]
async fn main() {
    let api_url = std::env::var("API_URL").unwrap_or_else(|_| DEFAULT_API_URL.into());
    let email = std::env::var("SMOKE_EMAIL").expect("SMOKE_EMAIL must be set");
    let password = std::env::var("SMOKE_PASSWORD").expect("SMOKE_PASSWORD must be set");

    println!("[stage5] API_URL = {api_url}");
    println!("[stage5] email   = {email}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap();

    // ── Derivation + signin ──────────────────────────────────
    step("derive credentials + signin");
    let auth = derive_auth_secret(&password, &email)
        .unwrap_or_else(|e| fail(format!("derivation failed: {e}")));
    let auth_secret = auth.expose_for_external_signin().to_string();

    let bearer = signin_or_signup(&client, &api_url, &email, &auth_secret).await;
    ok(format!("signed in (bearer length {})", bearer.len()));

    // Wipe any leftover state from a previous run so the smoke is
    // hermetic. Server side: DELETE /api/skills. Returns 200 even
    // when empty.
    api_client::wipe_skills(&bearer)
        .await
        .unwrap_or_else(|e| fail(format!("pre-test wipe failed: {e}")));
    ok("server skills wiped (clean slate)");

    // ── Build engine on a tempdir home ───────────────────────
    step("set up engine in a tempdir home");
    let home = tempfile::tempdir().unwrap();
    let skills_root = home.path().join(".codemux/skills");
    fs::create_dir_all(&skills_root).unwrap();

    let engine = SyncEngine::with_home(home.path());

    let user_paths: Vec<PathBuf> = vec![skills_root.clone()];
    ok(format!("engine home={}", home.path().display()));

    // ── Cycle 1: create a skill locally + sync_now → push ────
    step("cycle 1: write skill, sync_now → push");
    let skill_dir = skills_root.join("stage5-smoke");
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(&skill_path, "# Stage 5 smoke\n\nfirst version\n").unwrap();

    let r1 = engine
        .sync_now(&bearer, user_paths.clone())
        .await
        .unwrap_or_else(|e| fail(format!("sync_now cycle 1: {e}")));
    if r1.pushed_count != 1 {
        fail(format!("cycle 1 expected pushed_count=1, got {r1:?}"));
    }
    ok(format!(
        "cycle 1 pushed {}, pulled {}",
        r1.pushed_count, r1.pulled_count
    ));

    // Mapping should now have the new entry.
    let mapping_path = home.path().join(".codemux/sync/skills-mapping.json");
    let mapping_json = fs::read_to_string(&mapping_path)
        .unwrap_or_else(|e| fail(format!("read mapping: {e}")));
    if !mapping_json.contains("stage5-smoke") {
        fail("mapping missing the just-pushed skill");
    }
    ok("mapping records the skill with a remote_id");

    // ── Cycle 2: edit + sync_now → update (no duplicate) ────
    step("cycle 2: edit skill, sync_now → update");
    fs::write(
        &skill_path,
        "# Stage 5 smoke\n\nedited version with more body\n",
    )
    .unwrap();
    let r2 = engine
        .sync_now(&bearer, user_paths.clone())
        .await
        .unwrap_or_else(|e| fail(format!("sync_now cycle 2: {e}")));
    if r2.pushed_count != 1 {
        fail(format!("cycle 2 expected pushed_count=1, got {r2:?}"));
    }
    ok("cycle 2 pushed the update");

    // Server should still have exactly 1 skill (PUT not POST).
    let listed = api_client::list_skills(&bearer)
        .await
        .unwrap_or_else(|e| fail(format!("list_skills: {e}")));
    if listed.len() != 1 {
        fail(format!("expected 1 skill on server, got {}", listed.len()));
    }
    ok("server has exactly 1 skill (no duplicate)");

    // ── Cycle 3: idempotent sync_now is a no-op ─────────────
    step("cycle 3: sync_now with no local changes → no-op");
    let r3 = engine
        .sync_now(&bearer, user_paths.clone())
        .await
        .unwrap_or_else(|e| fail(format!("sync_now cycle 3: {e}")));
    if r3.pushed_count != 0 {
        fail(format!("cycle 3 expected pushed_count=0, got {r3:?}"));
    }
    ok("cycle 3 was a no-op");

    // ── Pull on a fresh-home engine writes to ~/.codemux/skills/ ─
    step("pull on a fresh-home engine writes to canonical path");
    let fresh_home = tempfile::tempdir().unwrap();
    let fresh_engine = SyncEngine::with_home(fresh_home.path());
    let fresh_paths: Vec<PathBuf> = vec![fresh_home.path().join(".codemux/skills")];

    let r4 = fresh_engine
        .sync_now(&bearer, fresh_paths.clone())
        .await
        .unwrap_or_else(|e| fail(format!("fresh sync_now: {e}")));
    if r4.pulled_count != 1 {
        fail(format!("expected pulled_count=1 on fresh home, got {r4:?}"));
    }
    let dest = fresh_home
        .path()
        .join(".codemux/skills/stage5-smoke/SKILL.md");
    if !dest.exists() {
        fail(format!(
            "expected pulled skill at {}, file missing",
            dest.display()
        ));
    }
    let content = fs::read_to_string(&dest).unwrap();
    if !content.contains("edited version with more body") {
        fail("pulled content doesn't match the latest server version");
    }
    ok(format!(
        "fresh-home pull created {} with the latest content",
        dest.display()
    ));

    // ── Export → wipe local → import → re-push ──────────────
    step("export → wipe local → import → re-push");
    let export_path = home.path().join("stage5-export.json");
    let exported = export_all_synced_skills(&bearer, &export_path, &email)
        .await
        .unwrap_or_else(|e| fail(format!("export: {e}")));
    if exported.skill_count != 1 {
        fail(format!("expected 1 skill in export, got {exported:?}"));
    }
    ok(format!(
        "export wrote {} skills, {} bytes to {}",
        exported.skill_count,
        exported.bytes_written,
        exported.path.display()
    ));

    // Wipe the server side as the reset flow would, then import
    // the local export to repopulate.
    api_client::wipe_skills(&bearer)
        .await
        .unwrap_or_else(|e| fail(format!("post-export wipe: {e}")));
    let listed_after_wipe = api_client::list_skills(&bearer)
        .await
        .unwrap_or_else(|e| fail(format!("list after wipe: {e}")));
    if !listed_after_wipe.is_empty() {
        fail("server still has skills after wipe");
    }
    ok("server wiped to empty");

    let imported = import_exported_skills(&bearer, &export_path, &email)
        .await
        .unwrap_or_else(|e| fail(format!("import: {e}")));
    if imported.queued_count != 1 || imported.failed_count != 0 || imported.mismatched_email {
        fail(format!("unexpected import result: {imported:?}"));
    }
    ok(format!(
        "import re-pushed {} skill(s) under the same key",
        imported.queued_count
    ));

    let listed_after_import = api_client::list_skills(&bearer)
        .await
        .unwrap_or_else(|e| fail(format!("list after import: {e}")));
    if listed_after_import.len() != 1 {
        fail(format!(
            "expected 1 skill after import, got {}",
            listed_after_import.len()
        ));
    }
    ok("server has 1 skill again");

    // ── Cleanup ─────────────────────────────────────────────
    step("cleanup");
    api_client::wipe_skills(&bearer)
        .await
        .unwrap_or_else(|e| fail(format!("final cleanup wipe: {e}")));
    ok("server skills wiped");

    println!("\n[stage5] PASS — full engine roundtrip verified");
}

async fn signin_or_signup(
    client: &reqwest::Client,
    api_url: &str,
    email: &str,
    auth_secret: &str,
) -> String {
    let signin_url = format!("{api_url}/api/auth/desktop/signin");
    let resp = client
        .post(&signin_url)
        .json(&SignInBody {
            email: email.to_string(),
            password: auth_secret.to_string(),
        })
        .send()
        .await
        .unwrap_or_else(|e| fail(format!("signin: {e}")));
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    if status.is_success() {
        let parsed: SignInResponse = serde_json::from_str(&text)
            .unwrap_or_else(|e| fail(format!("signin response shape: {e}")));
        return parsed.token;
    }
    if status.as_u16() != 401 {
        fail(format!("signin failed ({status}): {text}"));
    }

    eprintln!("[stage5] signin 401 — attempting signup");
    let signup_url = format!("{api_url}/api/auth/desktop/signup");
    let signup_resp = client
        .post(&signup_url)
        .json(&SignUpBody {
            email: email.to_string(),
            password: auth_secret.to_string(),
            name: "Stage 5 Smoke".to_string(),
        })
        .send()
        .await
        .unwrap_or_else(|e| fail(format!("signup: {e}")));
    let signup_status = signup_resp.status();
    let signup_text = signup_resp.text().await.unwrap_or_default();
    if !signup_status.is_success() {
        fail(format!("signup failed ({signup_status}): {signup_text}"));
    }
    eprintln!(
        "\n[stage5] signup ok. The user is unverified — Better Auth blocks signin\n\
         until emailVerified=true. Run this on the VPS, then re-run the smoke:\n\n  \
         ssh work@78.47.192.173 \"cd ~/codemux-api && docker compose exec -T postgres \\\n    \
           psql -U codemux -d codemux -c \\\"UPDATE \\\\\\\"user\\\\\\\" SET \\\\\\\"emailVerified\\\\\\\" = true WHERE email = '{email}';\\\"\"\n"
    );
    std::process::exit(2);
}
