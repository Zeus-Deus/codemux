// CI-only WebDriver acceptance against an installed, unmodified release app.
// Never run on a developer profile. No embedded driver or production test hooks.
import assert from "node:assert/strict";
import { reversionFixture } from "./reversion-fixture.mjs";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { cpus, totalmem, release, homedir } from "node:os";
import { createServer } from "node:http";
import {
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  writeFile,
  rename,
} from "node:fs/promises";
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
let credentialProbe;
async function choosePackage(path) {
  await script(
    `const original = window.fetch;
    const path = arguments[0];
    const url = window.__TAURI_INTERNALS__.convertFileSrc('plugin:dialog|open', 'ipc');
    window.fetch = function(input, options) {
      if (input === url) {
        window.fetch = original;
        return Promise.resolve(new Response(JSON.stringify(path), {
          status: 200, headers: {'Content-Type': 'application/json', 'Tauri-Response': 'ok'}
        }));
      }
      return original.call(this, input, options);
    };`,
    path,
  );
}
async function checkNativeUpdateRollback() {
  const id = "codemux.project-brief";
  const article = `([...document.querySelectorAll('article')].find(e => e.innerText.includes('Project Brief')))`;
  const original = await readFile(
    resolve(
      "examples/addons/project-brief/codemux.project-brief-1.0.0.cmxaddon",
    ),
  );
  const path = join(root, "watched-brief.cmxaddon");
  await writeFile(path, original);
  await click('[aria-label="Developer mode"]');
  await choosePackage(path);
  await clickText("Select development package");
  await hasText("Review Project Brief");
  const replace = await script(
    `return [...document.querySelectorAll('[role="dialog"] label')].find(e => e.innerText.includes('Replace the existing source')).querySelector('input')`,
  );
  await wd("POST", `/element/${elementId(replace)}/click`, {});
  await clickText("Accept and install");
  await until(
    "selected development source",
    async () => (await native("addon_inventory")).developmentPackage === id,
  );
  await until("development review closed", () =>
    script(`return !document.querySelector('[role="dialog"]')`),
  );
  const installed = async () =>
    (await native("addon_inventory")).installed.find(
      (i) => i.manifest.id === id,
    );
  const before = await installed();
  await click('[aria-label="Close settings"]');
  await openCommand("Open Project Brief");
  await hasText("Branch: main");
  const checkbox = 'section[aria-label="Add-on view"] input[type="checkbox"]';
  await click(checkbox);
  await until("private preference off", () =>
    script(
      `return document.querySelector(arguments[0])?.checked === false`,
      checkbox,
    ),
  );
  // Remount to observe persisted private storage rather than just local UI state.
  await click('[aria-label="Close Project Brief"]');
  await openCommand("Open Project Brief");
  await until("private preference persisted", () =>
    script(
      `return document.querySelector(arguments[0])?.checked === false`,
      checkbox,
    ),
  );
  await openSettings();
  await writeFile(path + ".next", reversionFixture(original, "1.0.1"));
  await rename(path + ".next", path);
  const updated = await until("same-access native update", async () => {
    const value = await installed();
    return (
      value.manifest.version === "1.0.1" &&
      value.status === "enabled-running" &&
      value
    );
  });
  assert.equal(updated.installationId, before.installationId);
  assert.deepEqual(updated.source, before.source);
  assert.equal(updated.previous.digest, before.digest);
  assert.equal(updated.previous.dataGeneration, before.dataGeneration);
  assert.notEqual(updated.dataGeneration, before.dataGeneration);
  // Expanded access must show review and cancellation must leave the active
  // release, grant and private data generation untouched.
  await writeFile(
    path + ".next",
    reversionFixture(original, "1.0.2", "external.open"),
  );
  await rename(path + ".next", path);
  await hasText("Review Project Brief");
  await hasText("Open HTTPS links after your interaction");
  await wd("POST", "/actions", {
    actions: [
      {
        type: "key",
        id: "review-cancel",
        actions: [
          { type: "keyDown", value: "\ue00c" },
          { type: "keyUp", value: "\ue00c" },
        ],
      },
    ],
  });
  await until("expanded-access review cancelled", () =>
    script(`return !document.querySelector('[role="dialog"]')`),
  );
  const cancelled = await installed();
  assert.equal(cancelled.digest, updated.digest);
  assert.deepEqual(cancelled.grant, updated.grant);
  assert.equal(cancelled.dataGeneration, updated.dataGeneration);
  await click('[aria-label="Developer mode"]');
  await until(
    "watch disabled",
    async () => !(await native("addon_inventory")).developerMode,
  );
  await click('[aria-label="Close settings"]');
  await openCommand("Open Project Brief");
  await until("updated release has previous preference", () =>
    script(
      `return document.querySelector(arguments[0])?.checked === false`,
      checkbox,
    ),
  );
  await click(checkbox);
  await until("updated private preference on", () =>
    script(
      `return document.querySelector(arguments[0])?.checked === true`,
      checkbox,
    ),
  );
  await click('[aria-label="Close Project Brief"]');
  await openCommand("Open Project Brief");
  await until("updated preference persisted", () =>
    script(
      `return document.querySelector(arguments[0])?.checked === true`,
      checkbox,
    ),
  );
  await openSettings();
  await clickText("Rollback", article);
  await hasText("Restore 1.0.0?");
  await clickText("Restore previous release");
  const restored = await until("native rollback completed", async () => {
    const value = await installed();
    return (
      value.manifest.version === "1.0.0" &&
      value.digest === before.digest &&
      value
    );
  });
  assert.equal(restored.installationId, before.installationId);
  assert.notEqual(restored.dataGeneration, updated.dataGeneration);
  await until("rollback dialog closed", () =>
    script(`return !document.querySelector('[role="dialog"]')`),
  );
  await click('[aria-label="Close settings"]');
  await openCommand("Open Project Brief");
  await until("rollback restores matching private snapshot", () =>
    script(
      `return document.querySelector(arguments[0])?.checked === false`,
      checkbox,
    ),
  );
  await click(checkbox);
  await checkCoreTerminal();
  evidence.nativeUpdates = {
    sameAccess: true,
    activeHost: true,
    expandedAccessReview: true,
    cancellation: true,
    rollbackMatchingPrivateData: true,
    developerModeOff: true,
  };
  await openSettings();
}
async function corePaneDeck() {
  return script(
    `return [...document.querySelectorAll('[data-testid="right-panel-tabs-content"] button[aria-pressed]')].map(e => ({ title: e.title, active: e.getAttribute('aria-pressed') === 'true' })).filter(e => !['Project Brief', 'Issue Companion'].includes(e.title))`,
  );
}
async function checkEmptyAccessorySpace() {
  const layout =
    await script(`return [...document.querySelectorAll('[data-testid="composer-body"]')].map(body => {
    const footer = body.nextElementSibling;
    return { next: footer?.getAttribute('data-testid'), gap: footer?.getBoundingClientRect().top - body.getBoundingClientRect().bottom };
  });`);
  assert.ok(layout.length > 0);
  for (const composer of layout) {
    assert.equal(
      composer.next,
      "composer-controls-row",
      "No empty accessory wrapper may separate the draft and footer",
    );
    assert.ok(
      Math.abs(composer.gap) <= 1,
      "No accessory spacing may remain without an accessory",
    );
  }
  assert.equal(
    await script(
      `return document.querySelectorAll('[aria-label="Close add-on accessory"]').length`,
    ),
    0,
  );
}
let corePanesBeforePlugin;
async function checkCorePaneRestoration() {
  const panes = await corePaneDeck();
  assert.deepEqual(
    panes.map((p) => p.title),
    corePanesBeforePlugin,
  );
  assert.ok(
    panes.some((p) => p.active),
    "A core pane must become active when plugin panes are removed",
  );
  assert.equal(
    await script(
      `return document.querySelectorAll('[data-testid="addon-tab"]').length`,
    ),
    0,
  );
  await checkEmptyAccessorySpace();
}
async function checkCredentialSettings() {
  // Finish the real public-network test first, then close its view. This
  // synthetic credential must never be sent to GitHub or another service.
  await openCommand("Open Project Brief");
  await hasText("Branch: main");
  await openSettings();
  await clickText(
    "Configure / Permissions",
    `([...document.querySelectorAll('article')].find(e => e.innerText.includes('Issue Companion')))`,
  );
  const field = "#credential-github-token";
  const secret = `ci-synthetic-only-${createHash("sha256").update(root).digest("hex")}`;
  await type(field, secret);
  assert.equal(
    await script("return document.querySelector(arguments[0]).type", field),
    "password",
  );
  const sessionOption = await script(
    `return [...document.querySelectorAll('label')].find(e => e.innerText.includes('Store new values for this session only')).querySelector('input')`,
  );
  assert.equal(
    await script("return arguments[0].checked", sessionOption),
    false,
  );
  await clickText("Save");
  if (process.platform === "linux") {
    // The fresh runner's dbus session has no Secret Service. Confirm that the
    // app does not silently fall back, then explicitly select its session mode.
    await hasText("Credential store unavailable or locked");
    assert.equal(
      await script("return arguments[0].checked", sessionOption),
      false,
    );
    assert.equal(
      await script(
        "return document.querySelector(arguments[0]).value.length",
        field,
      ),
      secret.length,
    );
    await wd("POST", `/element/${elementId(sessionOption)}/click`, {});
    await clickText("Save");
  }
  await until("credential field cleared after successful save", () =>
    script(
      "return document.querySelector(arguments[0]).value.length === 0",
      field,
    ),
  );
  await step(
    "07-credential-success-clears-error",
    async () => {
      assert.ok(
        !(await text()).includes("Credential store unavailable or locked"),
        "A successful explicit fallback must clear its stale failure",
      );
    },
    true,
  );
  const inventory = await native("addon_inventory");
  const installationId = inventory.installed.find(
    (i) => i.manifest.id === "codemux.issue-companion",
  ).installationId;
  assert.ok(!JSON.stringify(inventory).includes(secret));
  assert.ok(
    !JSON.stringify(
      await native("addon_settings_get", { id: "codemux.issue-companion" }),
    ).includes(secret),
  );
  if (process.platform === "win32") {
    assert.ok(
      (await run("cmdkey.exe", ["/list"])).includes(installationId),
      "Synthetic token must use the real Windows credential backend",
    );
  }
  credentialProbe = { secret, installationId };
  evidence.credentials = {
    maskedInput: true,
    noSecretInInventoryOrSettings: true,
    mode:
      process.platform === "win32"
        ? "native Windows credential store"
        : "missing Secret Service, explicit session-only fallback",
  };
  await clickText("Installed");
  await click('[aria-label="Close settings"]');
}
async function checkCredentialRemovalAndRedaction() {
  assert.ok(credentialProbe);
  if (process.platform === "win32")
    assert.ok(
      !(await run("cmdkey.exe", ["/list"])).includes(
        credentialProbe.installationId,
      ),
      "Uninstall must delete the native Windows credential",
    );
  const dataHome =
    process.platform === "win32"
      ? env.APPDATA
      : env.XDG_DATA_HOME || join(homedir(), ".local/share");
  const paths = await files(join(dataHome, "codemux", "addons-v1"));
  for (const path of [...paths, ...(await files(evidenceDir))])
    assert.ok(
      !(await readFile(path)).includes(Buffer.from(credentialProbe.secret)),
      "A credential leaked into private plugin files or UI evidence",
    );
  evidence.credentials.removalAndFileRedaction = true;
}
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
async function checkContextRaces(originalWorkspace, assertNoSubmission) {
  // Built by build-examples.sh with the packed public SDK/CLI. Outcomes are read
  // from the fixture's own panel: the notification limit (three per minute)
  // would otherwise hide them after the first case.
  const path = resolve(
    "scripts/addons/fixtures/context-races/example.context-races-1.0.0.cmxaddon",
  );
  const bytes = await readFile(path);
  evidence.seams.push(
    "CI-only delayed public-SDK package built with the packed author tools; normal native package import, SDK broker and plugin-rendered status panel",
  );
  evidence.contextRaces = {
    fixtureSha256: createHash("sha256").update(bytes).digest("hex"),
    delayMs: 4000,
    completed: [],
    rejections: {},
  };
  await openSettings();
  await choosePackage(path);
  await clickText("Import package");
  await hasText("Review Context Race Fixture");
  await clickText("Accept and install");
  await until("context fixture installed", async () =>
    (await native("addon_inventory")).installed.some(
      (i) => i.manifest.id === "example.context-races" && i.desiredEnabled,
    ),
  );
  await until("context review closed", () =>
    script(`return !document.querySelector('[role="dialog"]')`),
  );
  await click('[aria-label="Close settings"]');
  const cwd = join(root, "context-race-project");
  await mkdir(cwd);
  const workspaceId = await native("create_workspace", { cwd });
  const createPane = () =>
    native("agent_chat_create_pane", {
      workspaceId,
      cwd,
      provider: null,
      launchMode: null,
      threadId: null,
    });
  const selectWorkspace = async (id, name) => {
    await shortcut("k");
    await type('[role="combobox"]', name);
    await click(`[role="option"][data-value="ws:${id}"]`);
  };
  const findPane = (value, id) => {
    if (!value || typeof value !== "object") return null;
    if (value.pane_id === id) return value;
    return (
      Object.values(value)
        .map((v) => findPane(v, id))
        .find(Boolean) ?? null
    );
  };
  const fixtureThreads = new Map();
  const warmPane = async (id) => {
    await element(composer);
    await click(composer);
    const threadId = await until(
      "race pane bound",
      async () => findPane(await native("get_app_state"), id)?.thread_id,
    );
    await until("race pane provider settled", async () => {
      const content = await text();
      return (
        content.includes("Session error") ||
        content.includes(
          "Claude Code CLI (`claude`) is not installed or not on PATH.",
        )
      );
    });
    fixtureThreads.set(
      threadId,
      await native("agent_chat_list_messages", { threadId }),
    );
  };
  const drafts = () =>
    script(
      `return [...document.querySelectorAll(arguments[0])].map(e => e.value)`,
      composer,
    );
  const pending = async (id) => {
    await openCommand(`CI delayed ${id}`);
    await hasText(`CI pending ${id}`);
  };
  // The context change itself must reject the late append, well before the
  // 10 s interaction expires; an expiry would not prove cancellation.
  const cancelled = async (id, codes) => {
    const pattern = new RegExp(`CI cancelled ${id}: ([A-Z_]+)`);
    const code = await until(
      `CI cancelled ${id}`,
      async () => (await text()).match(pattern)?.[1],
    );
    evidence.contextRaces.rejections[id] = code;
    assert.ok(codes.includes(code), `${id} was rejected with ${code}`);
    assert.ok(
      (await drafts()).every((value) => !value.includes(`CI delayed ${id}.`)),
    );
    evidence.contextRaces.completed.push(id);
  };
  // The panel belongs to each workspace's deck; outcomes are the plugin's own
  // module state, so both decks show every outcome of the running generation.
  const openStatus = async () => {
    await openCommand("Open context race status");
    await hasText("CI status ready");
  };
  let pane = await createPane();
  await selectWorkspace(workspaceId, "context-race-project");
  await warmPane(pane);
  await openStatus();
  await selectWorkspace(originalWorkspace, "synthetic-project");
  await openStatus();
  await step("10-context-typing-preserved", async () => {
    await pending("typing");
    await typeComposer(" User race input.");
    await hasText("CI appended typing");
    assert.ok(
      (await drafts()).some(
        (value) =>
          value.includes("User race input.") &&
          value.includes("CI delayed typing."),
      ),
    );
    await assertNoSubmission();
    evidence.contextRaces.completed.push("typing");
  });
  await step("10-context-project-switch-cancels", async () => {
    await pending("workspace");
    await selectWorkspace(workspaceId, "context-race-project");
    await cancelled("workspace", ["CONTEXT_STALE"]);
    await selectWorkspace(originalWorkspace, "synthetic-project");
    assert.ok(
      (await drafts()).every(
        (value) => !value.includes("CI delayed workspace."),
      ),
    );
    await selectWorkspace(workspaceId, "context-race-project");
  });
  await step("10-context-thread-close-cancels", async () => {
    await pending("thread");
    await native("agent_chat_close_pane", { paneId: pane, select: true });
    await cancelled("thread", ["CONTEXT_STALE", "NO_COMPOSER"]);
  });
  pane = await createPane();
  await selectWorkspace(workspaceId, "context-race-project");
  await warmPane(pane);
  await step("10-context-composer-replacement-cancels", async () => {
    await pending("replacement");
    await native("agent_chat_close_pane", { paneId: pane, select: true });
    pane = await createPane();
    await selectWorkspace(workspaceId, "context-race-project");
    await cancelled("replacement", ["CONTEXT_STALE", "NO_COMPOSER"]);
    await warmPane(pane);
  });
  await step("10-context-disable-cancels", async () => {
    await pending("disable");
    await openSettings();
    const article = `([...document.querySelectorAll('article')].find(e => e.innerText.includes('Context Race Fixture')))`;
    await clickText("Disable", article);
    await until(
      "context fixture disabled",
      async () =>
        !(await native("addon_inventory")).installed.find(
          (i) => i.manifest.id === "example.context-races",
        ).desiredEnabled,
    );
    await delay(4500);
    assert.equal(await pluginHostCount(), 0);
    await click('[aria-label="Close settings"]');
    assert.ok(
      (await drafts()).every((value) => !value.includes("CI delayed disable.")),
    );
    evidence.contextRaces.completed.push("disable");
    await openSettings();
    await clickText("Remove", article);
    await hasText("Remove Context Race Fixture?");
    await clickText(
      "Remove add-on",
      `document.querySelector('[role="dialog"]')`,
    );
    await until(
      "context fixture removed",
      async () => (await native("addon_inventory")).installed.length === 0,
    );
    await until("context removal dialog closed", () =>
      script(`return !document.querySelector('[role="dialog"]')`),
    );
    await click('[aria-label="Close settings"]');
  });
  for (const [threadId, messages] of fixtureThreads)
    assert.deepEqual(
      await native("agent_chat_list_messages", { threadId }),
      messages,
      "Race tests must never submit prompts",
    );
  await selectWorkspace(originalWorkspace, "synthetic-project");
  await assertNoSubmission();
  await checkCoreTerminal();
}
async function checkRemovalDuringActivation() {
  const id = "example.activation-race";
  const path = resolve(
    "scripts/addons/fixtures/activation-race/example.activation-race-1.0.0.cmxaddon",
  );
  const bytes = await readFile(path);
  await openSettings();
  await choosePackage(path);
  await clickText("Import package");
  await hasText("Review Activation Race Fixture");
  await clickText("Accept and install");
  await until("activation fixture installed", async () =>
    (await native("addon_inventory")).installed.some(
      (i) => i.manifest.id === id && i.desiredEnabled,
    ),
  );
  await until("activation review closed", () =>
    script(`return !document.querySelector('[role="dialog"]')`),
  );
  await click('[aria-label="Close settings"]');
  // Installation ran its probe; make the palette command start a fresh host.
  await native("addon_disable", { id });
  await native("addon_enable", { id });
  await until("no host before the command", async () => (await pluginHostCount()) === 0);
  await openCommand("CI slow activation append");
  const started = Date.now();
  // Removal is requested while the fixture's activation is still waiting.
  await native("addon_remove", { id, keepData: false });
  const removalMs = Date.now() - started;
  await until("activation fixture removed", async () =>
    (await native("addon_inventory")).installed.every((i) => i.manifest.id !== id),
  );
  await until("activation host reaped", async () => (await pluginHostCount()) === 0, 2000);
  await delay(1500);
  assert.ok(
    (
      await script(
        `return [...document.querySelectorAll(arguments[0])].map(e => e.value)`,
        composer,
      )
    ).every((value) => !value.includes("CI activation race.")),
    "Removal during activation must not let the command append",
  );
  assert.equal(await pluginHostCount(), 0);
  evidence.removalDuringActivation = {
    fixtureSha256: createHash("sha256").update(bytes).digest("hex"),
    activationDelayMs: 700,
    removalCompletedMs: removalMs,
  };
  await checkCoreTerminal();
}
async function openCommand(title) {
  await shortcut("k");
  await type('[role="combobox"]', title);
  // Inventory refresh after rollback can replace a palette option between
  // locating it and the WebDriver click. Retry only a stale-element rejection
  // (the driver did not dispatch that click), never a possibly completed action.
  for (let attempt = 0; attempt < 3; attempt++) {
    const el = await until(title, () =>
      script(
        `return [...document.querySelectorAll('[role="option"]')].find(e => e.innerText.includes(arguments[0])) ?? null`,
        title,
      ),
    );
    try {
      await wd("POST", `/element/${elementId(el)}/click`, {});
      return;
    } catch (error) {
      if (
        attempt === 2 ||
        !String(error).includes('"error":"stale element reference"')
      )
        throw error;
    }
  }
}
async function checkOfficialUpdater(phase) {
  // Read-only check through the stock app's official updater. Never download,
  // install or relaunch another version underneath this acceptance run.
  const update = await native("plugin:updater|check", { timeout: 15000 });
  if (update) {
    assert.equal(typeof update.version, "string");
    await native("plugin:resources|close", { rid: update.rid });
  }
  (evidence.officialUpdater ??= []).push({
    phase,
    available: !!update,
    version: update?.version ?? null,
  });
}
async function checkRemoteBoundary() {
  assert.equal((await native("web_remote_status")).enabled, false);
  const reservation = createServer();
  await new Promise((done) => reservation.listen(0, "127.0.0.1", done));
  const port = reservation.address().port;
  await new Promise((done) => reservation.close(done));
  let socket, paired;
  try {
    await native("web_remote_set_config", {
      port,
      bindScope: "loopback",
      requireApproval: true,
      accountModeEnabled: false,
      trustAccountBrowsers: false,
      relayModeEnabled: false,
    });
    const status = await native("web_remote_enable");
    assert.equal(status.bind_scope, "loopback");
    const base = `http://127.0.0.1:${port}`;
    const pairing = await native("web_remote_create_pairing");
    const pairResponse = await fetch(`${base}/api/pair`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Origin: base },
      body: JSON.stringify({
        token: pairing.token,
        device_name: "Disposable add-on boundary test",
      }),
      signal: AbortSignal.timeout(10000),
    });
    assert.equal(pairResponse.status, 200);
    paired = await pairResponse.json();
    assert.equal(paired.approved, false);
    await native("web_remote_approve_session", {
      sessionId: paired.session_id,
    });
    const ticketResponse = await fetch(`${base}/api/ws-ticket`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${paired.session_token}`,
        Origin: base,
      },
      signal: AbortSignal.timeout(10000),
    });
    assert.equal(ticketResponse.status, 200);
    const { ticket } = await ticketResponse.json();
    socket = new WebSocket(`ws://127.0.0.1:${port}/ws?ticket=${ticket}`);
    const frames = [];
    socket.addEventListener("message", (event) =>
      frames.push(JSON.parse(event.data)),
    );
    await until(
      "paired remote WebSocket ready",
      () => socket.readyState === WebSocket.OPEN,
    );
    let sequence = 0;
    async function invoke(cmd, args = {}) {
      const id = ++sequence;
      socket.send(JSON.stringify({ t: "invoke", id, cmd, args }));
      return until(`remote ${cmd}`, () =>
        frames.find((frame) => frame.id === id),
      );
    }
    assert.equal(
      (await invoke("get_app_state")).t,
      "ok",
      "Core RPC must still work",
    );
    assert.ok(
      (await pluginHostCount()) > 0,
      "Test the boundary with actual active plugins",
    );
    for (const cmd of [
      "addon_inventory",
      "addon_subscribe",
      "addon_execute",
      "addon_future_command",
    ]) {
      const response = await invoke(cmd, {
        id: "codemux.project-brief",
        onEvent: "__CHANNEL__:999",
      });
      assert.equal(response.t, "err");
      assert.match(String(response.error), /REMOTE_UNSUPPORTED/);
    }
    for (const event of [
      "addon:ci-boundary",
      "addon_ci_boundary",
      "ci-core-boundary",
    ])
      socket.send(JSON.stringify({ t: "listen", event }));
    // A core RPC response is a barrier after the synchronous subscriptions.
    assert.equal((await invoke("get_app_state")).t, "ok");
    for (const event of [
      "addon:ci-boundary",
      "addon_ci_boundary",
      "ci-core-boundary",
    ])
      await native("plugin:event|emit", {
        event,
        payload: { synthetic: true },
      });
    await until("core event forwarded", () =>
      frames.some(
        (frame) => frame.t === "event" && frame.event === "ci-core-boundary",
      ),
    );
    assert.ok(
      !frames.some(
        (frame) => frame.t === "event" && frame.event.startsWith("addon"),
      ),
      "Plugin events must not cross the remote boundary",
    );
    evidence.remoteBoundary = {
      transport: "paired loopback HTTP/WebSocket against stock desktop",
      approval: "explicit desktop command",
      coreRpc: true,
      addonRpcDenied: true,
      coreEvents: true,
      addonEventsDenied: true,
    };
  } finally {
    socket?.close();
    if (paired)
      await native("web_remote_revoke_session", {
        sessionId: paired.session_id,
      });
    await native("web_remote_disable");
  }
  assert.equal((await native("web_remote_status")).enabled, false);
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
async function stopNativeSession() {
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
  }
}
async function startNativeSession() {
  if (process.platform === "win32") {
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
async function restartNativeSession() {
  await stopNativeSession();
  await startNativeSession();
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
        // Verbose WebDriver logs record password SendKeys payloads. Preserve
        // warnings without recording the synthetic secret in command traces.
        "--log-level=WARNING",
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
  await step(
    "01-official-updater-without-plugins",
    () => checkOfficialUpdater("clean"),
    true,
  );
  await step("01-keyboard-dialog-focus-and-core-themes", async () => {
    await clickText("Install from link / ID");
    const input = await element('[aria-label="Add-on install link or ID"]');
    await until("install dialog autofocus", () =>
      script("return document.activeElement === arguments[0]", input),
    );
    await wd("POST", "/actions", {
      actions: [
        {
          type: "key",
          id: "keyboard",
          actions: [
            { type: "keyDown", value: "\uE00C" },
            { type: "keyUp", value: "\uE00C" },
          ],
        },
      ],
    });
    await until("install dialog dismissed", () =>
      script("return !document.querySelector('[role=dialog]')"),
    );
    assert.equal(
      await script("return document.activeElement?.innerText.trim()"),
      "Install from link / ID",
    );
    await hasText("Import package");
    for (const [theme, scheme] of [
      ["Graphite Light", "light"],
      ["Ember", "dark"],
    ]) {
      await openCommand(theme);
      await until(`${scheme} theme applied`, () =>
        script(
          "return document.documentElement.style.colorScheme === arguments[0]",
          scheme,
        ),
      );
      await hasText("Import package");
      await capture(`01-addons-settings-${scheme}`);
    }
  });
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
  // The existing composer may show either the notice label or the provider's
  // direct availability detail. Both represent the same settled, unbound-CLI
  // state; neither starts inference or submits the synthetic draft.
  await until("provider unavailable in the disposable runner", async () => {
    const content = await text();
    return (
      content.includes("Session error") ||
      content.includes(
        "Claude Code CLI (`claude`) is not installed or not on PATH.",
      )
    );
  });
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
    corePanesBeforePlugin = (await corePaneDeck()).map((p) => p.title);
    assert.ok(corePanesBeforePlugin.length > 0);
    await checkEmptyAccessorySpace();
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
  await step(
    "06-host-credential-settings-and-explicit-fallback",
    checkCredentialSettings,
  );
  await step(
    "07-paired-remote-cannot-invoke-or-subscribe-to-plugins",
    checkRemoteBoundary,
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
    await step(
      "07-official-updater-while-plugins-paused",
      () => checkOfficialUpdater("paused"),
      true,
    );
    await clickText("Appearance");
    await hasText("Theme");
    await capture("07-paused-core-appearance");
    await click('[aria-label="Close settings"]');
    await checkCorePaneRestoration();
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
  await step("08-bounded-list-rendering-and-frame-budget", async () => {
    // Git's normal untracked mode collapses directories. Use root files so
    // this workload actually exercises 500 separate model rows.
    for (let i = 0; i < 500; i++)
      await writeFile(
        join(project, `ui-load-${String(i).padStart(3, "0")}.txt`),
        "Synthetic UI workload\n",
      );
    await click('[aria-label="Close settings"]');
    await openCommand("Open Project Brief");
    // The public Git API deliberately caches summaries for one second.
    await delay(1100);
    const view = `document.querySelector('section[aria-label="Add-on view"]')`;
    await clickText("Refresh", view);
    await hasText("501 untracked");
    await hasText("Showing the first 500 changed paths.");
    const rows = await script(
      `return ${view}.querySelectorAll('[role="listitem"]').length`,
    );
    assert.ok(
      rows > 0 && rows <= 14,
      "The trusted list must virtualize 500 paths",
    );
    await script(`window.__addonFrameProbe = {gaps: [], active: true, last: performance.now()};
      const probe = window.__addonFrameProbe;
      const tick = now => {
        if (!probe.active || probe.gaps.length >= 3000) return;
        probe.gaps.push(now - probe.last); probe.last = now; requestAnimationFrame(tick);
      }; requestAnimationFrame(tick);`);
    const refreshMs = [];
    for (let i = 0; i < 5; i++) {
      const started = performance.now();
      await clickText("Refresh", view);
      await hasText("501 untracked");
      refreshMs.push(performance.now() - started);
      await typeComposer(` Render probe ${i}.`);
      await delay(300);
    }
    const gaps = await script(
      `const p = window.__addonFrameProbe; p.active = false; delete window.__addonFrameProbe; return p.gaps;`,
    );
    assert.ok(
      gaps.length >= 30,
      "A visible native WebView must supply frame samples",
    );
    gaps.sort((a, b) => a - b);
    const p95FrameGapMs = gaps[Math.floor((gaps.length - 1) * 0.95)];
    evidence.uiRendering = {
      workload: "500 visible-model paths, five real Git refreshes while typing",
      renderedRows: rows,
      frameSamples: gaps.length,
      p95FrameGapMs,
      maxFrameGapMs: gaps.at(-1),
      refreshMs,
      p95FrameGapBudgetMs: 100,
    };
    assert.ok(
      p95FrameGapMs <= 100,
      "Shared-runner UI frame p95 exceeds the recorded 100 ms budget",
    );
    await step(
      "08-virtual-list-native-accessibility",
      async () => {
        const list = 'section[aria-label="Add-on view"] [role="list"]';
        const positions = () =>
          script(
            `return [...document.querySelector(arguments[0]).querySelectorAll('[role="listitem"]')].map(e => ({size:Number(e.getAttribute('aria-setsize')),position:Number(e.getAttribute('aria-posinset'))}))`,
            list,
          );
        assert.ok(
          (await positions()).every(
            (row) => row.size === 500 && row.position >= 1,
          ),
        );
        assert.equal(
          await script(
            `return document.querySelector(arguments[0]).getAttribute('aria-label')`,
            list,
          ),
          "Add-on list",
        );
        await click(list);
        await wd("DELETE", "/actions");
        await wd("POST", "/actions", {
          actions: [
            {
              type: "key",
              id: "list-keyboard",
              actions: [
                { type: "keyDown", value: "\ue010" },
                { type: "keyUp", value: "\ue010" },
              ],
            },
          ],
        });
        await until("keyboard reaches the final virtual row", async () =>
          (await positions()).some((row) => row.position === 500),
        );
        assert.ok((await positions()).length <= 14);
        evidence.virtualListAccessibility = {
          label: "Add-on list",
          setSize: 500,
          keyboardEndPosition: 500,
          boundedRows: true,
        };
      },
      true,
    );
    await assertNoSubmission();
    await capture("08-virtualized-native-plugin-list");
    await openSettings();
  });
  await step(
    "09-native-update-review-and-matching-data-rollback",
    checkNativeUpdateRollback,
  );
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
  await step(
    "09-classic-interface-keeps-plugin-panels-and-core-terminal",
    async () => {
      // Use the same persisted native setting as Settings → Interface, then
      // restart the owned app. No client store or plugin capability is overridden.
      await native("set_agent_chat_enabled", { enabled: false });
      await restartNativeSession();
      assert.equal(
        (await native("get_feature_flags")).enable_agent_chat,
        false,
      );
      await openSettings();
      await hasText("Project Brief");
      await click('[aria-label="Close settings"]');
      await checkCoreTerminal();
      assert.equal(
        await script(
          `return document.querySelectorAll(${JSON.stringify(composer)}).length`,
        ),
        0,
      );
      assert.equal(
        await script(
          `return document.querySelectorAll('[aria-label="Close add-on accessory"]').length`,
        ),
        0,
      );
      await openCommand("Open Project Brief");
      await hasText("Branch: main");
      await clickText(
        "Add to draft",
        `document.querySelector('section[aria-label="Add-on view"]')`,
      );
      await hasText("No chat composer is available");
      await capture("09-classic-interface-panel-without-composer");
      await native("set_agent_chat_enabled", { enabled: true });
      await restartNativeSession();
      await element(composer);
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
      // The native transaction completes before the Settings refresh and
      // dialog exit animation. Wait for the actual UI before the next click.
      await until(`removal dialog and ${title} card closed`, () =>
        script(
          `return !document.querySelector('[role="dialog"]') && ![...document.querySelectorAll('article')].some(e => e.innerText.includes(arguments[0]))`,
          title,
        ),
      );
    }
    assert.equal(await pluginHostCount(), 0);
    await click('[aria-label="Close settings"]');
    await checkCoreTerminal();
    await checkCredentialRemovalAndRedaction();
    await checkCorePaneRestoration();
    evidence.corePaneRestoration = {
      preserved: corePanesBeforePlugin,
      paused: true,
      removed: true,
      noEmptyAccessorySpace: true,
    };
  });
  await step("10-delayed-public-sdk-context-races", () =>
    checkContextRaces(workspaceId, assertNoSubmission),
  );
  await step(
    "10-remove-during-activation-leaves-no-host-or-draft",
    checkRemovalDuringActivation,
  );
  await step(
    "11-corrupt-plugin-registry-does-not-block-core-startup",
    async () => {
      assert.deepEqual((await native("addon_inventory")).installed, []);
      await stopNativeSession();
      // This exact registry was created by the imports above in a fresh disposable
      // runner. Preserve it and its SQLite companions; never touch core databases.
      const dataHome =
        process.platform === "win32"
          ? env.APPDATA
          : env.XDG_DATA_HOME || join(homedir(), ".local/share");
      assert.ok(dataHome);
      const registry = join(
        dataHome,
        "codemux",
        "addons-v1",
        "registry.sqlite",
      );
      assert.equal(
        (await readFile(registry)).subarray(0, 16).toString(),
        "SQLite format 3\0",
      );
      for (const suffix of ["", "-wal", "-shm"]) {
        try {
          await rename(registry + suffix, registry + suffix + ".ci-backup");
        } catch (error) {
          if (suffix === "" || error.code !== "ENOENT") throw error;
        }
      }
      await writeFile(registry, "Synthetic invalid plugin registry", {
        flag: "wx",
      });
      await startNativeSession();
      await openSettings();
      const unavailable = await native("addon_inventory");
      assert.equal(unavailable.paused, true);
      assert.ok(unavailable.error);
      assert.equal(await pluginHostCount(), 0);
      await clickText("Appearance");
      await hasText("Theme");
      await click('[aria-label="Close settings"]');
      await checkCoreTerminal();
      await typeComposer(" Core input while plugin storage is unavailable.");
    },
  );
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
