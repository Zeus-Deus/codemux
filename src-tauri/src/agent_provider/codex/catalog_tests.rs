use super::super::session::managed_tests::{fixture_binary, wire};
use super::*;

#[tokio::test]
async fn chatgpt_catalog_invalidation_cannot_be_undone_by_old_inflight_harvest() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("catalog-block"), b"block").unwrap();
    let binary = fixture_binary(dir.path());
    let cache = Arc::new(CodexCapabilityCache::new());
    let work = cache.clone();
    let request = tokio::spawn(async move { work.get_or_harvest(&binary, None).await });
    for _ in 0..200 {
        if wire(dir.path()).iter().any(|v| v["rpc"] == "model/list") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(wire(dir.path()).iter().any(|v| v["rpc"] == "model/list"));
    cache.invalidate().await;
    std::fs::write(dir.path().join("catalog-release"), b"release").unwrap();
    let _ = request.await.unwrap();
    assert!(
        cache.inner.lock().await.is_none(),
        "an old in-flight catalog must not repopulate an invalidated cache"
    );
}

#[tokio::test]
async fn chatgpt_catalog_uses_managed_grant_all_pages_and_rotation_key() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("catalog-pages"), b"pages").unwrap();
    let binary = fixture_binary(dir.path());
    let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
    let cache = CodexCapabilityCache::new();
    let models = cache.get_or_harvest_owned(&binary, &owner).await.unwrap();
    let first = wire(dir.path())
        .into_iter()
        .find(|v| v.get("spawn").is_some())
        .unwrap();
    assert_eq!(
        first["token"], "synthetic-runtime-a",
        "managed catalog must not use the user's native CLI home/login"
    );
    assert_eq!(
        models
            .models
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        vec!["fixture-model", "fixture-next-model"]
    );
    assert_eq!(
        models.models[1].effort_descriptions["ultra"],
        "Provider-authored effort"
    );
    let pid = first["spawn"].as_u64().unwrap();
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
        "catalog child must be reaped before returning"
    );
    owner.testing_rotate().await;
    let _ = cache.get_or_harvest_owned(&binary, &owner).await.unwrap();
    let launches: Vec<_> = wire(dir.path())
        .into_iter()
        .filter(|v| v.get("spawn").is_some())
        .collect();
    assert_eq!(launches.len(),2,"rotation must invalidate the credential-bound catalog even without the renderer event subscription");
    assert_eq!(launches[1]["token"], "synthetic-runtime-b");
}
