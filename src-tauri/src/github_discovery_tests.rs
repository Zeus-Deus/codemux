use super::*;
use serde_json::{json, Value};

fn scope() -> DiscoveryScope {
    DiscoveryScope::from_selected("github.com", Some("fixture/app"), None, Some(1)).unwrap()
}

fn row(number: u32, branch: &str, state: &str, owner: &str) -> Value {
    json!({"number": number, "url": format!("https://github.com/fixture/app/pull/{number}"), "title": "fixture", "state": state, "headRefName": branch, "baseRefName": "main", "isDraft": false, "updatedAt": "2026-01-01T00:00:00Z", "createdAt": "2026-01-01T00:00:00Z", "headRefOid": "remote-newer-than-local", "headRepositoryOwner": {"login": owner}})
}

fn response(branches: &[String], rows: &HashMap<String, Vec<Value>>) -> String {
    let mut repository = serde_json::Map::new();
    for (index, branch) in branches.iter().enumerate() {
        let nodes = rows.get(branch).cloned().unwrap_or_default();
        repository.insert(
            format!("b{index}"),
            json!({"totalCount": nodes.len(), "pageInfo": {"hasNextPage": false}, "nodes": nodes}),
        );
    }
    json!({"data": {"repository": repository}}).to_string()
}

fn context(branch: &str, owned: &[&str]) -> WorkspaceDiscovery {
    WorkspaceDiscovery {
        path: PathBuf::from("/not-a-checkout"),
        scope: scope(),
        branch: branch.into(),
        default_branch: Some("main".into()),
        owned: owned.iter().map(|s| s.to_string()).collect(),
        config: RemoteConfig::default(),
    }
}

fn run(
    contexts: Vec<Result<WorkspaceDiscovery, String>>,
    rows: HashMap<String, Vec<Value>>,
) -> Vec<Result<Vec<SourcedPr>, String>> {
    get_workspace_prs_batch_with(contexts, |_, _, branches, _| {
        parse_discovery_response(&response(branches, &rows), branches)
    })
}

#[test]
fn discovery_requests_only_badge_and_association_fields() {
    for expensive in [
        "body",
        "reviewRequests",
        "latestReviews",
        "mergeable",
        "mergeStateStatus",
        "additions",
        "deletions",
        "changedFiles",
        "mergedBy",
    ] {
        assert!(
            !DISCOVERY_GRAPHQL_FIELDS
                .split_ascii_whitespace()
                .any(|field| field == expensive),
            "badge discovery must not request {expensive}"
        );
    }
    for required in [
        "number",
        "url",
        "state",
        "title",
        "headRefName",
        "baseRefName",
        "isDraft",
        "updatedAt",
        "headRefOid",
        "headRepositoryOwner",
    ] {
        assert!(DISCOVERY_GRAPHQL_FIELDS
            .split_ascii_whitespace()
            .any(|field| field == required));
    }
}

#[test]
fn siblings_share_one_query_and_keep_current_branch_history() {
    let mut calls = Vec::new();
    let rows = HashMap::from([
        ("a".into(), vec![row(42, "a", "MERGED", "fixture")]),
        ("b".into(), vec![row(43, "b", "CLOSED", "fixture")]),
    ]);
    let answers = get_workspace_prs_batch_with(
        vec![Ok(context("a", &[])), Ok(context("b", &[]))],
        |_, _, branches, open_only| {
            assert!(!open_only);
            calls.push(branches.to_vec());
            parse_discovery_response(&response(branches, &rows), branches)
        },
    );
    assert_eq!(calls, vec![vec!["a".to_string(), "b".to_string()]]);
    assert_eq!(answers[0].as_ref().unwrap()[0].pr.state, "MERGED");
    assert_eq!(answers[1].as_ref().unwrap()[0].pr.state, "CLOSED");
}

#[test]
fn duplicate_heads_share_one_connection_but_keep_workspace_provenance() {
    let mut calls = 0;
    let rows = HashMap::from([("shared".into(), vec![row(42, "shared", "OPEN", "fixture")])]);
    let answers = get_workspace_prs_batch_with(
        vec![Ok(context("shared", &[])), Ok(context("shared", &[]))],
        |_, _, branches, _| {
            calls += 1;
            assert_eq!(branches, ["shared"]);
            parse_discovery_response(&response(branches, &rows), branches)
        },
    );
    assert_eq!(calls, 1);
    assert_eq!(answers.len(), 2);
    for answer in answers {
        assert_eq!(
            answer.unwrap()[0].checkout_branch.as_deref(),
            Some("shared")
        );
    }
}

#[test]
fn grouping_separates_selected_host_repository_and_account() {
    let first = scope();
    let mut host = first.clone();
    host.host = "ghe.example".into();
    let mut repository = first.clone();
    repository.repository = "other".into();
    let mut account = first.clone();
    account.account = Some(2);
    let mut calls = std::collections::HashSet::new();
    let requests = [first.clone(), host, repository, account]
        .into_iter()
        .map(|s| (PathBuf::from("/fixture"), s, vec!["feature".to_string()]))
        .collect::<Vec<_>>();
    let rows = discover_grouped(&requests, false, &mut |_, scope, branches, _| {
        assert!(calls.insert(scope.clone()));
        Ok(branches
            .iter()
            .map(|branch| (branch.clone(), Ok(Vec::new())))
            .collect())
    });
    assert_eq!(calls.len(), 4);
    assert_eq!(rows.len(), 4);
}

#[test]
fn selected_repository_overrides_origin_including_enterprise_hosts() {
    let scope = DiscoveryScope::from_selected(
        "ghe.example",
        Some("ghe.example/Team/Selected"),
        Some("git@github.com:wrong/origin.git"),
        Some(9),
    )
    .unwrap();
    assert_eq!(
        (
            scope.host.as_str(),
            scope.owner.as_str(),
            scope.repository.as_str()
        ),
        ("ghe.example", "team", "selected")
    );
    let url = DiscoveryScope::from_selected(
        "ghe.example",
        None,
        Some("ssh://git@ghe.example/Team/Selected.git"),
        Some(9),
    )
    .unwrap();
    assert_eq!(scope, url);
    for invalid in [
        "owner/repo/extra/path",
        "owner/..",
        "owner/repo\n",
        "owner/",
    ] {
        assert!(DiscoveryScope::from_selected("github.com", Some(invalid), None, Some(1)).is_err());
    }
}

#[test]
fn request_chunks_are_bounded_and_deduplicated() {
    let names: Vec<_> = (0..35).map(|index| format!("branch/{index:02}")).collect();
    let requests = vec![
        (PathBuf::from("/one"), scope(), names.clone()),
        (PathBuf::from("/two"), scope(), names.clone()),
    ];
    let mut counts = Vec::new();
    let rows = discover_grouped(&requests, false, &mut |_, selected, branches, _| {
        assert!(branches.len() <= DISCOVERY_BRANCHES_PER_QUERY);
        counts.push(branches.len());
        let query = discovery_query(selected, branches, false);
        assert_eq!(query.matches("headRefName:").count(), branches.len());
        assert!(!query.contains("body"));
        Ok(branches
            .iter()
            .map(|branch| (branch.clone(), Ok(Vec::new())))
            .collect())
    });
    assert_eq!(counts, vec![16, 16, 3]);
    assert_eq!(rows.len(), names.len());
}

#[test]
fn repository_variables_bind_the_shared_quota_cache_scope() {
    let query = discovery_query(&scope(), &["feature".into()], false);
    assert!(query.contains("$owner:String!"));
    assert!(query.contains("$name:String!"));
    assert!(query.contains("repository(owner:$owner,name:$name)"));
}

#[test]
fn graphql_branch_names_are_escaped_not_interpolated() {
    let branches = vec!["feature/quote\"and\\slash".to_string()];
    let query = discovery_query(&scope(), &branches, false);
    assert!(query.contains(&format!(
        "headRefName:{}",
        serde_json::to_string(&branches[0]).unwrap()
    )));
    assert!(query.contains("states:[OPEN,CLOSED,MERGED]"));
    assert!(discovery_query(&scope(), &branches, true).contains("states:[OPEN]"));
}

#[test]
fn null_missing_error_and_truncated_connections_are_never_empty_answers() {
    let branches = vec!["feature".to_string()];
    for output in [
        "not json",
        r#"{"data":{"repository":null}}"#,
        r#"{"data":{"repository":{}},"errors":[{"message":"timeout"}]}"#,
    ] {
        assert!(parse_discovery_response(output, &branches).is_err());
    }
    for connection in [
        json!(null),
        json!({"nodes": []}),
        json!({"nodes": [], "totalCount": 0, "pageInfo": {}}),
        json!({"nodes": [], "totalCount": 1, "pageInfo": {"hasNextPage": false}}),
        json!({"nodes": [], "totalCount": 0, "pageInfo": {"hasNextPage": true}}),
    ] {
        let output = json!({"data": {"repository": {"b0": connection}}}).to_string();
        assert!(parse_discovery_response(&output, &branches).unwrap()["feature"].is_err());
    }
    assert!(
        parse_discovery_response(r#"{"data":{"repository":{}}}"#, &branches).unwrap()["feature"]
            .is_err()
    );
}

#[test]
fn invalid_or_duplicate_branch_rows_preserve_badges() {
    let branches = vec!["feature".to_string()];
    let mut malformed = row(1, "feature", "OPEN", "fixture");
    malformed["number"] = json!(0);
    for nodes in [
        vec![json!(null)],
        vec![malformed],
        vec![row(1, "wrong", "OPEN", "fixture")],
        vec![row(1, "feature", "INVALID", "fixture")],
        vec![
            row(1, "feature", "OPEN", "fixture"),
            row(1, "feature", "OPEN", "fixture"),
        ],
    ] {
        let rows = HashMap::from([("feature".into(), nodes)]);
        assert!(
            parse_discovery_response(&response(&branches, &rows), &branches).unwrap()["feature"]
                .is_err()
        );
    }
}

#[test]
fn full_branch_page_is_authoritative_only_when_explicitly_complete() {
    let branches = vec!["feature".to_string()];
    let rows = HashMap::from([(
        "feature".into(),
        (1..=100)
            .map(|number| row(number, "feature", "OPEN", "fixture"))
            .collect(),
    )]);
    assert_eq!(
        parse_discovery_response(&response(&branches, &rows), &branches).unwrap()["feature"]
            .as_ref()
            .unwrap()
            .len(),
        100
    );
    let mut value: Value = serde_json::from_str(&response(&branches, &rows)).unwrap();
    value["data"]["repository"]["b0"]["pageInfo"]["hasNextPage"] = json!(true);
    value["data"]["repository"]["b0"]["totalCount"] = json!(101);
    assert!(parse_discovery_response(&value.to_string(), &branches).unwrap()["feature"].is_err());
}

#[test]
fn incomplete_alias_preserves_only_its_affected_workspace() {
    let answers = get_workspace_prs_batch_with(
        vec![Ok(context("a", &[])), Ok(context("main", &[]))],
        |_, _, branches, _| {
            let mut value: Value =
                serde_json::from_str(&response(branches, &HashMap::new())).unwrap();
            let index = branches.iter().position(|branch| branch == "a").unwrap();
            value["data"]["repository"][format!("b{index}")]["pageInfo"]["hasNextPage"] =
                json!(true);
            parse_discovery_response(&value.to_string(), branches)
        },
    );
    assert!(matches!(
        workspace_prs_outcome(answers[0].clone()),
        WorkspacePrsOutcome::Preserve
    ));
    assert!(matches!(
        workspace_prs_outcome(answers[1].clone()),
        WorkspacePrsOutcome::Clear
    ));
}

#[test]
fn incomplete_stack_never_replaces_open_layers_with_merged_primary() {
    let answers = get_workspace_prs_batch_with(
        vec![Ok(context("workspace", &["stack/01"]))],
        |_, _, branches, _| {
            Ok(branches
                .iter()
                .map(|branch| {
                    (
                        branch.clone(),
                        if branch == "workspace" {
                            Ok(vec![row(10, branch, "MERGED", "fixture")])
                        } else {
                            Err("incomplete stack".into())
                        },
                    )
                })
                .collect())
        },
    );
    assert!(matches!(
        workspace_prs_outcome(answers.into_iter().next().unwrap()),
        WorkspacePrsOutcome::Preserve
    ));
}

#[test]
fn owned_stack_keeps_order_and_current_branch_primary() {
    let rows = HashMap::from([
        (
            "workspace".into(),
            vec![row(10, "workspace", "MERGED", "fixture")],
        ),
        (
            "stack/01".into(),
            vec![row(11, "stack/01", "MERGED", "fixture")],
        ),
        (
            "stack/02".into(),
            vec![row(12, "stack/02", "OPEN", "fixture")],
        ),
    ]);
    let prs = run(
        vec![Ok(context("workspace", &["stack/01", "stack/02"]))],
        rows,
    )
    .pop()
    .unwrap()
    .unwrap();
    assert_eq!(
        prs.iter().map(|entry| entry.pr.number).collect::<Vec<_>>(),
        vec![10, 11, 12]
    );
    assert_eq!(prs[0].source, PrSource::Branch);
    assert_eq!(prs[1].source, PrSource::Worktree);
    for pr in prs {
        assert_eq!(pr.checkout_branch.as_deref(), Some("workspace"));
    }
}

#[test]
fn fork_head_owner_filter_is_reused_in_batched_selection() {
    let mut context = context("shared", &[]);
    context.config = RemoteConfig::parse("remote.origin.url git@github.com:fixture/app.git\nremote.fork.url git@github.com:contributor/app.git\nbranch.shared.remote fork\n");
    let rows = HashMap::from([(
        "shared".into(),
        vec![
            row(20, "shared", "OPEN", "fixture"),
            row(19, "shared", "MERGED", "contributor"),
        ],
    )]);
    let prs = run(vec![Ok(context)], rows).pop().unwrap().unwrap();
    assert_eq!(prs[0].pr.number, 19);
}

#[test]
fn default_branch_skips_all_fallback_and_suppresses_history() {
    let answers = run(
        vec![Ok(context("main", &[]))],
        HashMap::from([("main".into(), vec![row(42, "main", "MERGED", "fixture")])]),
    );
    assert!(answers[0].as_ref().unwrap().is_empty());
}

#[test]
fn detached_and_failed_contexts_preserve_without_a_query() {
    let answers = get_workspace_prs_batch_with(vec![Err("detached HEAD".into())], |_, _, _, _| {
        panic!("unanswerable context must not query")
    });
    assert!(matches!(
        workspace_prs_outcome(answers[0].clone()),
        WorkspacePrsOutcome::Preserve
    ));
}

#[test]
fn failed_transport_preserves_all_siblings() {
    let answers = get_workspace_prs_batch_with(
        vec![Ok(context("a", &[])), Ok(context("b", &[]))],
        |_, _, _, _| Err("offline".into()),
    );
    for answer in answers {
        assert!(matches!(
            workspace_prs_outcome(answer),
            WorkspacePrsOutcome::Preserve
        ));
    }
}

#[test]
fn stale_workspace_path_or_deletion_rejects_old_poll_result() {
    let targets = vec![("workspace".into(), "/new/path".into())];
    assert!(!workspace_pr_poll_target_is_current(
        &targets,
        "workspace",
        "/old/path"
    ));
    assert!(!workspace_pr_poll_target_is_current(
        &targets,
        "deleted",
        "/new/path"
    ));
    assert!(workspace_pr_poll_target_is_current(
        &targets,
        "workspace",
        "/new/path"
    ));
    assert!(!workspace_pr_poll_target_is_current(
        &[],
        "workspace",
        "/new/path"
    ));
}

fn git(path: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn failed_side_branch_query_preserves_instead_of_clearing_badges() {
    let tmp = tempfile::TempDir::new().unwrap();
    git(tmp.path(), &["init", "-q", "-b", "main"]);
    git(
        tmp.path(),
        &[
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Fixture",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    );
    git(tmp.path(), &["checkout", "-qb", "side"]);
    git(tmp.path(), &["checkout", "-qb", "workspace"]);
    let mut context = context("workspace", &[]);
    context.path = tmp.path().to_owned();
    let mut calls = 0;
    let answers = get_workspace_prs_batch_with(vec![Ok(context)], |_, _, branches, open_only| {
        calls += 1;
        if open_only {
            Err("fallback timed out".into())
        } else {
            parse_discovery_response(&response(branches, &HashMap::new()), branches)
        }
    });
    assert_eq!(calls, 2);
    assert!(matches!(
        workspace_prs_outcome(answers.into_iter().next().unwrap()),
        WorkspacePrsOutcome::Preserve
    ));
}

#[test]
fn sibling_side_branch_fallback_is_batched_only_after_strong_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut contexts = Vec::new();
    for name in ["one", "two"] {
        let path = tmp.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        git(
            &path,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=Fixture",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "fixture",
            ],
        );
        git(&path, &["checkout", "-qb", "side"]);
        git(&path, &["checkout", "-qb", "workspace"]);
        let mut context = context("workspace", &[]);
        context.path = path;
        contexts.push(Ok(context));
    }
    let mut calls = Vec::new();
    let answers = get_workspace_prs_batch_with(contexts, |_, _, branches, open_only| {
        calls.push((branches.to_vec(), open_only));
        let rows = if open_only {
            HashMap::from([("side".into(), vec![row(42, "side", "OPEN", "fixture")])])
        } else {
            HashMap::new()
        };
        parse_discovery_response(&response(branches, &rows), branches)
    });
    assert_eq!(
        calls,
        vec![
            (vec!["workspace".into()], false),
            (vec!["side".into()], true)
        ]
    );
    for answer in answers {
        let prs = answer.unwrap();
        assert_eq!(prs[0].pr.number, 42);
        assert_eq!(prs[0].source, PrSource::SideBranch);
    }
}
