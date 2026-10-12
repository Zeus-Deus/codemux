//! Repeatable browser-backend benchmark with real Chromium and semantic assertions.
//! cargo run -j 2 --manifest-path scripts/browser-benchmark/Cargo.toml -- results.json
//! Measures the production action handlers, including observation/verification;
//! excludes model inference, the outer CLI/MCP transport, and browser startup.

#![allow(dead_code)]

// Compile the production modules directly so running this benchmark does not
// require building or starting the desktop app or touching its live sessions.
#[path = "../../src-tauri/src/agent_browser.rs"]
mod agent_browser;
#[path = "../../src-tauri/src/browser_viewport.rs"]
mod browser_viewport;
#[path = "../../src-tauri/src/stream_input.rs"]
mod stream_input;

// Keep the fixture viewport independent of the user's synced settings.
mod settings_sync {
    pub struct Browser {
        pub default_viewport: Option<String>,
    }
    pub struct Cache {
        pub browser: Browser,
    }
    pub fn load_cache() -> Option<Cache> {
        None
    }
}
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>Browser benchmark</title>
<style>body{font:18px sans-serif;margin:24px}input,textarea,button{display:block;margin:12px;padding:10px}textarea{width:650px;height:90px}</style>
<form id="form"><input id="name" aria-label="Name"><input id="code" aria-label="Code"><button id="submit">Submit form</button></form>
<textarea id="text" aria-label="Text"></textarea><button id="counter">Count click</button><output id="result">Ready</output>
<script>
window.state={clicks:0,inputs:0,keys:0,submissions:0,trusted:true};
text.addEventListener('input',e=>{state.inputs++;state.trusted&&=e.isTrusted});
text.addEventListener('keydown',()=>state.keys++);
counter.onclick=e=>{state.clicks++;state.trusted&&=e.isTrusted};
form.onsubmit=e=>{e.preventDefault();state.submissions++;state.name=document.querySelector('#name').value;state.code=code.value;result.textContent=JSON.stringify(state)};
</script>"#;

async fn action(session: &str, port: u16, kind: &str, params: Value) -> Value {
    let session = session.to_owned();
    let kind = kind.to_owned();
    tokio::task::spawn_blocking(move || {
        agent_browser::run_cli_action(&session, &kind, params, port)
            .expect("browser action failed")
            .data
    })
    .await
    .expect("browser task failed")
}

fn decode_result(data: Value) -> Value {
    let value = data
        .get("data")
        .and_then(|v| v.get("result"))
        .or_else(|| data.get("result"))
        .unwrap_or(&data);
    match value.as_str() {
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| value.clone()),
        None => value.clone(),
    }
}

async fn read_state(session: &str, port: u16) -> Value {
    decode_result(action(session, port, "eval", json!({"script":
        "JSON.stringify({...window.state,text:document.querySelector('#text').value,result:document.querySelector('#result').textContent})"})).await)
}

async fn expect_state(session: &str, port: u16, expected: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let actual = read_state(session, port).await;
        if expected
            .as_object()
            .unwrap()
            .iter()
            .all(|(k, v)| actual.get(k) == Some(v))
        {
            return json!({"passed":true,"actual":actual});
        }
        if Instant::now() >= deadline {
            return json!({"passed":false,"actual":actual,"expected":expected});
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn center(session: &str, port: u16, selector: &str) -> (f64, f64) {
    let script = format!("JSON.stringify((()=>{{const r=document.querySelector({}).getBoundingClientRect();return [r.x+r.width/2,r.y+r.height/2]}})())", json!(selector));
    let v = decode_result(action(session, port, "eval", json!({"script":script})).await);
    (v[0].as_f64().unwrap(), v[1].as_f64().unwrap())
}

async fn task(session: &str, port: u16, name: &str) -> Value {
    action(session, port, "eval", json!({"script": "window.state={clicks:0,inputs:0,keys:0,submissions:0,trusted:true};document.querySelector('#form').reset();document.querySelector('#text').value='';document.querySelector('#result').textContent='Ready';document.activeElement.blur();true"})).await;
    let text = "Browser benchmark: exact input, punctuation & symbols! ".repeat(4);
    let (x, y) = center(
        session,
        port,
        if name == "coordinate_clicks" {
            "#counter"
        } else {
            "#text"
        },
    )
    .await;
    let start = Instant::now();
    let actual = match name {
        "structured_form" => {
            let snapshot = action(session, port, "snapshot", json!({})).await;
            assert!(snapshot.to_string().contains("Submit form"));
            action(
                session,
                port,
                "fill",
                json!({"selector":"#name","value":"Zoë Example"}),
            )
            .await;
            action(
                session,
                port,
                "fill",
                json!({"selector":"#code","value":"A-42 & 'quoted'"}),
            )
            .await;
            action(session, port, "click", json!({"selector":"#submit"})).await;
            let snapshot = action(session, port, "snapshot", json!({})).await;
            assert!(snapshot.to_string().contains("Submit form"));
            expect_state(
                session,
                port,
                &json!({"name":"Zoë Example","code":"A-42 & 'quoted'","submissions":1}),
            )
            .await
        }
        "coordinate_text" | "unicode_text" | "keyboard_controls" => {
            let text = if name == "unicode_text" {
                "Zoë — café 日本語 🚀\nSecond line".repeat(3)
            } else if name == "keyboard_controls" {
                "first line\nsecond line".into()
            } else {
                text
            };
            stream_input::type_at(port, &text, Some(x), Some(y))
                .await
                .unwrap();
            if name == "keyboard_controls" {
                stream_input::key_press(port, "Tab").await.unwrap();
                stream_input::key_press(port, "Enter").await.unwrap();
            }
            expect_state(session, port, &json!({"text":text,"trusted":true,"clicks":if name == "keyboard_controls" {1} else {0}})).await
        }
        "coordinate_clicks" => {
            for _ in 0..12 {
                stream_input::click_at(port, x, y, "left").await.unwrap();
            }
            expect_state(session, port, &json!({"clicks":12,"trusted":true})).await
        }
        "screenshot" => {
            action(
                session,
                port,
                "fill",
                json!({"selector":"#text","value":"Screenshot correctness fixture"}),
            )
            .await;
            let manager = agent_browser::AgentBrowserManager::new();
            let image = manager.get_screenshot(session).await.unwrap();
            assert!(image.len() > 2000, "empty screenshot");
            assert!(
                image.starts_with("data:image/png;base64,iVBOR"),
                "not a PNG: {}",
                &image[..image.len().min(60)]
            );
            expect_state(
                session,
                port,
                &json!({"text":"Screenshot correctness fixture"}),
            )
            .await
        }
        _ => unreachable!(),
    };
    json!({"task":name,"elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"passed":actual["passed"],"state":actual["actual"],"expected":actual.get("expected")})
}

#[tokio::main]
async fn main() {
    let output = std::env::args()
        .nth(1)
        .expect("usage: browser_benchmark <results.json>");
    let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", server.local_addr().unwrap());
    let server_task = tokio::spawn(async move {
        axum::serve(server, axum::Router::new()
            .route("/", axum::routing::get(|| async { axum::response::Html(PAGE) }))
            .route("/slow", axum::routing::get(|| async { axum::response::Html(format!("{PAGE}<script src='/slow.js'></script><script>window.addEventListener('load',()=>window.loaded=true)</script>")) }))
            .route("/slow.js", axum::routing::get(|| async {
                tokio::time::sleep(Duration::from_millis(250)).await;
                ([("content-type", "application/javascript")], "window.slowReady=true;")
            }))).await.unwrap();
    });
    let port_probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = port_probe.local_addr().unwrap().port();
    drop(port_probe);
    let session = format!("codemux-benchmark-{}", std::process::id());
    let navigation_start = Instant::now();
    let navigation = action(&session, port, "open", json!({"url":url})).await;
    let navigation_ms = navigation_start.elapsed().as_secs_f64() * 1000.0;
    eprintln!("initial navigation: {navigation_ms:.1} ms, response: {navigation}");
    let tasks = [
        "structured_form",
        "coordinate_text",
        "unicode_text",
        "keyboard_controls",
        "coordinate_clicks",
        "screenshot",
    ];
    let mut samples = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        action(&session, port, "open", json!({"url":format!("{url}slow")})).await;
        let actual = decode_result(action(&session, port, "eval", json!({"script":"JSON.stringify({ready:document.readyState,slowReady:window.slowReady,loaded:window.loaded})"})).await);
        let passed = actual == json!({"ready":"complete","slowReady":true,"loaded":true});
        let sample = json!({"task":"navigation","elapsed_ms":start.elapsed().as_secs_f64()*1000.0,"passed":passed,"state":actual});
        eprintln!("navigation: {sample}");
        samples.push(sample);
    }
    for round in 0..8 {
        for name in tasks {
            let sample = task(&session, port, name).await;
            eprintln!(
                "round {round} {name}: {:.1} ms, passed={}",
                sample["elapsed_ms"].as_f64().unwrap(),
                sample["passed"]
            );
            if round > 0 {
                samples.push(sample);
            }
        }
    }
    let mut summaries = Vec::new();
    for name in std::iter::once("navigation").chain(tasks) {
        let mut times: Vec<_> = samples
            .iter()
            .filter(|s| s["task"] == name)
            .map(|s| s["elapsed_ms"].as_f64().unwrap())
            .collect();
        times.sort_by(f64::total_cmp);
        let passed = samples
            .iter()
            .filter(|s| s["task"] == name && s["passed"] == true)
            .count();
        let summary = json!({"task":name,"runs":times.len(),"passed":passed,"median_ms":times[times.len()/2],"min_ms":times[0],"max_ms":times[times.len()-1]});
        println!("{summary}");
        summaries.push(summary);
    }
    std::fs::write(output, serde_json::to_vec_pretty(&json!({"scope":"production browser backend; real Chromium; local fixture; one warmup and seven measured runs per task; model inference and outer CLI/MCP transport excluded","initial_navigation_ms":navigation_ms,"initial_navigation_response":navigation,"summaries":summaries,"samples":samples})).unwrap()).unwrap();
    agent_browser::AgentBrowserManager::new()
        .close(&session)
        .await
        .unwrap();
    server_task.abort();
    if samples.iter().any(|s| s["passed"] != true) {
        std::process::exit(1);
    }
}
