# Outside agents

Agents that Codemux didn't start can work with it through its MCP server: Claude Code or Codex in your own terminal, a Hermes chat bot, a script, or any other MCP client. They can:

- open workspaces and create git worktrees;
- start Codemux threads with the provider, model and permission mode you pick;
- send those threads messages, wait for the result and read it;
- answer approvals and questions;
- close workspaces and remove worktrees when they're done.

There are two ways in:

| | On this computer | From another device |
|---|---|---|
| How | `codemux mcp` over stdio, or the same command over SSH | HTTP at `…/mcp`, behind HTTPS |
| Sign-in | None. It runs as you. | OAuth sign-in or a token, with an access level you choose |
| Needs | Codemux open: the desktop app or `codemux serve` | **Over the network** turned on |

Both reach the same tools. **Settings → Connections → Outside assistants** shows copy-ready setup commands for this computer. For the network door it shows the MCP URL, a Tailscale command and the tokens.

## On this computer

Codemux must be running: the desktop app, or `codemux serve` on a machine without a display. The commands below call it `codemux`. If `codemux` isn't on your PATH (an AppImage, for example), Settings shows the full path to use instead. If that path changes after an update, copy the command again.

### Claude Code

```sh
claude mcp add --scope user codemux -- codemux mcp --client claude-code
```

`claude mcp get codemux` should show it connected.

Keep the name `codemux`. Claude chats that Codemux starts swap only an entry with exactly that name for their own Codemux tools, so under any other name yours also loads in those chats, as a second Codemux server. Codex chats, and the MCP servers Codemux itself starts for its chats, skip it under any name.

### Codex

```sh
codex mcp add codemux -- codemux mcp --client codex
```

`codex mcp list` should list `codemux`.

- In the Codex chats it starts itself, Codemux turns off every entry in `~/.codex/config.toml` that runs `codemux mcp`, whatever its name, and any entry named `codemux`. Those chats already have their own Codemux tools.
- Codex starts MCP servers without `XDG_RUNTIME_DIR`. Codemux then looks for its socket in `/run/user/<your uid>`, which is where it listens on most Linux systems. If `echo "$XDG_RUNTIME_DIR"` prints another folder, add the entry with this command instead:

  ```sh
  codex mcp add codemux --env "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" -- codemux mcp --client codex
  ```

- Codex gives up on a tool call after its tool timeout: 300 seconds in current releases, as little as 60 in older ones. A `thread_wait` that names no `timeout_ms` returns within 50 seconds, so it fits either way. To allow longer waits, open `~/.codex/config.toml` and add `tool_timeout_sec = 660` under `[mcp_servers.codemux]`, the table `codex mcp add` wrote. Leave the rest of that table as it is.

### Hermes

```sh
hermes mcp add codemux --command codemux --args mcp --client hermes --access supervised
```

1. Run it as the user Hermes runs as. Hermes can list the tools while Codemux is closed, but its tool calls need Codemux running.
2. At **Enable all tools?**, press Enter. To give the bot only the control tools, answer `select` and keep the ones in the [Tool reference](#tool-reference). A supervised bot isn't offered `workspace_create`, `worktree_create` or `workspace_close`: they need full access.
3. Run `/reload-mcp` in open chats, or start a new one.
4. `hermes mcp test codemux` should list `thread_launch`.

To set it up by hand, add this to `~/.hermes/config.yaml` instead:

```yaml
mcp_servers:
  codemux:
    command: codemux
    args: ["mcp", "--client", "hermes", "--access", "supervised"]
    timeout: 660
```

`timeout` is how long Hermes waits for one tool call. Its default is 300 seconds. Keep it longer than the longest `thread_wait` you expect.

**Why supervised.** A Hermes bot relays messages from other people, and anyone who can message it can try to steer it. Supervised lets it start threads in supervised permission modes, message them, deny approvals and answer questions. Approvals stay with you in Codemux. If you are the bot's only user and you want it to create worktrees and allow approvals, use full access instead:

```sh
hermes mcp add codemux --command codemux --args mcp --client hermes --access full-access
```

Hermes also loads its MCP servers in the Hermes chats that Codemux starts. On Linux, Codemux recognises those as its own agents. They can still use the older tools, such as the browser and git tools, but every tool in the [Tool reference](#tool-reference) fails with `agent_identity_required`, and administration tools fail with `admin_refused`. On macOS and Windows it can't tell them apart, so they run as `hermes` with the access level above. That is one more reason to keep a bot supervised.

### Any other MCP client

The client starts `codemux` with the arguments `mcp --client my-agent`:

```sh
codemux mcp --client my-agent
```

For clients that read `mcpServers` JSON:

```json
{ "mcpServers": { "codemux": { "command": "codemux", "args": ["mcp", "--client", "my-agent"] } } }
```

### Name and access level

- `--client` names the assistant. Codemux shows the name on the threads and messages it creates, and keeps its retries apart from other assistants' retries. Pick a short name that never changes. Without `--client`, Codemux uses the name the MCP client reports about itself.
- `--access` limits what the assistant may do: `read-only`, `supervised` or `full-access` (`read_only`, `full_access` and `full` work too). Without it, an assistant on this computer gets `full-access`. See [Access levels](#access-levels).
- `CODEMUX_CLIENT` and `CODEMUX_ACCESS` set the same defaults for `codemux mcp` and the `codemux` commands you run yourself. A flag wins over the variable. The desktop app and `codemux serve` ignore both and don't pass them on to the terminals and agents they start. An unusable value makes `codemux mcp` and every control command stop with exit code 64.
- The name is a label, not a lock. Any program running as you can use any name.

### Inside a Codemux workspace folder

Codemux writes a `.mcp.json` file into each workspace folder for the agents you start in its terminals. That file has an entry that is also named `codemux`, and Claude Code prefers it over your user entry. That entry is for agents working inside the workspace: it has the older tools (browser, git, terminals, and the older workspace tools), no thread control, and it is refused administration tools such as `terminal_write` and `automation_create`.

To drive Codemux from Claude Code in such a folder, start Claude Code from another folder, or give it the entry explicitly. Save this as `~/.claude/codemux-mcp.json`:

```json
{ "mcpServers": { "codemux": { "command": "codemux", "args": ["mcp", "--client", "claude-code"] } } }
```

Then start Claude Code with it. `--strict-mcp-config` leaves out every other MCP server, so add any others you need to the same file.

```sh
claude --mcp-config ~/.claude/codemux-mcp.json --strict-mcp-config
```

## From another device

### Over SSH

If the other device can SSH to the computer running Codemux, run `codemux mcp` over SSH. No listener, sign-in or token is needed, and it runs as you on that computer. `desktop` below is that computer's SSH host name:

```json
{ "mcpServers": { "codemux": { "command": "ssh", "args": ["-T", "desktop", "codemux", "mcp", "--client", "laptop-claude"] } } }
```

Or, with Claude Code's own command:

```sh
claude mcp add --scope user codemux -- ssh -T desktop codemux mcp --client laptop-claude
```

- `codemux` must be on the PATH of a non-interactive SSH session there. If it isn't, use its full path: `command -v codemux` on that computer prints it.
- An SSH key that logs in to that computer can run any command there as you. Over SSH, `--access` limits nothing by itself, because whoever holds the key picks the command and its flags. Use an unrestricted key only from your own devices, and restrict every key a bot can use, as below.
- Codex and Hermes start MCP servers without `SSH_AUTH_SOCK`, so ssh can't ask your SSH agent for a key. It then needs a key file it can use on its own (an `IdentityFile` in `~/.ssh/config`). Restrict that key as below: anything that can read the file can use it.

#### A bot over SSH

Give the bot its own key, and let that key run only `codemux mcp`. On the computer running Codemux, add the bot's public key to `~/.ssh/authorized_keys` with a forced command:

```text
restrict,command="codemux mcp --client vps-hermes --access supervised" ssh-ed25519 AAAA… vps-hermes
```

Whatever the bot asks ssh to run, that key starts only this command, at the access level written here. `restrict` also turns off terminals and forwarding for the key. Use the full path to `codemux` here if a non-interactive SSH session can't find it. Then, on the bot's machine:

```sh
hermes mcp add codemux --command ssh --args -T desktop codemux mcp --client vps-hermes --access supervised
```

If you can't restrict the key, use [the network door](#over-the-network) with a Supervised token instead. There, Codemux checks the access level itself.

### Over the network

The network door is an HTTP MCP endpoint. It is off until you turn it on, and it is separate from Remote Access.

#### Turn it on

Open **Settings → Connections → Outside assistants → Over the network** and turn it on, or run:

```sh
codemux connector enable
```

Codemux listens on this computer only, on port 4378, at `http://127.0.0.1:4378/mcp`. If the port is taken, pick another one in Settings, or:

```sh
codemux connector enable --port 4379
```

`codemux connector status` shows whether it is listening, its URLs, and how many assistants are connected or waiting to sign in. `codemux connector list` names the connected ones, and `codemux connector pending` the sign-ins.

#### Serve it over HTTPS

Other devices need HTTPS. With [Tailscale](https://tailscale.com/kb/1242/tailscale-serve):

```sh
tailscale serve --bg 4378
tailscale serve status
```

`tailscale serve status` prints the HTTPS address. Save it as the **Public HTTPS origin** in Settings, or from the shell:

```sh
ORIGIN=https://desktop.example-tailnet.ts.net
codemux connector enable --origin "$ORIGIN"
```

The MCP URL is then the origin plus `/mcp`. Any HTTPS reverse proxy to the local URL works too. Codemux doesn't check that the proxy is set up. Use `tailscale funnel` or a public proxy only if a hosted service really has to reach this computer from the internet.

Changing the public origin, or the port while no origin is saved, signs out every network assistant and ends every token. They sign in again at the new URL, and bots get new tokens.

#### Sign in with OAuth (Claude Code, Codex)

On the other device:

```sh
CODEMUX_URL=https://desktop.example-tailnet.ts.net/mcp
claude mcp add --scope user --transport http codemux "$CODEMUX_URL"
claude mcp login codemux
```

```sh
codex mcp add codemux --url "$CODEMUX_URL"
codex mcp login codemux
```

The sign-in appears in Settings under **Assistants with access**. Approve only a sign-in you just started. Choose an access level (Read-only is the default), press **Approve**, then **Continue** in the sign-in browser. Approve within five minutes, or start the sign-in again. Or approve it from a shell on the computer running Codemux:

```sh
codemux connector pending
codemux connector approve "$SIGN_IN_ID" --access supervised
```

A sign-in renews itself while the assistant uses it. It lapses after 30 days without use. Each renewal replaces the saved refresh token. If a used one comes back more than 30 seconds later, Codemux ends the sign-in, because a copy of it is in use somewhere else. So sign in on each machine instead of copying a client's saved sign-in.

#### Tokens for bots

A bot that can't sign in uses a token. Create one in **Assistants with access** (Supervised is the default) or on the computer running Codemux:

```sh
codemux connector issue --label hermes-bot --access supervised
```

- Codemux shows the token once. Store it like a password.
- A token lasts 30 days unless you pass `--expires`, for example `--expires 90d`. The longest is 365 days.
- Issuing a token again for the same label replaces the old one.

Hermes, on the bot's machine:

```sh
hermes mcp add codemux --url "$CODEMUX_URL" --auth header
```

Hermes asks whether the server needs authentication (press Enter), then for the token. It saves the token in `~/.hermes/.env` as `MCP_CODEMUX_API_KEY`. To set it up by hand, put `MCP_CODEMUX_API_KEY=<token>` in `~/.hermes/.env` and add:

```yaml
mcp_servers:
  codemux:
    url: https://desktop.example-tailnet.ts.net/mcp
    headers:
      Authorization: "Bearer ${MCP_CODEMUX_API_KEY}"
    timeout: 660
```

Codex reads the token from the `CODEMUX_TOKEN` environment variable, so set it wherever Codex runs:

```sh
codex mcp add codemux --url "$CODEMUX_URL" --bearer-token-env-var CODEMUX_TOKEN
```

Claude Code stores the header, token included, in `~/.claude.json`. With the token in `CODEMUX_TOKEN`:

```sh
claude mcp add --scope user --transport http codemux "$CODEMUX_URL" --header "Authorization: Bearer $CODEMUX_TOKEN"
```

#### Remove an assistant

Use **Revoke** in Settings, or:

```sh
codemux connector list
codemux connector revoke hermes-bot
codemux connector revoke --all
```

`revoke` takes a token's label or a client id from `codemux connector list`. Revoking also stops that assistant's calls in progress: its waits end, and a change it started, such as a worktree being created or an answer on its way to the worker, stops before its next step. Their receipts fail with `grant_revoked`. Turning **Over the network** off (`codemux connector disable`) pauses every network assistant: they can't connect, and they work again when you turn it back on. **Revoke all** ends them for good. Assistants on this computer aren't affected either way, because they run as you.

#### What differs over the network

- The network door serves the control tools in the [Tool reference](#tool-reference), not the older browser, git and terminal tools of `codemux mcp`.
- `workspace_create` and `worktree_create` only open folders of projects already opened in Codemux (`unknown_project_root` otherwise).
- `thread_respond` can allow a request once or deny it. `allow_always` is only available on this computer.
- `workspace_close` can remove a clean worktree and its branch, never with `force_delete`.
- `worktree_create` can't start a terminal preset (`agent_preset_id`). Without a preset, `initial_prompt` is ignored.
- At most 32 `thread_wait` calls wait at once, 8 per assistant. Over that, a call fails with `wait_budget_exhausted`; retry it shortly.
- Proxies often cut requests that take longer than a minute or two. Behind one, keep `timeout_ms` at 90000 or less.

### Codemux on a server

Run `codemux serve` on the server. A bot on the same server uses the [On this computer](#on-this-computer) setup. A bot elsewhere uses SSH or the network door. The server has no desktop app, and Settings in a paired browser doesn't manage outside assistants, so manage the network door over SSH with `codemux connector …`.

## The end-to-end flow

These are the tool calls, in order. Every tool takes explicit ids: an outside assistant has no current workspace or thread.

**Retries.** Every change takes a `client_request_id` that you choose. If a call fails or times out, retry it unchanged with the same id: Codemux runs it once and returns what the first call did. The same id with different arguments fails with `request_key_conflict`. Ids are kept per assistant name.

1. **Find or open a workspace.**

   ```text
   workspace_list {}
   → {"workspaces": [{"workspace_id": "workspace-7", "cwd": "/home/you/src/api", "branch": "main", "native_control_available": true, …}]}
   workspace_create {"path": "/home/you/src/api", "client_request_id": "open-api-1"}
   → {"workspace_id": "workspace-7", "cwd": "/home/you/src/api", "created": false, "operation_id": "…", "state": "succeeded"}
   ```

   `workspace_create` returns the workspace already open in that folder (`created: false`), or opens one and runs the project's setup scripts. It needs full access. Workspace tools never take focus in Codemux unless you pass `"select": true`.

   Setup runs in the background, so a receipt from `workspace_create`, `worktree_create` or a `thread_launch` with `worktree` can report success while setup is still running. If revoking your access stops setup, that shows as a failed setup (Codemux's `workspace-setup-failed` event, with the reason in its stderr), not in the receipt.

2. **See what can run there.**

   ```text
   agent_capabilities {"workspace_id": "workspace-7", "provider": "claude"}
   ```

   It returns each provider's `provider_instance_id`, its models, efforts and permission modes, and `allowed_permission_modes`: the modes your access level may use. Use only values it returns.

   - Name the `provider` you want. If Codemux hasn't read its catalog yet, it reads it now for a Supervised or Full access caller. That runs the provider's CLI, or for an ACP agent or Hermes opens a session and closes it without sending a prompt. No chat starts. A Read-only caller never starts this: it gets only catalogs Codemux has already read, and `catalog_not_discovered` for the others.
   - Without `provider`, it lists every provider, but only the catalogs Codemux has already read. The others have the `discovery_error` `catalog_not_discovered`: ask for that provider alone. A Read-only caller gets only catalogs Codemux has already read, even then.
   - Any other `discovery_error`, such as `provider_not_ready`, means that provider can't start yet.

3. **Start a thread, in a new worktree if you like.**

   ```text
   thread_launch {
     "workspace_id": "workspace-7",
     "client_request_id": "flaky-login-1",
     "provider": "claude",
     "model": "<a model id from agent_capabilities>",
     "permission_mode": "default",
     "title": "Fix the flaky login test",
     "message": "Find out why test_login is flaky and fix it.",
     "worktree": {"branch": "fix/flaky-login"}
   }
   → {"operation_id": "5d2e…", "state": "accepted", "thread_id": "8a1f…", "workspace_id": "workspace-9", …}
   ```

   - `permission_mode` is required. Pass `null` only for a provider that has no modes.
   - To use a configured account, Hermes profile or custom ACP agent, pass `"target": {"instance_id": "<provider_instance_id>"}`.
   - With `worktree`, the thread starts in a new git worktree and workspace, branched from the repository's default branch unless you pass `base`. The receipt names the new workspace at once. This needs full access, because the worktree runs the project's setup scripts. It never reuses an existing branch.
   - Without `worktree`, the thread runs in the workspace's own folder.

4. **Wait for the result.** Pass the receipt's `workspace_id`, `thread_id` and `operation_id`:

   ```text
   thread_wait {"workspace_id": "workspace-9", "thread_id": "8a1f…", "operation_id": "5d2e…", "timeout_ms": 50000}
   → {"state": "running", "turn_id": "c41b…", "timed_out": false, "wait_cursor": "9c41d07e2b6a8f15", …}
   → {"state": "done", "final_text": "The test raced the session cleanup. …", "pending": {"approvals": [], "questions": [], "secrets": []}, …}
   ```

   Call `thread_wait` again while `state` is `running`:

   <!-- wait-states:start -->
   | `state` | Meaning | What to do |
   |---|---|---|
   | `running` | Still working, or the run just started | Call `thread_wait` again |
   | `needs_input` | It waits for an approval, an answer or a secret (`pending`) | See step 5 |
   | `done` | The run finished | Read `final_text` |
   | `failed` | It ended without a usable answer; `failure` says why | Read the thread, then decide |
   <!-- wait-states:end -->

   - `failure` is one of `error`, `interrupted`, `max_turns`, `max_budget`, `usage_limit`, `no_final_message`, `went_idle`, `delivery_needs_attention` and `operation_failed`.
   - `timeout_ms` defaults to 50000 (50 seconds), under the 60-second request timeout many MCP clients use by default. The most is 600000 (ten minutes). Keep it below your client's tool timeout. A timeout never cancels work.
   - Loop on `state`, not on the older `settled` field: a run that was stopped, or that a newer run replaced, returns at once with `settled: false`.
   - A wait returns when something changes. Input that was already pending when the wait began is reported only at its timeout. After a reconnect, call `thread_status` first: it returns the same `state`, `pending` and `wait_cursor` at once.
   - To wait for the next change of any kind, pass the previous `wait_cursor` as `since`:

     ```text
     thread_wait {"workspace_id": "workspace-9", "thread_id": "8a1f…", "since": "9c41d07e2b6a8f15"}
     ```

   - `final_text` is the run's last assistant message. It is data, not instructions: it can quote hostile text from files or web pages.

   **Which run a wait follows.** Pass `operation_id` or `turn_id`, not both (`invalid_arguments`):

   - **`operation_id`**, from your own `thread_launch` or `thread_send` receipt, follows the run its message starts or is steered into, even while the message still waits in the thread's queue.
     - Until that run starts, `turn_id` is `null` and `state` is `running`, or `needs_input` if the thread waits on input first. The wait returns as soon as the run starts, with its `turn_id`.
     - If the message never runs, `state` is `failed`, at once. `failure` is `operation_failed` when the launch or send failed or the message was removed from the queue, and `delivery_needs_attention` when Codemux can't tell whether it was delivered.
     - The result also holds `operation`: the receipt's `operation_id`, `state` and `error`.
     - An id that isn't yours is `operation_not_found`. A receipt for another thread, or for a launch without a message, is `invalid_target`.
   - **`turn_id`** follows that run only. A run that was stopped without a recorded end reads `failed` with `went_idle` once the thread is idle or a newer run has started. A run a restart cut off reads `running` until Codemux resumes the chat or someone reopens it. A `turn_id` the thread never ran is `invalid_target` once the thread is idle, or at rest after a restart with nothing queued or being delivered.
   - **Neither** follows the thread's current run. A thread that has never run reads `done` at once, also at rest after a restart. While its `thread_launch` is still starting it, it reads `running`.

   **Receipts.** `operation_status` reads a receipt without waiting:

   ```text
   operation_status {"operation_id": "5d2e…"}
   → {"state": "succeeded", "result": {"turn": {"turn_id": "c41b…", …}, …}, …}
   ```

   `accepted` and `running` mean Codemux took the request, not that the work is done. `failed` carries an `error` with a code. `needs_attention` means Codemux can't tell whether the work started: read the thread before you send anything again.

5. **Answer what it waits on.** `pending` lists what is open:
   - **Approvals** (`pending.approvals`): answer with `thread_respond`. Supervised assistants can only deny. Full access can allow once.

     ```text
     thread_respond {"workspace_id": "workspace-9", "thread_id": "8a1f…", "client_request_id": "deny-1", "request_id": "<from pending.approvals>", "decision": "deny"}
     ```

     A question that holds the run until it's answered, such as Claude's AskUserQuestion or Codex's `request_user_input`, is an approval with `request_kind` `"user-input"`, not a question. `thread_respond` can only allow it, with no answer, or deny it; `thread_answer` can't answer it. Only the person can answer it, in Codemux.

   - **Questions** (`pending.questions`): questions a Codex chat asks while it keeps working (`request_user_input_async`). Answer them with `thread_answer`, one answer per prompt, in order. It returns the `turn_id` of the run that continues; wait on that one.

     ```text
     thread_answer {"workspace_id": "workspace-9", "thread_id": "8a1f…", "client_request_id": "answer-1", "question_id": "<from pending.questions>", "answers": ["Use the staging database"]}
     → {"question_id": "…", "turn_id": "c41b…", "operation_id": "…", "state": "succeeded", …}
     ```

     An answer never changes the thread's permission mode. `question_closed` means someone answered or dismissed it already. `question_answer_failed` means the answer wasn't delivered, or its delivery isn't confirmed. If the message says the worker didn't take the answer, you can answer again with a new `client_request_id`. Otherwise don't: the person settles it in Codemux. If Codemux refused the answer because your access no longer covers the thread, you get that reason instead: `connector_disabled`, `grant_revoked`, `grant_changed`, `permission_ceiling` or `permission_state_unconfirmed`. Answering again won't help until the person changes that.
   - **Secrets** (`pending.secrets`): only the person can enter a secret, in Codemux. You see its label and reason, never a value. Tell the person, then wait again.

6. **Read more, or follow up.**

   ```text
   thread_read {"workspace_id": "workspace-9", "thread_id": "8a1f…", "limit": 20}
   thread_send {"workspace_id": "workspace-9", "thread_id": "8a1f…", "client_request_id": "follow-1", "message": "Now add a regression test."}
   ```

   - `thread_read` pages through the visible conversation with `cursor` (from `next_cursor`). It holds user messages and the assistant's replies, not hidden reasoning or raw tool output.
   - `thread_send` with the default `"delivery": "auto"` starts an idle thread, steers a busy one where that's safe, and otherwise queues. `queue`, `steer` and `interrupt` are explicit. Its receipt's `result.accepted_as` says which happened. Pass its `operation_id` to `thread_wait` to follow the run the message starts, queued or not.
   - `thread_interrupt` stops the current turn; pass `turn_id` so a newer run isn't cut short. `thread_stop` ends the provider session and its queue. The thread, its history and files stay.

7. **Clean up.**

   ```text
   workspace_close {"workspace_id": "workspace-9", "delete_worktree": true, "client_request_id": "close-9"}
   → {"workspace_id": "workspace-9", "closed": true, "worktree_removed": true, …}
   ```

   It stops the workspace's agents and terminals and runs its teardown. `delete_worktree` removes a worktree, never a project folder, and refuses one with uncommitted or unpushed work (`workspace_close_refused`). `delete_branch` also deletes its branch. `force_delete` removes it anyway; only assistants on this computer can use it.

**Errors.** A failed call is an MCP tool error (`isError: true`) whose text reads `code: message` and whose `structuredContent.error` holds `code`, `message` and `retryable`. An unknown tool is a JSON-RPC error.

A call Codemux accepted and that then failed is final for its `client_request_id`. Retrying it with the same id returns its receipt as a normal result, not an error: `{"operation_id": …, "state": "failed", "error": {"code": …, "message": …, "retryable": …}, …}`. A retry while the first call still runs returns the same receipt with `state` `accepted` or `running`; poll `operation_status`. When `error.retryable` is `true`, try again with a new `client_request_id`. A call refused before Codemux accepted it, such as one with a wrong argument or too little access, leaves its id unused.

If a client drops a `workspace_create`, `worktree_create`, `workspace_close` or `thread_answer` call (it disconnects, or a proxy times out), Codemux still finishes it. A retry with the same `client_request_id` returns the result, with `"replayed": true`.

## From the shell

The same tools work from scripts on this computer, with no MCP client:

```sh
codemux tools
codemux call workspace_list '{}'
WS=$(codemux worktree create --repo ~/src/api --branch fix/flaky-login)
THREAD=$(codemux thread start --workspace "$WS" --provider claude --permission-mode default -m "Find out why test_login is flaky")
codemux thread wait "$THREAD" --workspace "$WS" --timeout 10m
codemux thread send "$THREAD" --workspace "$WS" -m "Now fix it" --wait
codemux thread read "$THREAD" --workspace "$WS"
codemux workspace close "$WS" --remove-worktree
```

- `codemux tools` lists what this caller may run. `codemux call <tool> '<json>'` runs one (`-` reads the JSON from stdin), for example `agent_capabilities` to see the providers, models and permission modes. `codemux capabilities` describes every command and tool as JSON, without a running app.
- `worktree create` and `workspace create ~/src/api` print the workspace id. `thread start` prints the thread id, and `thread send` prints `Sent.`, `Steered.` or `Queued.` With `--wait`, both print the reply. `thread start --worktree-branch fix/flaky-login` creates the worktree and starts the thread in one step.
- Name the caller before the command: `codemux --client nightly-script --access supervised thread start …`.
- `--workspace` defaults to `$CODEMUX_WORKSPACE_ID`, which Codemux sets in its own terminals.
- `--json` prints one JSON object, even on failure.
- `thread respond "$THREAD" "$REQUEST_ID" deny --workspace "$WS"` answers an approval. `thread answer` answers a question, with one `--answer` per prompt, in order. It prints the `turn_id` of the run that continues:

  ```sh
  TURN=$(codemux thread answer "$THREAD" --workspace "$WS" --question "$QUESTION" --answer "Use the staging database")
  codemux thread wait "$THREAD" --workspace "$WS" --turn "$TURN"
  ```

- `workspace close --remove-worktree` also removes the worktree; it refuses one with uncommitted or unpushed work unless you add `--force`.
- Every mutation takes `--request-id` (`--request-key` for `thread respond`). Reuse it to retry safely.
- `--wait` follows the run its message starts, also when the message was queued, and stops when that run ends or needs input. A message that never runs exits `1` with its receipt's error.
- `thread wait` follows the thread's current run, then any runs queued behind it. It stops when a run needs input, or when a run ends with nothing queued after it. `--turn` keeps it on one run.
- Neither stops while the work goes on. Pass `--timeout` in scripts.

Exit codes: `0` done, `1` failed or error, `2` needs input, `3` timed out, `64` usage error.

`codemux call` prints the tool's result unchanged, under `result`, and exits `0` for any result that isn't a tool error. That includes a receipt whose `state` is `failed`, which a retry of a failed keyed call returns (see "Errors" above). After `codemux call`, check `state` yourself. The typed verbs, such as `thread answer` and `workspace close`, exit `1` for a failed receipt.

`codemux json <command> '<json>'` sends one raw control-socket command. It exits `1` when the reply is not ok. Scripts should prefer `codemux call`.

## Access levels

| | Read-only | Supervised | Full access |
|---|---|---|---|
| Read workspaces, threads, providers and models; wait for results | Yes, but only the model catalogs Codemux has already read | Yes | Yes |
| Start threads, send messages, interrupt, stop, answer questions, run plugin actions | No | Yes, for threads in supervised permission modes | Yes |
| Answer approvals | No | Deny only | Allow once or deny |
| Open workspaces, create worktrees (also `thread_launch` with `worktree`), close and remove them | No | No | Yes |

- Supervised permission modes are the ones that ask before acting, such as Claude's `default`, `acceptEdits` and `plan`, or Codex's `read-only`, `workspace-write` and `plan`. `agent_capabilities` lists the ones you may use as `allowed_permission_modes`.
- Opening a workspace or creating a worktree runs the project's setup scripts. That's why those need full access.
- `allow_always`, `force_delete`, and `worktree_create` with a terminal preset (`agent_preset_id`) are only for assistants on this computer.
- Assistants on this computer at full access also get the older administration tools of `codemux mcp`, such as `automation_create`, `terminal_write` and `preset_apply`. Network assistants never do.
- Approving connections, Remote Access and pairing devices are never MCP tools. Use Settings or the `codemux` CLI.
- Agents that Codemux started are refused administration commands, even through the `codemux` CLI (`admin_refused`). They use their chat's own Codemux tools for threads and workspaces (`agent_identity_required` otherwise).
- Codemux runs a project's setup and teardown scripts, and every git command it starts, as such agents, whoever asked for them. So a `codemux` command in a setup or teardown script, a git hook or a `core.fsmonitor` command gets the same refusals. Commands you type in your own terminal aren't affected.

## Security

- **An assistant on this computer runs as you.** Names and access levels set limits and record who did what. They don't isolate one program from another under the same account.
- **Full access lets an assistant run any program as you**, through the agents it starts and the setup scripts it triggers. Agents that Codemux started, setup and teardown scripts, and git hooks are refused administration commands as a safety net, not as a boundary. Give bots Supervised unless you trust everything that can reach them.
- **Chat bots relay other people's words.** A message to your Hermes bot can try to steer what it asks Codemux to do. That's prompt injection. Keep bots supervised, so approvals stay with you. Never approve worker requests automatically from a bot. Limit who can message the bot.
- **What a thread says is data, not instructions.** `final_text` and `thread_read` can quote files, web pages or tool output that contain hostile text.
- **Access covers every workspace.** Any level can read every workspace and every visible conversation, not just one project.
- **An SSH key that reaches Codemux is a shell** on that computer, unless `authorized_keys` restricts it to `codemux mcp`. See [A bot over SSH](#a-bot-over-ssh).
- **Tokens are passwords.** Give each bot its own token. Keep it in a secret store or `.env` file, never in a repository. Revoke it when you're done. Off this computer, use HTTPS only.
- **Approve only a sign-in you just started.** The name a client shows is its own claim, not proof.

## Supported MCP versions

- Through `initialize`: 2025-11-25, 2025-06-18, 2025-03-26 and 2024-11-05. Codemux answers with the version the client asks for, or with 2025-11-25 when it asks for another one.
- Through `server/discover`: 2026-07-28, on `codemux mcp` and on the network door. Clients that support it, such as Claude Code, try it first and fall back to `initialize`. Each such request names its version and `clientCapabilities` in `_meta`. A version Codemux doesn't speak gets error -32022, which lists the ones it does.
- `codemux-remote mcp` speaks only the `initialize` versions.
- Over HTTP, a request without an `MCP-Protocol-Version` header counts as 2025-03-26.
- Batched requests are refused.

## Remote devices and phones

### Agents on your devices

When you add a server under **Settings → Connections → Devices**, Codemux installs `codemux-remote` there. An agent on that server can use `codemux-remote mcp` to register folders and git worktrees on the server, run shells there and check the helper. Workspaces it registers show up under Devices in the Codemux app. It can't start or message Codemux chats.

On a server where the Codemux app has never run, `codemux-remote` adds itself to Claude Code (`~/.claude.json`) and Vexis (`~/.vexis/mcp-servers.yaml`) as `codemux`, but only if no `codemux` entry exists. It never changes an entry that runs another program. On a machine where the Codemux app runs, it adds nothing and removes the entry it added earlier, because there `codemux` means `codemux mcp`.

Outside agents see workspaces on your devices in `workspace_list` with `native_control_available: false`. Thread tools can't target them yet.

To control Codemux from another machine, use SSH or **Over the network**, not `codemux-remote`: `codemux-remote` listens only on its own server and manages that server, not your desktop.

### Phones and browsers

A paired phone or browser can remove a clean worktree, for example with **Devices → Sweep**. Codemux refuses one with uncommitted or unpushed work. Force delete, deleting the branch together with the worktree, and deleting archived workspaces are only in the desktop app. Merging a pull request from the phone can delete its branch, as on the desktop.

## If you used Codemux's MCP server before

- **The network door moved to its own port, 4378.** It used to share Remote Access's port 4377, and it no longer needs Remote Access. Point your proxy at port 4378. Assistants that used `http://127.0.0.1:4377/mcp` directly need the new URL and a new sign-in.
- **Network sign-ins renew themselves**, and bots can use labelled tokens. A used refresh token that comes back more than 30 seconds later ends its sign-in, so don't copy a client's saved sign-in between machines. Hermes can now connect over HTTP.
- **`codemux mcp` lists 76 tools at full access.** `workspace_list`, `workspace_info`, `workspace_create`, `worktree_create` and `workspace_close` are now control tools there. If your scripts called them before:
  - All five refuse unknown arguments (`invalid_arguments`), return compact JSON with `structuredContent`, and report errors as `isError` results with a code.
  - `workspace_list` returns summaries; `"detail": "full"` returns the old snapshots. Rows add `worktree_path` and `parent_workspace_id`, and workspaces on other machines read `native_control_available: false`.
  - `workspace_info` takes an optional `workspace_id`. Without one, while no workspace is focused in Codemux, it is the error `no_active_workspace`, not a result with an `error` key.
  - `workspace_create` returns the workspace already open in that folder (`created: false`) instead of opening a second one. Its `path` must be an existing folder, absolute or starting with `~`.
  - `workspace_create` and `worktree_create` don't take focus unless you pass `"select": true`.
  - `workspace_close` needs full access, also when it keeps the worktree.
  - `workspace_create`, `worktree_create` and `workspace_close` take a `client_request_id`, so a retry returns the first call's result.
  - A `codemux mcp` that serves a Codemux agent, or that a workspace's `.mcp.json` starts, keeps the older versions unchanged.
- **`--access` narrows `codemux mcp`.** A supervised or read-only client is offered fewer tools, and it refuses to work with an older Codemux that can't enforce its access level (`server_too_old`).
- **`thread_launch` needs `permission_mode`.** Without it, it fails with `invalid_arguments` (missing field `permission_mode`).
- **`agent_capabilities` with `provider` reads that provider's catalog** if Codemux hasn't yet, for a Supervised or Full access caller. It used to list only catalogs Codemux had already read, so a provider no chat had used yet offered no models. A Read-only caller still gets only those.
- **`thread_wait` returns `state`, `final_text`, `pending` and `wait_cursor`.** It waits up to 50 seconds by default and 10 minutes at most. With `operation_id` it follows the run a launch or send starts, also from the queue.
- **Outside assistants can answer worker questions** in `pending.questions` with `thread_answer`. A question that arrives as a `"user-input"` approval, such as Claude's AskUserQuestion, can only be allowed or denied.
- **New worktrees branch from the repository's default branch**, not always `main`.
- **`codemux json` and the older `codemux` commands exit `1` when the reply is not ok.** That includes the refusals below. Check scripts that use `set -e`.
- **Agents that Codemux started are refused administration commands**, through MCP and through the `codemux` CLI, also on devices that run `codemux serve`. That includes Claude Code started in a workspace folder (see [Inside a Codemux workspace folder](#inside-a-codemux-workspace-folder)).
- **Setup and teardown scripts and git hooks run as Codemux agents.** This covers a project's setup and teardown commands, and the hooks and `core.fsmonitor` commands of every git command Codemux runs, itself or through `gh` and `glab`. A `codemux` command in them gets `admin_refused` for administration commands and `agent_identity_required` for control tools, and exits `1`. Such a setup command now stops the setup, and such a teardown command stops the close, unless it ends with `|| true`.
- **Codex chats that Codemux starts turn off your own Codemux MCP entries**: every entry that runs `codemux mcp`, whatever its name, and any entry named `codemux`. Those chats have their own Codemux tools.
- **Chats with "Let agents work with other chats" turned off** (Settings → Agents → Models & behavior) are offered no Codemux thread or plugin tools.
- **`codemux-remote` no longer replaces a `codemux` entry it doesn't own.**

## Troubleshooting

- **`app_unavailable` from `codemux mcp`, `codemux_unavailable` from the `codemux` CLI, or a connection error.** Codemux isn't running, or the command can't find it: start the app or `codemux serve`, then retry the call. From Codex, see the `XDG_RUNTIME_DIR` note under [Codex](#codex).
- **`server_too_old` from `codemux mcp` or the CLI, or `codemux_outdated` from the CLI.** The running Codemux is older than the `codemux` command: restart Codemux after an update. `server_too_old` means a read-only or supervised client refused to work with a Codemux that can't enforce its access level. From `codemux mcp`, a tool the older Codemux lacks fails with "Unknown control command".
- **Only the older tools, no `thread_*`.** Claude Code is using a workspace folder's `.mcp.json`; see [Inside a Codemux workspace folder](#inside-a-codemux-workspace-folder). Or a Codemux agent started this program, so it counts as that agent.
- **`admin_refused` or `agent_identity_required`.** A Codemux agent started this program, or it runs in a setup or teardown script or a git hook that Codemux ran. Run it from your own terminal.
- **`access_denied`.** The access level is too low for that tool. Raise `--access`, or approve or issue the network assistant again with more access.
- **`permission_ceiling`.** That permission mode needs full access. Pick one from `allowed_permission_modes`.
- **A wait ends with a client timeout.** Lower `timeout_ms`, or raise the client's tool timeout (`timeout` for Hermes, `tool_timeout_sec` for Codex).
- **"Couldn’t listen on port …".** Another program uses the port. Pick another one in Settings, then update your proxy and URLs.
- **Sign-in asks again after you changed the origin or port.** Sign-ins and tokens are bound to the URL. Sign in again and issue new tokens.
- **`~/.claude.json` has a `codemux` entry that runs `codemux-remote mcp`.** An older `codemux-remote` wrote it, and it has no thread tools. Current versions remove it on a machine where the Codemux app runs, the next time `codemux-remote` starts. To fix it now, run `claude mcp remove --scope user codemux`, then add the entry above. `codemux-remote` never touches an entry that runs another program.
- **`codemux-remote` refuses `workspace_info` with "missing field `id`".** Its background service still runs the old version. Run `systemctl --user restart codemux-remote` on that device.

## Error codes

The codes an outside assistant meets most often. "Retry" means retrying the identical call later can succeed without anyone changing anything. If Codemux had accepted the first call, the retry needs a new `client_request_id` (see [Errors](#the-end-to-end-flow) in the flow).

<!-- error-codes:start -->
| Code | Means | Retry |
|---|---|---|
| `invalid_arguments` | An argument is missing, unknown or out of range; the message names it | No |
| `access_denied` | Your access level is too low for this tool | No |
| `permission_ceiling` | The permission mode or decision needs full access | No |
| `permission_state_unconfirmed` | Supervised: the worker's live permission mode doesn't match its saved one. The person applies the mode in Codemux or restarts the worker | No |
| `permission_denied` | Not allowed over the network (`allow_always`, `force_delete`), or read-only | No |
| `admin_refused` | An agent that Codemux started asked for an administration command | No |
| `agent_identity_required` | An agent that Codemux started called a control tool; it uses its chat's own tools | No |
| `request_key_conflict` | This `client_request_id` was used with other arguments | No |
| `operation_failed` | `codemux` CLI only: the app's error carried no code, or a receipt ended without an error of its own. As a `thread_wait` `failure`, the message never ran | No |
| `operation_not_found` | No operation with that id belongs to this assistant | No |
| `workspace_not_found` | No open workspace has that id | No |
| `no_active_workspace` | `workspace_info` without an id, and no workspace is focused in Codemux | No |
| `thread_not_found` | No thread has that id in that workspace | No |
| `invalid_target` | `thread_wait` can't follow that `turn_id` or `operation_id` in this thread | No |
| `unsupported_remote_workspace` | The workspace is on another machine; thread tools can't target it | No |
| `unknown_project_root` | Network assistants may only open folders of projects already open in Codemux | No |
| `not_a_git_repository` | A worktree needs a git repository | No |
| `worktree_conflict` | The branch or its checkout is already in use | No |
| `worktree_base_unavailable` | The base branch can't be read, or origin can't be fetched when it must be | No |
| `workspace_close_refused` | The worktree can't be removed safely; the message says why, such as uncommitted or unpushed work | No |
| `question_not_found` | No open question has that id | No |
| `question_closed` | The question was answered or dismissed already | No |
| `question_answer_failed` | The answer wasn't delivered or confirmed. Answer again, with a new id, only if the worker refused it; otherwise the person settles it in Codemux | No |
| `provider_not_ready` | The provider isn't ready: not installed, signed out or still starting | Yes |
| `wait_budget_exhausted` | Too many waits at once on the network door | Yes |
| `app_unavailable` | `codemux mcp` can't reach Codemux: it isn't running, or its socket can't be found. The `codemux` CLI reports `codemux_unavailable` | Yes |
| `server_too_old` | A read-only or supervised client met a Codemux too old to enforce its access level; restart Codemux. Other calls an older Codemux can't run are `codemux_outdated` from the CLI | No |
| `grant_revoked` | This network assistant's access was revoked | No |
| `connector_disabled` | **Over the network** is off | No |
<!-- error-codes:end -->

## Tool reference

<!-- tool-reference:start -->
| Tool | What it does | Needs |
|---|---|---|
| `workspace_list` | List open workspaces and their ids | Read-only |
| `workspace_info` | Read one workspace's full snapshot | Read-only |
| `workspace_create` | Open a folder as a workspace, or return the one open there | Full access |
| `worktree_create` | Create a git worktree and its workspace | Full access |
| `workspace_close` | Close a workspace, optionally removing its worktree and branch | Full access |
| `agent_capabilities` | Providers, models, efforts and permission modes for a workspace | Read-only |
| `thread_list` | Threads in a workspace | Read-only |
| `thread_launch` | Start a thread, optionally in a new worktree | Supervised; full access with `worktree` |
| `thread_send` | Send a follow-up message | Supervised |
| `thread_read` | Page through a thread's visible conversation | Read-only |
| `thread_status` | A thread's state and what it waits on, without waiting | Read-only |
| `thread_wait` | Wait for a run to finish or need input; returns state, final text and pending input | Read-only |
| `thread_interrupt` | Interrupt the current turn | Supervised |
| `thread_stop` | Stop the provider session and its queue | Supervised |
| `thread_respond` | Answer an approval | Supervised (deny); full access to allow |
| `thread_answer` | Answer a worker's question | Supervised |
| `operation_status` | Read a change's receipt | Read-only |
| `plugin_list` | Plugin actions exposed to agents | Read-only |
| `plugin_invoke` | Run a plugin action | Supervised |
<!-- tool-reference:end -->
