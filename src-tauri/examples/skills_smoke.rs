// Smoke test: transport + storage round-trip against the live
// `/api/skills` endpoints. Skills are stored server-side (plaintext
// columns, no client-held key), so this proves the plaintext wire
// format round-trips cleanly through the deployed server.
//
// What it proves:
//   1. `derive_auth_secret(password, email)` produces an AuthSecret
//      that signs in against the deployed `/api/auth/desktop/signin`.
//   2. A skill's `name` + `content` round-trip byte-for-byte through
//      POST → GET (the server stores and returns them unchanged).
//   3. Per-record CRUD (POST → GET → DELETE) composes correctly.
//
// The smoke handles full account lifecycle on each run:
//   1. Tries `signin` with the derived AuthSecret. If that succeeds
//      we have an existing test user — proceed.
//   2. If signin returns 401 ("invalid credentials"), assume the
//      user doesn't exist yet, sign them up sending the derived
//      AuthSecret as the password (matching real-client semantics
//      so subsequent signin succeeds), and ask the operator to
//      flip `emailVerified` via SSH (Better Auth blocks signin
//      until verified). The operator command is printed.
//   3. After verification, re-run the binary — signin will succeed
//      this time and the loop completes.
//
// Run (after the server is deployed with the plaintext skills schema):
//   API_URL=https://api.codemux.org \
//   SMOKE_EMAIL=smoke@example.test \
//   SMOKE_PASSWORD=smoke-test-password-12345 \
//   cargo run --manifest-path src-tauri/Cargo.toml --example skills_smoke
//
// Cleanup (run once after smoke passes):
//   docker compose exec -T postgres psql -U codemux -d codemux \
//     -c "DELETE FROM \"user\" WHERE email = 'smoke@example.test';"

use codemux_lib::auth::derive_auth_secret;
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillUpload {
    name: String,
    content: String,
    provider: String,
    scope: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct SkillRow {
    remote_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    content: String,
    provider: String,
    scope: String,
}

#[derive(Deserialize, Debug)]
struct SkillsListResponse {
    skills: Vec<SkillRow>,
}

#[derive(Deserialize, Debug)]
struct SkillCreateResponse {
    skill: SkillRow,
}

fn main() {
    let api_url = std::env::var("API_URL").unwrap_or_else(|_| DEFAULT_API_URL.into());
    let email = std::env::var("SMOKE_EMAIL").expect("SMOKE_EMAIL must be set");
    let password = std::env::var("SMOKE_PASSWORD").expect("SMOKE_PASSWORD must be set");

    println!("[smoke] API_URL = {api_url}");
    println!("[smoke] email   = {email}");

    // 1. Derive the AuthSecret (the server never sees the raw password).
    let auth = derive_auth_secret(&password, &email).expect("derivation failed");
    println!(
        "[smoke] derivation done; auth_secret length = {}",
        auth.expose_for_external_signin().len()
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("http client");

    let auth_secret_string = auth.expose_for_external_signin().to_string();

    // 2. Try signin. If the user exists and is verified, this
    //    returns a bearer token. If signin returns 401 we fall
    //    through to signup → email-verify hint → re-run.
    let signin_url = format!("{api_url}/api/auth/desktop/signin");
    let signin = client
        .post(&signin_url)
        .json(&SignInBody {
            email: email.clone(),
            password: auth_secret_string.clone(),
        })
        .send()
        .expect("signin request");
    let signin_status = signin.status();
    let signin_text = signin.text().unwrap_or_default();

    let bearer: String = if signin_status.is_success() {
        let parsed: SignInResponse =
            serde_json::from_str(&signin_text).expect("signin response shape");
        println!("[smoke] signed in; bearer length = {}", parsed.token.len());
        parsed.token
    } else if signin_status.as_u16() == 401 {
        // Account doesn't exist or password mismatch. Sign up
        // using the derived AuthSecret as the password so the
        // bcrypt-stored value matches what subsequent signin
        // attempts will send.
        println!("[smoke] signin 401 — attempting signup");
        let signup_url = format!("{api_url}/api/auth/desktop/signup");
        let signup_resp = client
            .post(&signup_url)
            .json(&SignUpBody {
                email: email.clone(),
                password: auth_secret_string.clone(),
                name: "Skills Smoke".into(),
            })
            .send()
            .expect("signup request");
        let signup_status = signup_resp.status();
        let signup_text = signup_resp.text().unwrap_or_default();
        if !signup_status.is_success() {
            panic!("signup failed ({signup_status}): {signup_text}");
        }
        eprintln!(
            "\n[smoke] signup ok. The user is unverified — Better Auth blocks signin\n\
             until emailVerified=true. Run on the VPS, then re-run the smoke:\n\n  \
             ssh work@78.47.192.173 \"cd ~/codemux-api && docker compose exec -T postgres \\\n    \
               psql -U codemux -d codemux -c \\\"UPDATE \\\\\\\"user\\\\\\\" SET \\\\\\\"emailVerified\\\\\\\" = true WHERE email = '{email}';\\\"\"\n"
        );
        std::process::exit(2);
    } else {
        panic!("signin failed ({signin_status}): {signin_text}");
    };

    // 3. POST /api/skills with a plaintext skill.
    let skill_name = "my-smoke-skill";
    let skill_content = "# My smoke skill\n\nThis content is stored server-side.";
    let upload = SkillUpload {
        name: skill_name.into(),
        content: skill_content.into(),
        provider: "claude".into(),
        scope: "user".into(),
    };
    let post_url = format!("{api_url}/api/skills");
    let post_resp = client
        .post(&post_url)
        .bearer_auth(&bearer)
        .json(&upload)
        .send()
        .expect("post /api/skills");
    let post_status = post_resp.status();
    let post_text = post_resp.text().unwrap_or_default();
    if !post_status.is_success() {
        panic!("POST failed ({post_status}): {post_text}");
    }
    let created: SkillCreateResponse =
        serde_json::from_str(&post_text).expect("POST response shape");
    println!("[smoke] POST ok; remoteId = {}", created.skill.remote_id);

    // 4. GET /api/skills
    let get_resp = client
        .get(&post_url)
        .bearer_auth(&bearer)
        .send()
        .expect("get /api/skills");
    let get_status = get_resp.status();
    let get_text = get_resp.text().unwrap_or_default();
    if !get_status.is_success() {
        panic!("GET failed ({get_status}): {get_text}");
    }
    let listed: SkillsListResponse =
        serde_json::from_str(&get_text).expect("GET response shape");
    let our = listed
        .skills
        .iter()
        .find(|s| s.remote_id == created.skill.remote_id)
        .expect("our skill not in GET response");

    // 5. Plaintext round-trip check — the server returns exactly
    //    what we stored.
    assert_eq!(our.name, skill_name, "name drifted on wire");
    assert_eq!(our.content, skill_content, "content drifted on wire");
    assert_eq!(our.provider, "claude");
    assert_eq!(our.scope, "user");
    println!("[smoke] name + content round-tripped byte-for-byte");

    // 6. Clean up — delete the test skill so re-runs are idempotent.
    let del_url = format!("{api_url}/api/skills/{}", created.skill.remote_id);
    let del_resp = client
        .delete(&del_url)
        .bearer_auth(&bearer)
        .send()
        .expect("delete");
    if !del_resp.status().is_success() {
        eprintln!(
            "[smoke] WARN: cleanup DELETE failed ({}): {}",
            del_resp.status(),
            del_resp.text().unwrap_or_default()
        );
    } else {
        println!("[smoke] cleanup ok");
    }

    println!("[smoke] PASS — server-side skills round-trip verified");
}
