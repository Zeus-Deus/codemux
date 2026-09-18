// CI-only WebDriver acceptance against an installed, unmodified release app.
// Never run on a developer profile. No embedded driver or production test hooks.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
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
  checks: [],
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
const elementId = (el) => el["element-6066-11e4-a52e-4f735466cecf"];
async function clickText(value, scope = "document") {
  const el = await until(`click ${value}`, () =>
    script(
      `return [...${scope}.querySelectorAll('button,[role="menuitem"],[role="option"]')].find(e => e.getClientRects().length && (e.innerText.trim() === arguments[0] || e.getAttribute('aria-label') === arguments[0])) ?? null`,
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
  await wd("POST", `/element/${elementId(el)}/value`, { text: value });
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
  await writeFile(
    join(evidenceDir, `${name}.png`),
    Buffer.from(await wd("GET", "/screenshot"), "base64"),
  );
}
async function step(name, fn) {
  console.log(`Native UI: ${name}`);
  await fn();
  evidence.checks.push(name);
  await capture(name);
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
try {
  await run(application, ["login", "--token", token], { env });
  driver = start("tauri-driver", ["--port", "4444"], {
    env,
  });
  await until(
    "driver startup",
    async () => (await request("GET", "/status"))?.ready === true,
  );
  const created = await request("POST", "/session", {
    capabilities: { alwaysMatch: { "tauri:options": { application } } },
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
  await step("01-settings", openSettings);
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
      // One-shot, chooser-only seam. Restore before the real review command.
      await script(
        `const original = window.__TAURI_INTERNALS__.invoke; const path = arguments[0]; window.__TAURI_INTERNALS__.invoke = function(command, args, options) { if (command === 'plugin:dialog|open') { window.__TAURI_INTERNALS__.invoke = original; return Promise.resolve(path); } return original(command, args, options); };`,
        path,
      );
      await clickText("Import package");
      await hasText(`Review ${title}`);
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
    const article = `([...document.querySelectorAll('article')].find(e => e.innerText.includes('Project Brief')))`;
    await clickText("Disable", article);
    await until(
      "disabled",
      async () =>
        !(await native("addon_inventory")).installed.find(
          (i) => i.manifest.id === "codemux.project-brief",
        ).desiredEnabled,
    );
    await clickText("Enable", article);
    await until(
      "enabled",
      async () =>
        (await native("addon_inventory")).installed.find(
          (i) => i.manifest.id === "codemux.project-brief",
        ).desiredEnabled,
    );
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
  // Workspace creation normally writes its own MCP discovery file. Keep that
  // normal behavior while making the deliberately untracked path unambiguous.
  await writeFile(join(project, ".git", "info", "exclude"), ".mcp.json\n");
  await run("git", ["-C", project, "add", "README.md"]);
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
  await native("agent_chat_create_pane", {
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
  await element("textarea");
  const sessionsBefore = await native("agent_chat_list_sessions", {
    workspaceId,
  });
  await step("04-project-brief-native-git", async () => {
    await openCommand("Open Project Brief");
    await hasText("Branch: main");
    await hasText("1 untracked");
    await hasText("draft-context.txt");
  });
  await step("05-project-brief-real-draft", async () => {
    // WebDriver translates a newline to Enter; never send a submit key. The
    // plugin itself appends its multiline text through the real draft adapter.
    await type("textarea", "Existing draft <literal> ");
    await clickText("Add to draft");
    await until("literal draft appended", () =>
      script(
        `return [...document.querySelectorAll('textarea')].some(e => e.value.startsWith('Existing draft <literal>') && e.value.includes('Project:') && e.value.includes('draft-context.txt'))`,
      ),
    );
  });
  await step("06-issue-companion-native-https", async () => {
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
        `return [...document.querySelectorAll('textarea')].some(e => e.value.startsWith('Existing draft <literal>') && e.value.includes('Project:') && e.value.includes('https://github.com/octocat/Hello-World/issues/'))`,
      ),
    );
  });
  await step("07-no-auto-submit", async () => {
    assert.deepEqual(
      await native("agent_chat_list_sessions", { workspaceId }),
      sessionsBefore,
      "Plugin draft insertion must not start or submit a session",
    );
    await openSettings();
    await hasText("Pause all add-ons");
    await clickText("Pause all add-ons");
    await until("paused", async () => (await native("addon_inventory")).paused);
    await clickText("Resume add-ons");
    await until(
      "resumed",
      async () => !(await native("addon_inventory")).paused,
    );
  });
  await step("08-hostile-plugin-core-interactivity", async () => {
    await click('[aria-label="Close settings"]');
    await openCommand("Run isolated blocking fixture");
    const started = performance.now();
    await type("textarea", " Core remains interactive.");
    await until(
      "hostile runtime quarantined",
      async () =>
        (await native("addon_inventory")).installed.find(
          (i) => i.manifest.id === "example.fault-isolation",
        ).status === "failed-disabled",
      5000,
    );
    evidence.hostileUiObservationMs = performance.now() - started;
    await until("core input accepted", () =>
      script(
        `return [...document.querySelectorAll('textarea')].some(e => e.value.includes('Core remains interactive.'))`,
      ),
    );
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
    assert.deepEqual(
      await native("agent_chat_list_sessions", { workspaceId }),
      sessionsBefore,
    );
  });
  evidence.status = "passed";
} catch (error) {
  evidence.status = "failed";
  evidence.error = String(error);
  if (session) await capture("failure").catch(() => {});
  throw error;
} finally {
  if (session) await wd("DELETE", "").catch(() => {});
  for (const child of owned) child.kill();
  if (driver) {
    const result = await Promise.race([
      driver.done.catch((error) => ({ code: null, output: String(error) })),
      delay(3000).then(() => null),
    ]);
    if (result) await writeFile(join(evidenceDir, "driver.log"), result.output);
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
