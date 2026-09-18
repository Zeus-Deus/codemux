// CI-only WebDriver acceptance against an installed, unmodified release app.
// Never run on a developer profile. No embedded driver or production test hooks.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { cpus, totalmem, release } from "node:os";
import { createServer } from "node:http";
import { mkdir, mkdtemp, readFile, readdir, writeFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

assert.equal(
  process.env.GITHUB_ACTIONS,
  "true",
  "Requires disposable GitHub CI",
);
assert.equal(
  process.env.RUNNER_ENVIRONMENT,
  "github-hosted",
  "Refuses persistent/self-hosted runners",
);
assert.ok(process.env.RUNNER_TEMP);
const root = await mkdtemp(join(process.env.RUNNER_TEMP, "codemux-native-ui-"));
const evidenceDir = resolve("addon-native-ui-evidence");
await mkdir(evidenceDir, { recursive: true });
const evidence = {
  commit: process.env.GITHUB_SHA,
  platform: process.platform,
  hardware: {
    osRelease: release(),
    cpu: cpus()[0]?.model,
    logicalCpus: cpus().length,
    memoryBytes: totalmem(),
  },
  checks: [],
  failedChecks: [],
  seams: [
    "Synthetic loopback account API; no real account or credentials",
    "Native file chooser selection supplies package fixture path; all addon IPC remains native",
  ],
};
evidence.installerBuild = JSON.parse(
  await readFile("addon-native-ui-build.json", "utf8"),
);
assert.equal(evidence.installerBuild.platform, process.platform);
const owned = new Set();
function start(executable, args, options = {}) {
  const child = spawn(executable, args, {
    stdio: ["ignore", "pipe", "pipe"],
    ...options,
  });
  owned.add(child);
  let output = "";
  child.stdout.on("data", (chunk) => {
    output = (output + chunk).slice(-100000);
  });
  child.stderr.on("data", (chunk) => {
    output = (output + chunk).slice(-100000);
  });
  child.done = new Promise((done, fail) => {
    child.once("error", (error) => {
      owned.delete(child);
      fail(error);
    });
    child.once("exit", (code) => {
      owned.delete(child);
      done({ code, output });
    });
  });
  child.done.catch(() => {});
  return child;
}
async function run(executable, args, options = {}) {
  const child = start(executable, args, options);
  const timer = setTimeout(() => child.kill(), 120000);
  try {
    const result = await child.done;
    assert.equal(result.code, 0, `${executable}: ${result.output}`);
    return result.output;
  } finally {
    clearTimeout(timer);
  }
}
async function files(path) {
  const result = [];
  for (const entry of await readdir(path, { withFileTypes: true })) {
    if (entry.isDirectory())
      result.push(...(await files(join(path, entry.name))));
    else if (entry.isFile()) result.push(join(path, entry.name));
  }
  return result;
}
const bundle = resolve(process.argv[2] ?? "src-tauri/target/release/bundle");
const installers = (await files(bundle)).filter((file) =>
  file.endsWith(process.platform === "win32" ? ".exe" : ".deb"),
);
assert.equal(installers.length, 1);
evidence.installerSha256 = createHash("sha256")
  .update(await readFile(installers[0]))
  .digest("hex");
const installDir = join(root, "app");
let application;
if (process.platform === "win32") {
  await run(installers[0], ["/S", `/D=${installDir}`]);
  application = join(installDir, "codemux.exe");
} else {
  await run("sudo", ["dpkg", "--install", installers[0]]);
  application = "/usr/bin/codemux";
}

// The public release supports CODEMUX_API_URL and normal token login. Route only
// account traffic to this synthetic server; plugin HTTPS still uses its broker.
const token = "ci-synthetic-session-not-a-real-credential";
const account = createServer((req, res) => {
  res.setHeader("Content-Type", "application/json");
  if (
    req.url === "/api/auth/desktop/verify" &&
    req.headers.authorization === `Bearer ${token}`
  ) {
    res.end(
      JSON.stringify({
        user: {
          id: "ci-addon-user",
          email: "addons@example.invalid",
          name: "Add-on CI",
          image: null,
        },
        session: { expiresAt: "2099-01-01T00:00:00Z" },
      }),
    );
  } else {
    res.statusCode = 404;
    res.end('{"error":"Synthetic CI account has no remote services"}');
  }
});
await new Promise((done) => account.listen(0, "127.0.0.1", done));
const env = {
  ...process.env,
  CODEMUX_API_URL: `http://127.0.0.1:${account.address().port}`,
  WEBKIT_DISABLE_COMPOSITING_MODE: "1",
};
let session;
let driver;
let desktop;
let windowsDebugPolicy = false;
let capabilities;
const endpoint = "http://127.0.0.1:4444";
async function request(method, path, body) {
  const response = await fetch(endpoint + path, {
    method,
    headers: { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(60000),
  });
  const data = await response.json();
  if (!response.ok || data.value?.error)
    throw Error(`${method} ${path}: ${JSON.stringify(data)}`);
  return data.value;
}
const wd = (method, path, body) =>
  request(method, `/session/${session}${path}`, body);
const script = (source, ...args) =>
  wd("POST", "/execute/sync", { script: source, args });
async function native(command, args = {}) {
  const result = await wd("POST", "/execute/async", {
    script: `const done = arguments[arguments.length - 1]; window.__TAURI_INTERNALS__.invoke(arguments[0], arguments[1]).then(value => done({value}), error => done({error: JSON.stringify(error)}));`,
    args: [command, args],
  });
  assert.equal(result.error, undefined, `${command}: ${result.error}`);
  return result.value;
}
async function until(label, fn, timeout = 30000) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try {
      const result = await fn();
      if (result) return result;
    } catch (e) {
      last = e;
    }
    await delay(250);
  }
  throw Error(`Timed out: ${label}${last ? `: ${last.message}` : ""}`);
}
const text = () => script("return document.body.innerText");
const hasText = (value) =>
  until(value, async () => (await text()).includes(value));
async function element(css) {
  return until(css, () =>
    wd("POST", "/element", { using: "css selector", value: css }),
  );
}
const composer = '[data-testid="composer-body"] textarea';
const elementId = (el) => el["element-6066-11e4-a52e-4f735466cecf"];
async function clickText(value, scope = "document") {
  const el = await until(`click ${value}`, () =>
    script(
      `return [...${scope}.querySelectorAll('button,[role="menuitem"],[role="option"]')].find(e => e.getClientRects().length && !e.disabled && e.getAttribute('aria-disabled') !== 'true' && (e.innerText.trim() === arguments[0] || e.getAttribute('aria-label') === arguments[0])) ?? null`,
      value,
    ),
  );
  await wd("POST", `/element/${elementId(el)}/click`, {});
}
async function click(css) {
  const el = await element(css);
  await wd("POST", `/element/${elementId(el)}/click`, {});
}
async function type(css, value) {
  const el = await element(css);
  await until(`enabled ${css}`, () =>
    wd("GET", `/element/${elementId(el)}/enabled`),
  );
  await wd("POST", `/element/${elementId(el)}/value`, { text: value });
}
async function typeComposer(value) {
  // Use explicit paired keys and release prior modifier state. Verify the
  // controlled value before invoking a plugin, so driver input cannot be
  // mistaken for a plugin draft-preservation or auto-submit failure.
  await wd("DELETE", "/actions");
  await click(composer);
  await wd("POST", "/actions", {
    actions: [
      {
        type: "key",
        id: "keyboard",
        actions: [...value].flatMap((value) => [
          { type: "keyDown", value },
          { type: "keyUp", value },
          { type: "pause", duration: 20 },
        ]),
      },
    ],
  });
  await until("controlled composer input", () =>
    script(
      `return [...document.querySelectorAll(arguments[0])].some(e => e.value.includes(arguments[1]))`,
      composer,
      value,
    ),
  );
}
async function pluginHostCount() {
  // Read-only inventory on the disposable job VM, including orphaned hosts.
  if (process.platform === "win32") {
    return Number(
      (
        await run("powershell.exe", [
          "-NoProfile",
          "-Command",
          "@(Get-Process | Where-Object { $_.ProcessName -like 'codemux-addon-host*' }).Count",
        ])
      ).trim(),
    );
  }
  return (await run("ps", ["-eo", "comm="]))
    .split("\n")
    .filter((name) => name.trim().startsWith("codemux-addon-h")).length;
}
let terminalProbe = 0;
async function checkCoreTerminal() {
  const marker = `CODEMUX_CORE_${++terminalProbe}`;
  await wd("DELETE", "/actions");
  const screen = await element(".xterm-screen");
  const rect = await wd("GET", `/element/${elementId(screen)}/rect`);
  // The shell-starting badge covers the centre of a narrow terminal. Focus
  // its visible first row using a real pointer action, not a DOM focus bypass.
  await wd("POST", "/actions", {
    actions: [
      {
        type: "pointer",
        id: "mouse",
        parameters: { pointerType: "mouse" },
        actions: [
          {
            type: "pointerMove",
            origin: "viewport",
            x: Math.round(rect.x + 12),
            y: Math.round(rect.y + 12),
          },
          { type: "pointerDown", button: 0 },
          { type: "pointerUp", button: 0 },
        ],
      },
    ],
  });
  await type("textarea.xterm-helper-textarea", `echo ${marker}`);
  await wd("POST", "/actions", {
    actions: [
      {
        type: "key",
        id: "keyboard",
        actions: [
          { type: "keyDown", value: "\uE007" },
          { type: "keyUp", value: "\uE007" },
        ],
      },
    ],
  });
  await until("native terminal command returned", () =>
    script(
      `return [...document.querySelectorAll('.xterm')].some(e => e.innerText.replace(/\\s+/g, '').split(arguments[0]).length >= 3)`,
      marker,
    ),
  );
}
async function shortcut(key) {
  await wd("POST", "/actions", {
    actions: [
      {
        type: "key",
        id: "keyboard",
        actions: [
          { type: "keyDown", value: "\uE009" },
          { type: "keyDown", value: key },
          { type: "keyUp", value: key },
          { type: "keyUp", value: "\uE009" },
        ],
      },
    ],
  });
}
async function capture(name) {
  await writeFile(join(evidenceDir, `${name}.txt`), await text());
  if (process.platform === "linux") {
    // WebKitWebDriver's snapshot hangs after the terminal canvas appears on
    // the Ubuntu runner. Capture the actual disposable X display instead.
    await run("import", ["-window", "root", join(evidenceDir, `${name}.png`)]);
  } else {
    await writeFile(
      join(evidenceDir, `${name}.png`),
      Buffer.from(await wd("GET", "/screenshot"), "base64"),
    );
  }
}
async function step(name, fn, continueAfterFailure = false) {
  console.log(`Native UI: ${name}`);
  try {
    await fn();
    evidence.checks.push(name);
    await capture(name);
  } catch (error) {
    if (!continueAfterFailure) throw error;
    evidence.failedChecks.push({ name, error: String(error) });
    await capture(`${name}-failed`);
  }
}
async function openSettings() {
  await shortcut(",");
  await clickText("Add-ons");
  await hasText("Import package");
}
async function openCommand(title) {
  await shortcut("k");
  await type('[role="combobox"]', title);
  await until(title, () =>
    script(
      `return [...document.querySelectorAll('[role="option"]')].find(e => e.innerText.includes(arguments[0])) ?? null`,
      title,
    ),
  ).then((el) => wd("POST", `/element/${elementId(el)}/click`, {}));
}
async function createNativeSession() {
  const created = await request("POST", "/session", {
    capabilities: { alwaysMatch: capabilities },
  });
  session = created.sessionId;
  await wd("POST", "/timeouts", {
    implicit: 0,
    script: 30000,
    pageLoad: 60000,
  });
  await until("native app ready", () =>
    script(
      "return !!window.__TAURI_INTERNALS__ && document.body.innerText.length > 50",
    ),
  );
  await element('button[aria-label="Menu"]');
}
async function restartNativeSession() {
  // The driver owns the Linux app child. On Windows attach mode we retain the
  // exact process handle ourselves. Never stop a process by name or pattern.
  await wd("DELETE", "");
  session = undefined;
  if (process.platform === "win32") {
    desktop.kill();
    await Promise.race([
      desktop.done,
      delay(5000).then(() => {
        throw Error("Owned desktop did not exit");
      }),
    ]);
    await until("old WebView2 debugger closed", async () => {
      try {
        await fetch("http://127.0.0.1:9231/json/version", {
          signal: AbortSignal.timeout(500),
        });
        return false;
      } catch {
        return true;
      }
    });
    desktop = start(application, [], { env });
    await until(
      "restarted WebView2 ready",
      async () =>
        (
          await fetch("http://127.0.0.1:9231/json/version", {
            signal: AbortSignal.timeout(1000),
          })
        ).ok,
    );
  }
  await createNativeSession();
}

try {
  await run(application, ["login", "--token", token], { env });
  if (process.platform === "win32") {
    // The standard driver-launch path times out for this stock Windows app.
    // Microsoft's documented attach mode keeps the same binary and gives us
    // the owned process's startup diagnostics. Elevated runners ignore env
    // overrides, so the app-specific policy is temporary and CI-only.
    await run("powershell.exe", [
      "-NoProfile",
      "-File",
      "scripts/addons/windows-webview-debug.ps1",
      "-Mode",
      "enable",
    ]);
    windowsDebugPolicy = true;
    desktop = start(application, [], {
      env: {
        ...env,
        WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: "--remote-debugging-port=9231",
      },
    });
    await until("stock WebView2 startup", async () => {
      const response = await fetch("http://127.0.0.1:9231/json/version", {
        signal: AbortSignal.timeout(1000),
      });
      return response.ok;
    });
    driver = start(
      "msedgedriver.exe",
      [
        "--port=4444",
        "--verbose",
        `--log-path=${join(evidenceDir, "edge-driver.log")}`,
      ],
      { env },
    );
    capabilities = {
      browserName: "webview2",
      "ms:edgeOptions": { debuggerAddress: "127.0.0.1:9231" },
    };
    evidence.windowsDriverMode =
      "Microsoft WebView2 attach (app-specific disposable runner policy)";
  } else {
    driver = start("tauri-driver", ["--port", "4444"], { env });
    capabilities = { "tauri:options": { application } };
  }
  await until(
    "driver startup",
    async () => (await request("GET", "/status"))?.ready === true,
  );
  await createNativeSession();
  await step("01-settings", openSettings);
  assert.deepEqual((await native("addon_inventory")).installed, []);
  assert.equal(
    await pluginHostCount(),
    0,
    "Clean startup must have no plugin hosts",
  );
  evidence.checks.push("01-no-plugin-hosts-at-clean-start");
  for (const [slug, title, id] of [
    ["project-brief", "Project Brief", "codemux.project-brief"],
    ["issue-companion", "Issue Companion", "codemux.issue-companion"],
    ["fault-isolation", "Fault Isolation Fixture", "example.fault-isolation"],
  ]) {
    await step(`02-import-${slug}`, async () => {
      const path = resolve(
        slug === "fault-isolation"
          ? `scripts/addons/fixtures/${slug}/${id}-1.0.0.cmxaddon`
          : `examples/addons/${slug}/${id}-1.0.0.cmxaddon`,
      );
      // Tauri defines invoke as non-writable. Intercept only the exact chooser
      // transport URL, then restore fetch before the real review command. No
      // addon command, plugin HTTP request, or broker result is intercepted.
      await script(
        `const original = window.fetch;
        const path = arguments[0];
        const chooserUrl = window.__TAURI_INTERNALS__.convertFileSrc('plugin:dialog|open', 'ipc');
        window.__addonChooserConsumed = false;
        const selectFixture = function(input, options) {
          if (input === chooserUrl) {
            window.fetch = original;
            window.__addonChooserConsumed = true;
            return Promise.resolve(new Response(JSON.stringify(path), {
              status: 200, headers: {'Content-Type': 'application/json', 'Tauri-Response': 'ok'}
            }));
          }
          return original.call(this, input, options);
        };
        window.fetch = selectFixture;
        if (window.fetch !== selectFixture) throw Error('Chooser fixture could not be installed');`,
        path,
      );
      await clickText("Import package");
      await hasText(`Review ${title}`);
      assert.equal(await script("return window.__addonChooserConsumed"), true);
      await hasText("SHA-256");
      await capture(`review-${slug}`);
      await clickText("Accept and install");
      await until(`native installation ${slug}`, async () =>
        (await native("addon_inventory")).installed.some(
          (i) => i.manifest.id === id && i.desiredEnabled,
        ),
      );
    });
  }
  await step("03-disable-enable", async () => {
    // Install & enable includes the specification's candidate/normal activation
    // transaction. Test lazy enablement separately after explicitly stopping it.
    for (const [title, id] of [
      ["Project Brief", "codemux.project-brief"],
      ["Issue Companion", "codemux.issue-companion"],
      ["Fault Isolation Fixture", "example.fault-isolation"],
    ]) {
      const article = `([...document.querySelectorAll('article')].find(e => e.innerText.includes(${JSON.stringify(title)})))`;
      await clickText("Disable", article);
      await until(
        `disabled ${id}`,
        async () =>
          !(await native("addon_inventory")).installed.find(
            (i) => i.manifest.id === id,
          ).desiredEnabled,
      );
      await clickText("Enable", article);
      await until(
        `enabled ${id}`,
        async () =>
          (await native("addon_inventory")).installed.find(
            (i) => i.manifest.id === id,
          ).desiredEnabled,
      );
    }
    assert.equal(
      await pluginHostCount(),
      0,
      "Enabling inert contributions must not eagerly start hosts",
    );
    evidence.checks.push("03-enabled-plugins-remain-lazy");
  });
  await step("03-issue-configuration", async () => {
    await clickText(
      "Configure / Permissions",
      `([...document.querySelectorAll('article')].find(e => e.innerText.includes('Issue Companion')))`,
    );
    for (const [label, value] of [
      ["Repository owner", "octocat"],
      ["Repository name", "Hello-World"],
    ]) {
      const input = await until(label, () =>
        script(
          `return [...document.querySelectorAll('label')].find(e => e.innerText.trim() === arguments[0])?.querySelector('input') ?? null`,
          label,
        ),
      );
      await until(`loaded ${label}`, () =>
        wd("GET", `/element/${elementId(input)}/enabled`),
      );
      await wd("POST", `/element/${elementId(input)}/value`, { text: value });
    }
    await clickText("Save settings");
    await hasText("Saved");
    const settings = await native("addon_settings_get", {
      id: "codemux.issue-companion",
    });
    assert.equal(settings.owner, "octocat");
    assert.equal(settings.repository, "Hello-World");
  });
  const project = join(root, "synthetic-project");
  await mkdir(project);
  await run("git", ["init", "--initial-branch=main", project]);
  await writeFile(
    join(project, "README.md"),
    "# Native add-on acceptance fixture\n",
  );
  // Keep the fixture independent of app-generated discovery files. Repository
  // info/exclude preservation has a separate native Git regression test.
  await writeFile(join(project, ".gitignore"), ".mcp.json\n");
  await run("git", ["-C", project, "add", "README.md", ".gitignore"]);
  await run("git", [
    "-C",
    project,
    "-c",
    "user.name=Add-on CI",
    "-c",
    "user.email=addons@example.invalid",
    "commit",
    "-m",
    "fixture",
  ]);
  await writeFile(
    join(project, "draft-context.txt"),
    "Synthetic untracked file\n",
  );
  await click('[aria-label="Close settings"]');
  const workspaceId = await native("create_workspace", { cwd: project });
  const paneId = await native("agent_chat_create_pane", {
    workspaceId,
    cwd: project,
    provider: null,
    launchMode: null,
    threadId: null,
  });
  // Native setup creates the workspace, but only a normal frontend activation
  // dismisses the startup Home draft. Select the real workspace via the palette
  // instead of mutating the app's client stores from the test.
  await shortcut("k");
  await type('[role="combobox"]', "synthetic-project");
  await click(`[role="option"][data-value="ws:${workspaceId}"]`);
  await element(composer);
  // Existing app behavior warms a provider when an unbound pane first gains
  // focus, replacing its provisional composer. Finish that normal transition
  // before typing or mounting a plugin; no provider CLI is installed in CI.
  await click(composer);
  const findPane = (value) => {
    if (!value || typeof value !== "object") return null;
    if (value.pane_id === paneId) return value;
    return Object.values(value).map(findPane).find(Boolean) ?? null;
  };
  const threadId = await until(
    "normal chat pane binding",
    async () => findPane(await native("get_app_state"))?.thread_id,
  );
  await hasText("Session error");
  const messagesBefore = await native("agent_chat_list_messages", { threadId });
  assert.ok(
    !messagesBefore.some((row) => JSON.parse(row).type === "user_message"),
  );
  const sessionsBefore = await native("agent_chat_list_sessions", {
    workspaceId,
  });
  async function assertNoSubmission() {
    // Session lists omit rows without an SDK cursor. Check the actual bound
    // thread's persisted messages too; an unchanged empty list alone is weak.
    assert.deepEqual(
      await native("agent_chat_list_messages", { threadId }),
      messagesBefore,
      "Plugin operations must not persist or submit a prompt",
    );
    assert.equal(findPane(await native("get_app_state"))?.thread_id, threadId);
    assert.deepEqual(
      await native("agent_chat_list_sessions", { workspaceId }),
      sessionsBefore,
    );
  }
  await capture("04-core-chat-ready");
  await step("04-project-brief-native-git", async () => {
    await openCommand("Open Project Brief");
    await hasText("Branch: main");
    await hasText("1 untracked");
    await hasText("draft-context.txt");
  });
  await step("05-project-brief-real-draft", async () => {
    // WebDriver translates a newline to Enter; never send a submit key. The
    // plugin itself appends its multiline text through the real draft adapter.
    await typeComposer("Existing draft <literal> ");
    await clickText("Add to draft");
    await until("literal draft appended", () =>
      script(
        `return [...document.querySelectorAll('[data-testid="composer-body"] textarea')].some(e => e.value.startsWith('Existing draft <literal>') && e.value.includes('Project:') && e.value.includes('draft-context.txt'))`,
      ),
    );
  });
  await step(
    "06-issue-companion-native-https",
    async () => {
      await openCommand("Open Issue Companion");
      // Public, read-only HTTPS through the production DNS/TLS broker. No token,
      // intercepted fetch, or fixture-only origin exception. A rate limit fails
      // this gate visibly instead of treating an error state as a successful fetch.
      await element('section[aria-label="Add-on view"] select');
      await clickText(
        "Add to draft",
        `document.querySelector('section[aria-label="Add-on view"]')`,
      );
      await until("issue appended to actual draft", () =>
        script(
          `return [...document.querySelectorAll('[data-testid="composer-body"] textarea')].some(e => e.value.startsWith('Existing draft <literal>') && e.value.includes('Project:') && e.value.includes('https://github.com/octocat/Hello-World/issues/'))`,
        ),
      );
    },
    true,
  );
  await step("07-no-auto-submit", async () => {
    await assertNoSubmission();
    await openSettings();
    await hasText("Pause all add-ons");
    await clickText("Pause all add-ons");
    await until("paused", async () => (await native("addon_inventory")).paused);
    await until(
      "all native plugin processes reaped",
      async () => (await pluginHostCount()) === 0,
    );
    await clickText("Appearance");
    await hasText("Theme");
    await capture("07-paused-core-appearance");
    await click('[aria-label="Close settings"]');
    await checkCoreTerminal();
    await capture("07-paused-core-terminal");
    await openSettings();
    await clickText("Resume add-ons");
    await until(
      "resumed",
      async () => !(await native("addon_inventory")).paused,
    );
  });
  const faultInstallation = (await native("addon_inventory")).installed.find(
    (i) => i.manifest.id === "example.fault-isolation",
  );
  evidence.hostileWorkloads = [];
  // Saved installers include their exact fixture packages. Record every
  // available workload: retrying an older one-command fixture never establishes
  // the additional workloads in a newer package.
  for (const command of faultInstallation.manifest.contributes.commands) {
    await step(`08-hostile-${command.id}-core-interactivity`, async () => {
      const article = `([...document.querySelectorAll('article')].find(e => e.innerText.includes('Fault Isolation Fixture')))`;
      const current = (await native("addon_inventory")).installed.find(
        (i) => i.manifest.id === "example.fault-isolation",
      );
      if (current.status === "failed-disabled") {
        await clickText("Retry", article);
        await until(
          "explicit retry enabled",
          async () =>
            (await native("addon_inventory")).installed.find(
              (i) => i.manifest.id === "example.fault-isolation",
            ).desiredEnabled,
        );
      }
      await click('[aria-label="Close settings"]');
      await openCommand(command.title);
      const started = performance.now();
      const marker = ` Core input after ${command.id}.`;
      await typeComposer(marker);
      await until(
        "hostile runtime quarantined",
        async () =>
          (await native("addon_inventory")).installed.find(
            (i) => i.manifest.id === "example.fault-isolation",
          ).status === "failed-disabled",
        5000,
      );
      evidence.hostileWorkloads.push({
        command: command.id,
        uiObservationMs: performance.now() - started,
      });
      await until("core input accepted", () =>
        script(
          `return [...document.querySelectorAll('[data-testid="composer-body"] textarea')].some(e => e.value.includes(arguments[0]))`,
          marker,
        ),
      );
      await checkCoreTerminal();
      await openCommand("Open Project Brief");
      await hasText("Branch: main");
      await clickText(
        "Refresh",
        `document.querySelector('section[aria-label="Add-on view"]')`,
      );
      await hasText("draft-context.txt");
      await openSettings();
      await hasText("Fault Isolation Fixture");
      await hasText("failed disabled");
      await assertNoSubmission();
    });
  }
  await step(
    "09-paused-restart-preserves-installations-and-settings",
    async () => {
      const persisted = (inventory) =>
        inventory.installed
          .map((i) => ({
            installationId: i.installationId,
            id: i.manifest.id,
            digest: i.digest,
            dataGeneration: i.dataGeneration,
            source: i.source,
            grant: i.grant,
            desiredEnabled: i.desiredEnabled,
          }))
          .sort((a, b) => a.id.localeCompare(b.id));
      const before = persisted(await native("addon_inventory"));
      const settings = await native("addon_settings_get", {
        id: "codemux.issue-companion",
      });
      await clickText("Pause all add-ons");
      await until(
        "paused before restart",
        async () => (await native("addon_inventory")).paused,
      );
      await until(
        "hosts reaped before restart",
        async () => (await pluginHostCount()) === 0,
      );
      await restartNativeSession();
      await openSettings();
      const after = await native("addon_inventory");
      assert.equal(after.paused, true);
      assert.deepEqual(persisted(after), before);
      assert.deepEqual(
        await native("addon_settings_get", { id: "codemux.issue-companion" }),
        settings,
      );
      assert.equal(await pluginHostCount(), 0);
      await click('[aria-label="Close settings"]');
      await checkCoreTerminal();
      await openSettings();
      await clickText("Resume add-ons");
      await until(
        "resumed after restart",
        async () => !(await native("addon_inventory")).paused,
      );
      await click('[aria-label="Close settings"]');
      await openCommand("Open Project Brief");
      await hasText("draft-context.txt");
      await openSettings();
    },
  );
  await step("10-remove-packages-keeps-core-usable", async () => {
    for (const title of [
      "Issue Companion",
      "Project Brief",
      "Fault Isolation Fixture",
    ]) {
      const article = `([...document.querySelectorAll('article')].find(e => e.innerText.includes(${JSON.stringify(title)})))`;
      await clickText("Remove", article);
      await hasText(`Remove ${title}?`);
      await clickText(
        "Remove add-on",
        `document.querySelector('[role="dialog"]')`,
      );
      await until(
        `removed ${title}`,
        async () =>
          !(await native("addon_inventory")).installed.some(
            (i) => i.manifest.name === title,
          ),
      );
    }
    assert.equal(await pluginHostCount(), 0);
    await click('[aria-label="Close settings"]');
    await checkCoreTerminal();
  });
  if (evidence.failedChecks.length)
    throw Error("One or more native acceptance gates failed; see failedChecks");
  evidence.status = "passed";
} catch (error) {
  evidence.status = "failed";
  evidence.error = String(error);
  if (session) {
    // Installation metadata contains no credential values. Record the real
    // manager's state so a missing contribution can be distinguished from a
    // driver locator failure or a quarantined plugin.
    evidence.failureInventory = await native("addon_inventory").catch(
      () => null,
    );
  }
  if (session)
    await capture("failure").catch((error) => {
      evidence.captureError = String(error);
    });
  throw error;
} finally {
  if (process.platform === "win32" && desktop) {
    await run("powershell.exe", [
      "-NoProfile",
      "-File",
      "scripts/addons/windows-ui-diagnostics.ps1",
      "-DesktopPid",
      String(desktop.pid),
      "-EvidenceDirectory",
      evidenceDir,
    ]).catch((error) => {
      evidence.diagnosticsError = String(error);
    });
  }
  if (session) await wd("DELETE", "").catch(() => {});
  for (const child of owned) child.kill();
  if (windowsDebugPolicy) {
    await run("powershell.exe", [
      "-NoProfile",
      "-File",
      "scripts/addons/windows-webview-debug.ps1",
      "-Mode",
      "disable",
    ]).catch((error) => {
      evidence.policyCleanupError = String(error);
    });
  }
  if (driver) {
    const result = await Promise.race([
      driver.done.catch((error) => ({ code: null, output: String(error) })),
      delay(3000).then(() => null),
    ]);
    if (result) await writeFile(join(evidenceDir, "driver.log"), result.output);
  }
  if (desktop) {
    const result = await Promise.race([
      desktop.done.catch((error) => ({ output: String(error) })),
      delay(3000).then(() => null),
    ]);
    if (result)
      await writeFile(join(evidenceDir, "desktop.log"), result.output);
  }
  account.closeAllConnections();
  account.close();
  await writeFile(
    join(evidenceDir, "result.json"),
    JSON.stringify(evidence, null, 2) + "\n",
  );
  // Job VM disposal removes the synthetic profile; do not recursively delete
  // system app-data directories or terminate unrelated processes here.
}
