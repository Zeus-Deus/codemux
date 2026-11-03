// SDK options for helper queries (model list, usage, side chats) that
// must reach Claude the same way the user's chats do, without picking up
// the rest of the user's setup.
//
// Proxies and gateways are usually configured in `~/.claude/settings.json`
// (`env.ANTHROPIC_BASE_URL`, `env.ANTHROPIC_AUTH_TOKEN`, `apiKeyHelper`).
// With `settingSources: []` the CLI never reads that file, so a helper
// would fall back to a direct, unauthenticated connection. Loading only
// the user source keeps that routing; hooks and MCP servers from the same
// file are switched off because a helper must not run them. Project and
// local settings stay unread.

import type { Options } from "@anthropic-ai/claude-agent-sdk";

export type UserRoutingOptions = Pick<
  Options,
  "settingSources" | "settings" | "strictMcpConfig" | "mcpServers"
>;

export function userRoutingOptions(): UserRoutingOptions {
  return {
    settingSources: ["user"],
    settings: { disableAllHooks: true },
    strictMcpConfig: true,
    mcpServers: {},
  };
}
