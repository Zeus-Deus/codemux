//! Per-account usage for Settings → Usage.
//!
//! Every ledger row carries the account whose history produced it (see
//! `usage_import`). This splits the summary's rows by account, for the
//! providers where the user has more than one.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::database::provider_accounts::ProviderAccountRow;
use crate::database::UsageLedgerRow;

use super::DEFAULT_ACCOUNT_ID;

/// One account's share of the period.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountUsage {
    pub provider: String,
    pub account_id: String,
    /// The account's name; `None` for the CLI's own sign-in, which the page
    /// names itself.
    pub label: Option<String>,
    pub tokens: i64,
    pub cost_usd: f64,
    pub session_count: i64,
}

/// Split `rows` in `[start_ms, end_ms)` by account. Only providers with an
/// extra account (or history tagged with one) appear; every account of
/// such a provider is listed, idle ones at zero, CLI sign-in first.
pub fn account_usage(
    rows: &[UsageLedgerRow],
    start_ms: i64,
    end_ms: i64,
    accounts: &[ProviderAccountRow],
) -> Vec<AccountUsage> {
    let mut providers: BTreeSet<String> = accounts.iter().map(|a| a.provider.clone()).collect();
    providers.extend(
        rows.iter()
            .filter(|r| r.account_id.is_some())
            .map(|r| r.provider.clone()),
    );
    let labels: HashMap<&str, &str> = accounts.iter().map(|a| (a.id.as_str(), a.label.as_str())).collect();

    let mut order: Vec<(String, String)> = Vec::new();
    let mut totals: Totals = HashMap::new();
    for provider in &providers {
        ensure(&mut totals, &mut order, &labels, provider, DEFAULT_ACCOUNT_ID);
        for account in accounts.iter().filter(|a| &a.provider == provider) {
            ensure(&mut totals, &mut order, &labels, provider, &account.id);
        }
    }
    for row in rows {
        if row.created_at < start_ms || row.created_at >= end_ms || !providers.contains(&row.provider) {
            continue;
        }
        let id = row.account_id.as_deref().unwrap_or(DEFAULT_ACCOUNT_ID);
        let key = ensure(&mut totals, &mut order, &labels, &row.provider, id);
        let (usage, threads) = totals.get_mut(&key).expect("ensured above");
        usage.tokens += row.total_tokens();
        usage.cost_usd += row.cost_usd.unwrap_or(0.0);
        threads.insert(row.thread_id.clone());
    }
    order
        .into_iter()
        .filter_map(|key| totals.remove(&key))
        .map(|(mut usage, threads)| {
            usage.session_count = threads.len() as i64;
            usage
        })
        .collect()
}

type Totals = HashMap<(String, String), (AccountUsage, HashSet<String>)>;

/// The entry for `(provider, id)`, created at zero in first-seen order.
fn ensure(
    totals: &mut Totals,
    order: &mut Vec<(String, String)>,
    labels: &HashMap<&str, &str>,
    provider: &str,
    id: &str,
) -> (String, String) {
    let key = (provider.to_string(), id.to_string());
    if !totals.contains_key(&key) {
        order.push(key.clone());
        let label = (id != DEFAULT_ACCOUNT_ID)
            .then(|| labels.get(id).map(|l| l.to_string()).unwrap_or_else(|| "Removed account".into()));
        let usage = AccountUsage {
            provider: provider.to_string(),
            account_id: id.to_string(),
            label,
            tokens: 0,
            cost_usd: 0.0,
            session_count: 0,
        };
        totals.insert(key.clone(), (usage, HashSet::new()));
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(at: i64, provider: &str, account: Option<&str>, thread: &str, tokens: i64, cost: f64) -> UsageLedgerRow {
        UsageLedgerRow {
            created_at: at,
            thread_id: thread.into(),
            workspace_id: None,
            provider: provider.into(),
            model: Some("m".into()),
            subagent: false,
            input_tokens: tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            cost_usd: Some(cost),
            cost_source: Some("table".into()),
            source: "provider_history".into(),
            account_id: account.map(str::to_string),
        }
    }

    fn account(id: &str, provider: &str) -> ProviderAccountRow {
        ProviderAccountRow {
            id: id.into(),
            provider: provider.into(),
            label: format!("{id} label"),
            home: String::new(),
            created_at_ms: 0,
        }
    }

    #[test]
    fn splits_a_provider_with_accounts_and_leaves_others_alone() {
        let rows = vec![
            row(10, "claude", None, "a", 100, 1.0),
            row(11, "claude", Some("work"), "b", 50, 0.5),
            row(12, "claude", Some("work"), "c", 25, 0.25),
            row(13, "codex", None, "d", 999, 9.0),
            // Outside the window.
            row(99, "claude", Some("work"), "e", 1, 1.0),
        ];
        let usage = account_usage(&rows, 0, 50, &[account("work", "claude"), account("idle", "claude")]);
        let ids: Vec<_> = usage.iter().map(|u| (u.provider.as_str(), u.account_id.as_str())).collect();
        assert_eq!(ids, vec![("claude", "default"), ("claude", "work"), ("claude", "idle")]);
        assert_eq!(usage[0].tokens, 100);
        assert_eq!(usage[0].label, None);
        assert_eq!(usage[1].tokens, 75);
        assert_eq!(usage[1].session_count, 2);
        assert_eq!(usage[1].label.as_deref(), Some("work label"));
        assert_eq!(usage[2].tokens, 0);
    }

    #[test]
    fn history_from_a_removed_account_still_shows() {
        let rows = vec![row(1, "codex", Some("gone"), "a", 10, 0.1)];
        let usage = account_usage(&rows, 0, 50, &[]);
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[1].label.as_deref(), Some("Removed account"));
    }

    #[test]
    fn no_accounts_means_no_breakdown() {
        assert!(account_usage(&[row(1, "claude", None, "a", 1, 0.0)], 0, 50, &[]).is_empty());
    }
}
