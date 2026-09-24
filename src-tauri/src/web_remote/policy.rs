//! Web clients get an explicit development surface, not the desktop IPC registry.
//! New commands are denied until reviewed here. This is not a sandbox: paired
//! clients can run agents and terminals, but cannot accidentally invoke desktop
//! administration, dev fixtures, destructive Git/worktree operations or account changes.

pub(super) fn allowed(cmd: &str) -> bool {
    matches!(
        cmd,
        "reply_to_pr_thread"
            | "set_pr_thread_resolved"
            | "submit_pr_review"
            | "add_pr_inline_comment"
            | "submit_pr_review_with_comments"
            | "create_preset"
            | "update_preset"
            | "delete_preset"
            | "set_preset_pinned"
            | "refresh_workspace_issue"
            | "agent_chat_provider_health"
            | "hermes_profiles"
            | "hermes_catalog"
            | "hermes_binding"
            | "hermes_disconnect"
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
            | "agent_chat_interrupt_turn"
            | "agent_chat_list_messages"
            | "agent_chat_list_messages_after"
            | "agent_chat_list_messages_before"
            | "agent_chat_list_messages_tail"
            | "agent_chat_list_session_mentions"
            | "agent_chat_list_sessions"
            | "agent_chat_list_turn_checkpoints"
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
            | "agent_chat_set_model"
            | "agent_chat_set_permission_mode"
            | "agent_chat_stage_image"
            | "agent_chat_start_session"
            | "agent_chat_stop_monitoring"
            | "agent_chat_stop_session"
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
            | "discover_source_control"
            | "dock_browser_in_right_panel"
            | "flush_scrollback_cache"
            | "generate_branch_name"
            | "generate_random_branch_name"
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
            | "terminal_session_cwds"
            | "touch_workspace"
            | "uncache_terminal_scrollback"
            | "undock_browser_from_right_panel"
            | "update_workspace_cwd"
            | "usage_summary"
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
    fn hermes_remote_profile_workflow_is_explicitly_allowed() {
        // Paired clients already run and configure agent chats. Hermes exposes
        // profile identity/catalog/binding metadata and runtime disconnect only;
        // authentication remains in the official Hermes runtime.
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
            "submit_pr_review_with_comments",
            "automations_create",
            "unarchive_workspace",
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
            "plugin:shell|execute",
            "future_command",
        ] {
            assert!(!allowed(cmd), "{cmd}");
        }
    }
}
