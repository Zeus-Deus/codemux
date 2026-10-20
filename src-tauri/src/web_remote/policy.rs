//! Web clients get an explicit development surface, not the desktop IPC registry.
//! New commands are denied until reviewed here. This is not a sandbox: paired
//! clients can run agents and terminals, but cannot accidentally invoke desktop
//! administration, dev fixtures, destructive Git/worktree operations or account changes.

pub(super) fn allowed(cmd: &str) -> bool {
    matches!(
        cmd,
        "delegation_host_info" | "delegation_grants" | "delegation_authorize" | "delegation_revoke" | "delegate_task" | "delegation_list" | "delegation_read" | "delegation_cancel" | "delegation_respond" | "delegation_deliver"
            | "reply_to_pr_thread"
            | "set_pr_thread_resolved"
            | "submit_pr_review"
            | "add_pr_inline_comment"
            | "submit_pr_review_with_comments"
            // A paired client can already run the chat; watching only
            // schedules turns that chat would get anyway.
            | "pr_watch_start"
            | "pr_watch_stop"
            | "pr_watch_status"
            | "create_preset"
            | "update_preset"
            | "delete_preset"
            | "set_preset_pinned"
            | "refresh_workspace_issue"
            | "agent_chat_provider_health"
            // Read-only harness flag; always false in release builds.
            | "web_remote_e2e_local_mode"
            // Settings → Updates from a paired browser or phone. Each command
            // only picks a provider or a configured device; the executable
            // and installer are resolved on the machine that owns the CLI.
            // The chat notice's commands stay desktop-only: they accept a
            // Hermes executable path.
            | "provider_updates_local"
            | "provider_update_local"
            | "provider_updates_device"
            | "provider_update_device"
            | "device_helper_update"
            | "hermes_profiles"
            | "hermes_catalog"
            | "hermes_binding"
            | "hermes_disconnect"
            | "acp_agents"
            | "acp_save_agent"
            | "acp_delete_agent"
            | "acp_probe"
            | "acp_binding"
            | "acp_catalog"
            | "acp_thread_catalog"
            | "acp_set_config"
            | "unarchive_workspace"
            | "import_worktree_workspace"
            | "link_workspace_issue"
            | "unlink_workspace_issue"
            | "set_workspace_host"
            | "automations_create"
            | "automations_update"
            | "automations_set_enabled"
            | "automations_delete"
            | "automations_check_repo_access"
            | "create_pull_request"
            | "merge_pull_request"
            | "update_synced_settings"
            | "close_workspace"
            | "close_workspace_with_worktree"
            | "archive_workspace"
            | "activate_pane"
            | "file_exists"
            | "file_signature"
            | "grep_count_pattern"
            | "git_log_entries"
            | "git_commits_ahead"
            | "git_stage_files"
            | "git_unstage_files"
            | "git_commit_changes"
            | "git_push_changes"
            | "git_pull_changes"
            | "git_fetch_changes"
            | "git_clone_repo"
            | "init_git_repo"
            | "init_git_repo_no_commit"
            | "create_empty_repo"
            | "write_file"
            | "db_add_recent_project"
            | "bootstrap_session"
            | "refresh_session"
            | "check_auth"
            | "get_sync_status"
            | "repair_inactive_mcp_configs"
            | "apply_preset"
            | "agent_chat_open_search_result"
            | "activate_tab"
            | "activate_terminal_session"
            | "add_structured_log"
            | "agent_browser_close"
            | "agent_browser_run"
            | "agent_browser_screenshot"
            | "agent_browser_spawn"
            | "agent_chat_answer_question"
            | "agent_chat_cancel_queued_turn"
            | "agent_chat_close_pane"
            | "agent_chat_create_pane"
            | "agent_chat_discard_staged_image"
            | "agent_chat_get_checkpoint"
            | "agent_chat_get_session"
            | "agent_chat_get_session_context"
            | "agent_chat_get_tool_result"
            | "agent_chat_get_tool_result_image"
            | "agent_chat_interrupt_turn"
            | "agent_chat_list_messages"
            | "agent_chat_list_messages_after"
            | "agent_chat_list_messages_before"
            | "agent_chat_list_messages_tail"
            | "agent_chat_list_session_mentions"
            | "agent_chat_list_sessions"
            | "agent_chat_list_turn_checkpoints"
            | "agent_chat_get_turn_diff"
            // The editable, saved follow-up queue; the same reach as send/cancel.
            | "agent_chat_delivery_attention"
            | "agent_chat_acknowledge_delivery"
            | "agent_chat_retry_delivery"
            | "agent_chat_list_queued_turns"
            | "agent_chat_edit_queued_turn"
            | "agent_chat_move_queued_turn"
            | "agent_chat_cancel_saved_queued_turn"
            | "agent_chat_send_saved_queued_turn"
            | "agent_chat_prime_mcp"
            | "agent_chat_question_attention"
            | "agent_chat_read_image"
            | "agent_chat_read_local_image"
            | "agent_chat_rename_session"
            | "agent_chat_respond_to_request"
            | "agent_chat_search"
            | "agent_chat_send_queued_turn_now"
            | "agent_chat_send_turn"
            | "agent_chat_resume_after_usage_limit"
            | "agent_chat_cancel_usage_resume"
            | "agent_chat_set_fast_mode"
            | "agent_chat_set_service_tier"
            | "agent_chat_set_model"
            | "agent_chat_set_permission_mode"
            | "agent_chat_stage_image"
            | "agent_chat_start_session"
            | "agent_chat_stop_monitoring"
            | "agent_chat_stop_session"
            | "agent_chat_switch_provider"
            | "agent_chat_thread_head_id"
            | "agent_chat_turn_active"
            | "agent_chat_update_session_config"
            | "attach_agent_chat_output"
            | "attach_pty_output"
            | "automations_get"
            | "automations_list"
            | "automations_runs"
            | "browser_history_back"
            | "browser_history_forward"
            | "browser_open_url"
            | "browser_reload"
            | "browser_set_loading_state"
            | "cache_terminal_scrollback"
            | "check_claude_available"
            | "check_gh_available"
            | "check_gh_status"
            | "check_github_repo"
            | "check_is_git_repo"
            | "check_merge_conflicts"
            | "check_provider_auth"
            | "clear_agent_status"
            | "close_pane"
            | "close_tab"
            | "close_terminal_session"
            | "create_browser_pane"
            | "create_empty_workspace"
            | "create_tab"
            | "create_workspace"
            | "create_workspace_with_preset"
            | "create_worktree_workspace"
            | "db_get_all_settings"
            | "db_get_recent_projects"
            | "db_get_setting"
            | "db_get_ui_state"
            | "debug_log"
            | "detach_agent_chat_output"
            | "detach_pty_output"
            | "detect_editors"
            | "detect_package_manager"
            | "detect_run_candidates"
            | "discover_source_control"
            | "dock_browser_in_right_panel"
            | "environment_info"
            | "flush_scrollback_cache"
            | "generate_branch_name"
            | "generate_random_branch_name"
            | "generate_workspace_title"
            | "get_adapter_info"
            | "get_app_state"
            | "get_base_branch_diff"
            | "get_base_branch_file_diff"
            | "get_branch_pull_request"
            | "get_browser_data_size"
            | "get_check_log_excerpt"
            | "get_commit_files"
            | "get_current_theme"
            | "get_default_branch"
            | "get_detected_ports"
            | "get_feature_flags"
            | "get_git_branch_info"
            | "get_git_diff"
            | "get_git_diff_stat"
            | "get_git_status"
            | "get_github_issue"
            | "get_github_issue_by_path"
            | "get_github_pr_by_path"
            | "get_github_pr_diff_by_path"
            | "get_home_dir"
            | "get_mcp_runtime_status"
            | "get_merge_state"
            | "get_omarchy_theme"
            | "get_or_create_home_workspace"
            | "get_package_format"
            | "get_performance_diagnostics"
            | "get_pr_deployments"
            | "get_pr_inline_comments"
            | "get_pr_review_comments"
            | "get_pr_review_diff"
            | "get_pr_review_threads"
            | "get_pr_timeline"
            | "get_presets"
            | "get_preset_availability"
            | "get_project_index_status"
            | "get_project_memory_snapshot"
            | "get_project_scripts"
            | "get_pull_request_checks"
            | "get_renderer_mode"
            | "get_resolution_diff"
            | "get_resource_metrics"
            | "get_scanner_captures"
            | "get_shell_appearance"
            | "get_synced_settings"
            | "get_terminal_scrollback"
            | "get_terminal_status"
            | "get_workspace_config"
            | "github_rate_limit"
            | "has_codemuxinclude"
            | "hosts_list"
            | "hosts_status_list"
            | "get_local_device_name"
            | "create_workspace_on_host"
            | "list_branches"
            | "list_branches_detailed"
            | "list_chat_provider_capabilities"
            | "list_chat_slash_commands"
            | "list_directory"
            | "list_github_issues"
            | "list_github_issues_by_path"
            | "list_incoming_prs"
            | "list_launch_gemini_models"
            | "list_mcp_servers"
            | "list_mcp_tools"
            | "list_mcp_tools_for_server"
            | "list_project_files"
            | "list_project_folders"
            | "list_prs_overview"
            | "refresh_github_read_cache"
            | "list_prs_overview_stats"
            | "list_pull_requests"
            | "list_skills"
            | "list_tool_permissions"
            | "list_worktrees"
            | "materialize_chat_workspace"
            | "opencode_check_availability"
            | "opencode_list_models"
            | "opencode_ping"
            | "pause_pty_output"
            | "prime_mcp_runtime"
            | "read_file"
            | "read_file_for_attachment"
            | "read_folder_for_attachment"
            | "refresh_workspace_git_info"
            | "rename_tab"
            | "rename_workspace"
            | "reorder_tabs"
            | "reorder_workspaces"
            | "resize_pty"
            | "resize_split"
            | "restart_terminal_session"
            | "resume_pty_output"
            | "save_terminal_scrollback"
            | "search_file_names"
            | "search_in_files"
            | "search_project_index"
            | "set_workspace_muted"
            | "set_workspace_pinned"
            | "split_pane"
            | "start_browser_stream"
            | "start_skills_watcher"
            | "stop_skills_watcher"
            | "swap_panes"
            | "terminal_foreground_jobs"
            | "terminal_session_cwds"
            | "touch_workspace"
            | "uncache_terminal_scrollback"
            | "undock_browser_from_right_panel"
            | "update_workspace_cwd"
            | "usage_summary"
            // Usage → Limits and model prices. Reading limits runs the same
            // Claude/Codex CLIs paired clients already drive through agents;
            // a model price only changes how this host displays cost.
            | "usage_refresh_quota"
            | "usage_price_overrides"
            | "usage_set_price_override"
            // Provider accounts: paired clients see the accounts and move a
            // chat between them. Adding, removing, signing in, and defaults
            // stay on the desktop with the account folders.
            | "provider_accounts_list"
            | "provider_accounts_refresh"
            | "provider_account_thread"
            | "provider_account_switch_thread"
            | "validate_resume"
            | "workspaces_adoption_preview"
            | "workspaces_sync_list"
            | "workspaces_worktree_sizes"
            | "write_to_pty"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chatgpt_private_metadata_and_mutations_are_denied_to_web_remote() {
        for cmd in [
            "get_chatgpt_status", "start_chatgpt_login", "cancel_chatgpt_login",
            "disconnect_chatgpt", "acknowledge_chatgpt_welcome",
            "get_local_workbench", "set_local_workbench",
        ] {
            assert!(!allowed(cmd), "{cmd} exposes local-only ChatGPT account/preferences authority");
        }
    }
    #[test]
    fn paired_clients_can_read_the_e2e_local_mode_flag() {
        assert!(allowed("web_remote_e2e_local_mode"));
    }
    #[test]
    fn web_remote_reads_environment_identity_but_never_dials_out() {
        // A paired client learns which machine it drives; it must not use the
        // desktop's account token or iroh client to reach other machines.
        assert!(allowed("environment_info"));
        for cmd in [
            "environment_account_request", "environment_dial",
            "environment_stream_write", "environment_stream_close",
        ] {
            assert!(!allowed(cmd), "{cmd} lends desktop account or network authority");
        }
    }
    #[test]
    fn custom_acp_thread_catalog_matches_existing_remote_read_policy() {
        for cmd in ["acp_binding", "acp_catalog", "acp_thread_catalog"] {
            assert!(allowed(cmd), "{cmd}");
        }
        assert!(!allowed("acp_get_credentials"));
        assert!(!allowed("acp_future_command"));
    }
    #[test]
    fn hermes_remote_profile_workflow_is_explicitly_allowed() {
        // Paired clients already run and configure agent chats. Hermes exposes
        // profile identity/catalog/binding metadata and runtime disconnect only;
        // authentication remains in the official Hermes runtime. The executable
        // always comes from host settings, never from the caller's profile.
        for cmd in [
            "hermes_profiles",
            "hermes_catalog",
            "hermes_binding",
            "hermes_disconnect",
        ] {
            assert!(allowed(cmd), "{cmd}");
        }
        for cmd in ["hermes_auth", "hermes_get_credentials", "hermes_future_command"] {
            assert!(!allowed(cmd), "{cmd}");
        }
    }

    #[test]
    fn update_all_allows_provider_names_but_not_executable_paths() {
        for cmd in [
            "provider_updates_local",
            "provider_update_local",
            "provider_updates_device",
            "provider_update_device",
            "device_helper_update",
        ] {
            assert!(allowed(cmd), "{cmd}");
        }
        for cmd in ["agent_chat_provider_update", "agent_chat_provider_update_check"] {
            assert!(!allowed(cmd), "{cmd} accepts an executable path");
        }
    }

    #[test]
    fn usage_page_commands_are_explicitly_allowed() {
        for cmd in [
            "usage_summary",
            "usage_refresh_quota",
            "usage_price_overrides",
            "usage_set_price_override",
        ] {
            assert!(allowed(cmd), "{cmd}");
        }
    }

    #[test]
    fn provider_accounts_can_be_used_but_not_managed_remotely() {
        for cmd in ["provider_accounts_list", "provider_accounts_refresh", "provider_account_thread", "provider_account_switch_thread"] {
            assert!(allowed(cmd), "{cmd}");
        }
        for cmd in [
            "provider_account_add",
            "provider_account_rename",
            "provider_account_remove",
            "provider_account_set_default",
            "provider_account_open_sign_in",
        ] {
            assert!(!allowed(cmd), "{cmd} changes accounts on the desktop");
        }
    }

    #[test]
    fn workspace_naming_commands_are_explicitly_allowed() {
        // Paired clients create workspaces too, so they need the same branch
        // and title naming as the desktop; both run on the Utility model.
        for cmd in [
            "generate_branch_name",
            "generate_random_branch_name",
            "generate_workspace_title",
        ] {
            assert!(allowed(cmd), "{cmd}");
        }
    }

    #[test]
    fn remote_invoke_allowlist_is_closed_by_default() {
        for cmd in [
            "touch_workspace",
            "get_app_state",
            "write_to_pty",
            "split_pane",
            "agent_chat_send_turn",
            "agent_chat_resume_after_usage_limit",
            "agent_chat_cancel_usage_resume",
            "materialize_chat_workspace",
            "create_workspace_on_host",
            "get_local_device_name",
            "submit_pr_review_with_comments",
            "automations_create",
            "unarchive_workspace",
            "detect_run_candidates",
        ] {
            assert!(allowed(cmd), "{cmd}");
        }
        for cmd in [
            "activate_workspace",
            "cycle_workspace",
            "create_terminal_session",
            "dev_agent_chat_spawn_test_pane",
            "quit_app",
            "delete_archived_workspace",
            "remove_worktree",
            "git_discard_file",
            "get_auth_token",
            "web_remote_disable",
            "hosts_add",
            "hosts_ssh_config_hosts",
            "plugin:shell|execute",
            "future_command",
            // Webhook URL tokens, signing setup and the secret store stay on
            // the desktop: a paired browser never reads or enters them.
            "automations_webhook_credentials",
            "automations_rotate_webhook_token",
            "automations_set_webhook_signature",
            "secrets_store",
            "secrets_list",
            "secrets_delete",
        ] {
            assert!(!allowed(cmd), "{cmd}");
        }
    }
}
