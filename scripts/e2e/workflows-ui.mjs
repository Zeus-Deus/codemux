import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import path from 'node:path';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';

const puppeteerModule = process.env.CODEMUX_PUPPETEER_MODULE || pathToFileURL(path.join(execFileSync('npm', ['root', '-g'], { encoding: 'utf8' }).trim(), 'puppeteer/node_modules/puppeteer-core/lib/puppeteer/puppeteer-core.js')).href;
const { default: puppeteer } = await import(puppeteerModule);

const evidence = process.env.CODEMUX_WORKFLOW_EVIDENCE
  ? pathToFileURL(path.resolve(process.env.CODEMUX_WORKFLOW_EVIDENCE) + path.sep)
  : new URL('../../docs/features/assets/workflow-ui-integrated/', import.meta.url);
await fs.mkdir(evidence, { recursive: true });
const browser = await puppeteer.launch({ executablePath: process.env.CODEMUX_CHROMIUM || '/usr/bin/chromium', headless: true, args: ['--no-sandbox', '--disable-dev-shm-usage'] });
let qaPage;
try {
  const page = await browser.newPage();
  qaPage = page;
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  await page.emulateMediaFeatures([{ name: 'prefers-reduced-motion', value: 'reduce' }]);
  const pageErrors = [];
  page.on('pageerror', error => pageErrors.push(String(error)));
  await page.setRequestInterception(true);
  page.on('request', request => {
    const url = new URL(request.url());
    if (['http:', 'https:'].includes(url.protocol) && url.hostname !== '127.0.0.1' && url.hostname !== 'localhost') request.abort();
    else request.continue();
  });
  const appUrl = process.env.CODEMUX_WORKFLOW_URL || 'http://127.0.0.1:1420/';
  await page.goto(`${appUrl}${appUrl.includes('?') ? '&' : '?'}workflowDelay=3000`, { waitUntil: 'networkidle0' });
  await page.evaluate(() => document.fonts.ready);
  if (process.argv.includes('--before')) {
    await page.locator('[data-testid="composer-workflows-toggle"]').click();
    await page.waitForSelector('[data-testid="workflow-panel"]');
    await page.screenshot({ path: new URL('before.png', evidence).pathname });
  } else {
    const entry = await page.waitForSelector('[data-testid="composer-workflows-toggle"]');
    assert.ok(entry);
    await entry.click();
    await page.waitForSelector('[data-testid="workflow-panel"]');
    const reveal = async selector => {
      if (selector.includes('workflow-task-') && !await page.$(selector)) {
        const settled = await page.$('[data-testid="workflow-settled"]');
        if (settled && !await settled.evaluate(element => element.open)) {
          await page.click('[data-testid="workflow-settled"] > summary');
        }
      }
      await page.waitForSelector(selector);
      for (;;) {
        const handle = await page.evaluateHandle(target => {
          const closed = [];
          const targetElement = document.querySelector(target);
          // Clicking a disclosure summary must toggle that disclosure itself.
          const firstAncestor = targetElement?.tagName === 'SUMMARY'
            ? targetElement.parentElement?.parentElement
            : targetElement?.parentElement;
          for (let element = firstAncestor; element; element = element.parentElement) {
            if (element instanceof HTMLDetailsElement && !element.open) closed.unshift(element);
          }
          return closed[0]?.querySelector(':scope > summary') || null;
        }, selector);
        const summary = handle.asElement();
        if (!summary) { await handle.dispose(); break; }
        await summary.evaluate(element => element.scrollIntoView({ block: 'center' }));
        await summary.click();
        await handle.dispose();
      }
    };
    const fill = async (selector, value) => {
      await reveal(selector);
      await page.$eval(selector, (element, text) => {
      const prototype = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(prototype, 'value').set.call(element, text);
      element.dispatchEvent(new Event('input', { bubbles: true }));
      }, value);
    };
    const click = async selector => {
      await reveal(selector);
      await page.$eval(selector, element => element.scrollIntoView({ block: 'center' }));
      await page.locator(selector).click();
    };
    const capturedLayouts = [];
    const screenshot = async filename => {
      await page.evaluate(() => document.fonts.ready);
      assert.equal(await page.$eval('[data-testid="workflow-panel"]', element => element.scrollWidth > element.clientWidth + 1), false, `Overflow while capturing ${filename}`);
      capturedLayouts.push(await page.$eval('[data-testid="workflow-panel"]', (element, file) => ({ file, viewport: { width: innerWidth, height: innerHeight }, paneWidth: Math.round(element.getBoundingClientRect().width), layout: element.querySelector('[data-testid="workflow-run-inspector"]')?.getAttribute('data-layout') ?? 'setup' }), filename));
      await page.screenshot({ path: new URL(filename, evidence).pathname });
      if (['after-setup.png', 'after-running.png', 'after-review.png'].includes(filename)) {
        const panel = await page.$('[data-testid="workflow-panel"]');
        // Chrome's beyond-viewport element capture can temporarily change layout.
        await page.screenshot({ path: new URL(filename.replace('.png', '-pane.png'), evidence).pathname, clip: await panel.boundingBox(), captureBeyondViewport: false });
      }
    };
    const setPaneWidth = async width => {
      const resizer = await page.$('[data-testid="right-panel-resizer-hit"]');
      if (!resizer) return;
      const box = await resizer.boundingBox();
      const current = await page.$eval('[data-testid="workflow-panel"]', element => element.getBoundingClientRect().width);
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      await page.mouse.move(box.x + (current - width), box.y + box.height / 2, { steps: 15 });
      await page.mouse.up();
    };
    const readRuns = () => page.evaluate(() => JSON.parse(localStorage.getItem('codemux:dev-workflows:v1') || '{"runs":[]}').runs);
    const waitForRun = async (id, expression) => {
      await page.waitForFunction((runId, source) => {
        const run = JSON.parse(localStorage.getItem('codemux:dev-workflows:v1') || '{"runs":[]}').runs.find(item => item.id === runId);
        return run && new Function('run', `return (${source})`)(run);
      }, { timeout: 45000 }, id, expression);
    };
    await setPaneWidth(500);
    assert.equal(await page.$eval('[data-testid="workflow-script-settings"]', element => element.open), false);
    assert.equal(await page.$eval('[data-testid="workflow-limit-settings"]', element => element.open), false);
    assert.equal(await page.$eval('[data-testid="workflow-route-settings"]', element => element.open), false);
    assert.equal(await page.$eval('[data-testid="workflow-allow-writes"]', element => element.checked), false);
    await screenshot('after-setup.png');
    await fill('[data-testid="workflow-title"]', 'Mixed-provider architecture review');
    await fill('[data-testid="workflow-goal"]', 'Inspect six areas in parallel, then create a synthesis task from their results.');
    await fill('[data-testid="workflow-script"]', `await workflow.phase("Inspect six areas");
const areas = ["architecture", "tests", "performance", "UX", "adapters", "recovery"];
const findings = await parallel(areas, (area, index) => agent("Inspect " + area, {
  id: "inspect-" + index,
  title: "Inspect " + area,
  route_id: routes[index % routes.length].id
}));
await workflow.phase("Adapt: synthesize findings");
return await agent(JSON.stringify(findings), {
  id: "synthesis", title: "Synthesize findings", route_id: routes[1].id
});`);
    for (const provider of ['hermes', 'opencode', 'cursor', 'grok']) await click(`[data-testid="workflow-route-${provider}"]`);
    await reveal('[data-testid="workflow-concurrency"]');
    await page.select('[data-testid="workflow-concurrency"]', '2');
    assert.equal(await page.$eval('[data-testid="workflow-live-run"]', button => button.disabled), true);
    await click('[data-testid="workflow-save-script"]');
    await page.waitForFunction(() => JSON.parse(localStorage.getItem('codemux:dev-workflows:v1')).scripts.length === 1);
    await page.$eval('[data-testid="workflow-title"]', el => el.scrollIntoView());
    await screenshot('after-advanced.png');
    await click('[data-testid="workflow-dry-run"]');
    await page.waitForSelector('[data-testid="workflow-run-inspector"]');
    const id = (await readRuns())[0].id;
    await waitForRun(id, 'run.tasks.length === 6 && run.tasks.filter(task => task.status === "running").length === 2');
    await page.waitForSelector('[data-testid="workflow-task-inspect-5"]');
    await screenshot('after-running.png');
    await click('[data-testid="workflow-pause"]');
    await waitForRun(id, 'run.status === "paused" && run.tasks.filter(task => task.status === "running").length === 0');
    let run = (await readRuns()).find(item => item.id === id);
    assert.equal(run.tasks.filter(task => task.status === 'succeeded').length, 2);
    assert.equal(run.tasks.filter(task => task.status === 'queued').length, 4);
    await screenshot('after-paused.png');
    await page.reload({ waitUntil: 'networkidle0' });
    await page.waitForSelector('[data-testid="workflow-resume"]');
    await click('[data-testid="workflow-resume"]');
    await waitForRun(id, 'run.tasks.filter(task => task.status === "running").length === 2');
    await page.reload({ waitUntil: 'networkidle0' });
    await page.waitForSelector('[data-testid="workflow-resume"]');
    await click('[data-testid="workflow-resume"]');
    await waitForRun(id, 'run.status === "completed" && run.tasks.length === 7');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-run-status"]')?.getAttribute('data-status') === 'completed');
    run = (await readRuns()).find(item => item.id === id);
    assert.equal(new Set(run.tasks.map(task => task.spec.id)).size, 7);
    assert.ok(run.tasks.every(task => task.status === 'succeeded'));
    assert.equal(run.usage.total_tokens, 0);
    assert.equal(run.usage.cost_usd, 0);
    await screenshot('after-completed.png');
    assert.equal(await page.$eval('[data-testid="workflow-settled"]', element => element.open), false);
    await reveal('[data-testid="workflow-task-inspect-1"]');
    await page.focus('[data-testid="workflow-task-inspect-1"]');
    await page.keyboard.press('Enter');
    await page.waitForSelector('[data-testid="workflow-task-result"]');
    await page.waitForFunction(() => document.activeElement?.hasAttribute('data-workflow-detail-title'));
    assert.match(await page.$eval('[data-testid="workflow-task-detail"]', element => element.innerText), /Codex/);
    await screenshot('after-result.png');
    await click('[data-testid="workflow-back-checkpoints"]');
    await page.waitForFunction(() => document.activeElement?.getAttribute('data-workflow-task-id') === 'inspect-1');
    // Individual cancellation and explicit retry without starting a new run.
    await click('[data-testid="workflow-new"]');
    await fill('[data-testid="workflow-title"]', 'Cancellation contract');
    await fill('[data-testid="workflow-script"]', 'return await agent("Inspect cancellation", {id:"cancellable", title:"Cancellation test"});');
    await click('[data-testid="workflow-dry-run"]');
    await page.waitForSelector('[data-testid="workflow-task-cancellable"]');
    const cancelledId = (await readRuns())[0].id;
    await waitForRun(cancelledId, 'run.tasks[0].status === "running"');
    await click('[data-testid="workflow-task-cancellable"]');
    await click('[data-testid="workflow-cancel-task"]');
    await waitForRun(cancelledId, 'run.tasks[0].status === "cancelled"');
    await click('[data-testid="workflow-retry"]');
    await waitForRun(cancelledId, 'run.tasks[0].status === "succeeded"');
    const retried = (await readRuns()).find(item => item.id === cancelledId);
    assert.equal(retried.tasks[0].attempts.length, 2);
    // Writer authority cannot be enabled by the script.
    await click('[data-testid="workflow-new"]');
    await fill('[data-testid="workflow-title"]', 'Write permission contract');
    await fill('[data-testid="workflow-script"]', 'return await agent("Change file", {id:"writer", access:"write", scope:["src"]});');
    await click('[data-testid="workflow-dry-run"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-panel"]')?.innerText.includes('Allow file changes before'));
    await page.select('[data-testid="workflow-run-picker"]', id);
    await reveal('[data-testid="workflow-task-synthesis"]');
    await click('[data-testid="workflow-task-inspect-1"]');
    await click('[data-testid="right-panel-maximize"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-panel"]')?.getBoundingClientRect().width >= 720);
    await page.waitForSelector('[data-testid="workflow-task-detail"]');
    await screenshot('after-expanded.png');
    await click('[data-testid="right-panel-maximize"]');
    await click('[data-testid="workflow-back-checkpoints"]');

    // Retained live-mode snapshots are synthetic fixtures. The browser mock
    // returns before/after previews and refuses source-file application.
    const base = (await readRuns()).find(item => item.id === id);
    const reviewId = 'workflow-ui-retained-fixture';
    const writer = structuredClone(base.tasks[0]);
    writer.spec = { ...writer.spec, id: 'writer', title: 'Repair session recovery', access: 'write', scope: ['src/session'], dependencies: [] };
    writer.generation = 2;
    writer.status = 'succeeded';
    writer.error = null;
    writer.result = { summary: 'Retained synthetic change ready for review.' };
    const fixtureAttempt = (attemptId, generation) => ({
      ...structuredClone(base.tasks[0].attempts.at(-1)), id: attemptId, generation,
      status: 'succeeded', route_id: 'claude', output: writer.result,
      artifacts: [{ kind: 'workspace_snapshot', run_id: reviewId, attempt_id: attemptId, baseline_digest: 'synthetic-baseline', digest: `synthetic-${attemptId}`, changed_paths: ['src/session/recovery.ts', 'src/session/recovery.test.ts'] }],
    });
    writer.attempts = [fixtureAttempt('historical-attempt', 1), fixtureAttempt('current-attempt', 2)];
    writer.current_attempt = writer.attempts[1];
    const retained = { ...structuredClone(base), id: reviewId, spec: { ...base.spec, title: 'Refactor session recovery', mode: 'live', allow_writes: true }, tasks: [writer] };
    const seed = async fixture => {
      await page.evaluate(snapshot => {
        const state = JSON.parse(localStorage.getItem('codemux:dev-workflows:v1'));
        state.runs = [snapshot, ...state.runs.filter(run => run.id !== snapshot.id)];
        localStorage.setItem('codemux:dev-workflows:v1', JSON.stringify(state));
      }, fixture);
      await page.reload({ waitUntil: 'networkidle0' });
      await page.waitForSelector('[data-testid="workflow-run-picker"]');
      await page.select('[data-testid="workflow-run-picker"]', fixture.id);
      await page.waitForSelector('[data-testid="workflow-run-inspector"]');
    };
    await seed(retained);
    await page.waitForSelector('[data-testid="workflow-ready-review"]');
    await click('[data-testid="workflow-task-writer"]');
    assert.equal(await page.$eval('[data-testid="workflow-apply-artifact"]', element => element.disabled), true);
    await click('[data-testid="workflow-review-artifact"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-artifact-review"]')?.innerText.includes('Synthetic browser-preview retained change'));
    await page.waitForFunction(() => !document.querySelector('[data-testid="workflow-apply-artifact"]')?.disabled);
    assert.match(await page.$eval('[data-testid="workflow-apply-artifact"]', element => element.textContent), /Apply 2 file changes/);
    await page.select('[data-testid="workflow-artifact-file"]', 'src/session/recovery.test.ts');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-artifact-file"]')?.value === 'src/session/recovery.test.ts' && !document.querySelector('[data-testid="workflow-apply-artifact"]')?.disabled);
    await screenshot('after-review.png');
    await reveal('[data-testid="workflow-earlier-artifacts"]');
    await click('[data-testid="workflow-earlier-artifacts"] > summary');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-earlier-artifacts"]')?.innerText.includes('Earlier attempt · review only'));
    assert.equal(await page.$$eval('[data-testid="workflow-earlier-artifacts"] [data-testid="workflow-apply-artifact"]', elements => elements.length), 0);
    assert.equal(await page.$$eval('[data-testid="workflow-apply-artifact"]', elements => elements.filter(element => !element.disabled).length), 1);
    await click('[data-testid="workflow-apply-artifact"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-task-detail"]')?.innerText.includes('apply file changes from the desktop host'));

    const unknown = structuredClone(retained);
    unknown.id = 'workflow-ui-unknown-fixture';
    unknown.status = 'unknown';
    unknown.spec.title = 'Confirm worker stop';
    unknown.spec.allow_writes = false;
    unknown.resolved_limits.concurrency = 2;
    unknown.script.status = 'paused';
    unknown.tasks = [structuredClone(writer)];
    unknown.tasks[0].spec.access = 'read_only';
    unknown.tasks[0].status = 'unknown';
    unknown.tasks[0].error = 'The worker outcome is unconfirmed.';
    unknown.tasks[0].result = null;
    unknown.tasks[0].attempts = [];
    unknown.tasks[0].current_attempt = { ...fixtureAttempt('unknown-attempt', 2), status: 'unknown', reserved_tokens: 4096, artifacts: [] };
    unknown.tasks[0].attempts.push(unknown.tasks[0].current_attempt);
    unknown.usage.reserved_tokens = 4096;
    unknown.usage.tokens_unknown = true;
    await seed(unknown);
    await page.waitForSelector('[data-testid="workflow-needs-you"]');
    assert.match(await page.$eval('[data-testid="workflow-capacity"]', element => element.getAttribute('aria-label')), /1 of 2 worker slots occupied, 1 awaiting confirmation/);
    assert.equal(await page.$('[data-testid="workflow-retry"]'), null);
    await screenshot('after-unknown.png');
    await click('[data-testid="workflow-task-writer"]');
    await page.waitForSelector('[data-testid="workflow-reconcile"]');
    assert.equal(await page.$('[data-testid="workflow-retry"]'), null);
    await click('[data-testid="workflow-reconcile"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-panel"]')?.innerText.includes('no managed process to reconcile'));
    assert.equal((await readRuns()).find(run => run.id === unknown.id).tasks[0].status, 'unknown');

    const cancelled = structuredClone(retained);
    cancelled.id = 'workflow-ui-cancelled-fixture';
    cancelled.status = 'cancelled';
    cancelled.cancel_requested = true;
    for (const attempt of cancelled.tasks[0].attempts) for (const artifact of attempt.artifacts) artifact.run_id = cancelled.id;
    await seed(cancelled);
    assert.equal(await page.$('[data-testid="workflow-ready-review"]'), null);
    await click('[data-testid="workflow-task-writer"]');
    assert.equal(await page.$('[data-testid="workflow-apply-artifact"]'), null);
    assert.equal(await page.$('[data-testid="workflow-retry"]'), null);
    await click('[data-testid="workflow-review-artifact"]');
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-artifact-review"]')?.innerText.includes('Synthetic browser-preview retained change'));
    await screenshot('after-cancelled.png');

    await page.select('[data-testid="workflow-run-picker"]', id);
    await reveal('[data-testid="workflow-task-synthesis"]');
    // Workflow execution is desktop-only; browser clients switch to a separate
    // mobile/remote shell below 1024px. Check the smallest desktop width here.
    await page.setViewport({ width: 1024, height: 844, deviceScaleFactor: 1 });
    await setPaneWidth(360);
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-panel"]')?.getBoundingClientRect().width > 250);
    const overflow = await page.$eval('[data-testid="workflow-panel"]', element => element.scrollWidth > element.clientWidth + 1);
    assert.equal(overflow, false);
    await screenshot('after-narrow.png');
    assert.deepEqual(pageErrors, []);
    await fs.writeFile(new URL('verification.json', evidence), JSON.stringify({ checked: ['progressive setup with script/routes/limits collapsed and writes off', 'mixed-provider dynamic fan-out', 'two-worker ceiling', 'pause drains admitted work', 'save script', 'reload replay without duplicate tasks', 'dynamic synthesis', 'task output/route attribution', 'settled tasks collapsed until requested', 'keyboard task selection and return focus', 'individual cancellation/retry', 'write permission refusal', 'expanded checkpoint/detail split', 'retained file review before whole-manifest apply', 'historical artifacts inspectable without current authority', 'browser host refuses file application', 'Unknown needs attention without retry or released capacity', 'cancelled run preserves evidence and suppresses apply/retry', 'narrow pane overflow'], providerCalls: 0, modelTokens: 0, capturedLayouts, screenshots: ['before.png', 'after-setup.png', 'after-setup-pane.png', 'after-advanced.png', 'after-running.png', 'after-running-pane.png', 'after-paused.png', 'after-completed.png', 'after-result.png', 'after-expanded.png', 'after-review.png', 'after-review-pane.png', 'after-unknown.png', 'after-cancelled.png', 'after-narrow.png'] }, null, 2) + '\n');
    console.log('Workflow UI E2E passed: 19 flows, no provider calls or tokens, screenshots saved.');
  }
} catch (error) {
  if (qaPage && !qaPage.isClosed()) {
    const runs = await qaPage.evaluate(() => { try { return JSON.parse(localStorage.getItem('codemux:dev-workflows:v1') || '{"runs":[]}').runs; } catch { return []; } });
    await fs.writeFile(new URL('failed-flow-state.json', evidence), JSON.stringify({ error: String(error), url: qaPage.url(), runs }, null, 2) + '\n');
    await qaPage.screenshot({ path: new URL('failed-flow.png', evidence).pathname });
    console.error('Synthetic workflow state at failure:', JSON.stringify(runs.map(run => ({ status: run.status, script: run.script, tasks: run.tasks.map(task => ({ id: task.spec.id, status: task.status })) }))));
  }
  throw error;
} finally {
  await browser.close();
}
