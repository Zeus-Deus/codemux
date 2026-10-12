/** Responsive evidence uses only the development mock: no provider calls. */
import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';
import path from 'node:path';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';

const moduleUrl = process.env.CODEMUX_PUPPETEER_MODULE || pathToFileURL(path.join(execFileSync('npm', ['root', '-g'], { encoding: 'utf8' }).trim(), 'puppeteer/node_modules/puppeteer-core/lib/puppeteer/puppeteer-core.js')).href;
const { default: puppeteer } = await import(moduleUrl);
const evidence = path.resolve(process.env.CODEMUX_WORKFLOW_RESPONSIVE_EVIDENCE || 'docs/features/assets/workflow-ui-responsive');
await fs.mkdir(evidence, { recursive: true });
const appUrl = process.env.CODEMUX_WORKFLOW_URL || 'http://127.0.0.1:1420/';
const browser = await puppeteer.launch({ executablePath: process.env.CODEMUX_CHROMIUM || '/usr/bin/chromium', headless: true, args: ['--no-sandbox', '--disable-dev-shm-usage'] });
const results = [];
const errors = [];
const failedRequests = [];
const navigations = [];
const browserConsole = [];
let page;
try {
  page = await browser.newPage();
  page.on('pageerror', error => errors.push(String(error)));
  page.on('framenavigated', frame => { if (frame === page.mainFrame()) navigations.push({ url: frame.url(), time: Date.now() }); });
  page.on('console', event => { if (event.text().includes('[vite]')) browserConsole.push(event.text()); });
  await page.setRequestInterception(true);
  page.on('request', request => {
    const url = new URL(request.url());
    if (['http:', 'https:'].includes(url.protocol) && !['127.0.0.1', 'localhost'].includes(url.hostname)) { failedRequests.push({ origin: url.origin, path: url.pathname, resourceType: request.resourceType() }); void request.abort(); }
    else void request.continue();
  });
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  await page.emulateMediaFeatures([{ name: 'prefers-reduced-motion', value: 'reduce' }]);
  await page.goto(`${appUrl}${appUrl.includes('?') ? '&' : '?'}workflowDelay=10`, { waitUntil: 'networkidle0' });
  await page.locator('[data-testid="composer-workflows-toggle"]').click();
  await page.waitForSelector('[data-testid="workflow-panel"]');

  const click = async selector => {
    if (selector.startsWith('[data-testid="workflow-task-task-') && !await page.$(selector)) {
      const settled = await page.$('[data-testid="workflow-settled"]');
      if (settled && !await settled.evaluate(element => element.open)) {
        await settled.evaluate(element => element.querySelector('summary').click());
      }
    }
    await page.waitForSelector(selector);
    await page.$eval(selector, element => element.scrollIntoView({ block: 'center' }));
    await page.locator(selector).click();
  };
  const paneWidth = async width => {
    const hit = await page.$('[data-testid="right-panel-resizer-hit"]');
    if (!hit) return;
    const box = await hit.boundingBox();
    const current = await page.$eval('[data-testid="workflow-panel"]', element => element.getBoundingClientRect().width);
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + (current - width), box.y + box.height / 2, { steps: 10 });
    await page.mouse.up();
    await page.waitForFunction(() => document.querySelector('[data-testid="workflow-panel"]')?.clientWidth > 0);
  };
  const inspect = async name => {
    await page.evaluate(async () => { await document.fonts.ready; await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))); });
    const measurement = await page.$eval('[data-testid="workflow-panel"]', (panel, label) => {
      const visible = element => {
        const style = getComputedStyle(element);
        return style.visibility !== 'hidden' && style.display !== 'none' && element.getClientRects().length > 0;
      };
      const rect = panel.getBoundingClientRect();
      // Textareas and code previews deliberately scroll their own content.
      const overflowing = [...panel.querySelectorAll('*')].filter(element => visible(element) && !['TEXTAREA', 'INPUT', 'SELECT', 'PRE', 'CODE'].includes(element.tagName) && getComputedStyle(element).overflowX === 'visible' && element.scrollWidth > element.clientWidth + 2 && element.getBoundingClientRect().right + (element.scrollWidth - element.clientWidth) > rect.right + 2).map(element => ({ tag: element.tagName, testId: element.getAttribute('data-testid'), width: element.clientWidth, content: element.scrollWidth, text: element.textContent?.slice(0, 100) }));
      const scroller = panel.lastElementChild;
      return { name: label, viewport: { width: innerWidth, height: innerHeight }, pane: { width: Math.round(rect.width), height: Math.round(rect.height), overflow: panel.scrollWidth > panel.clientWidth + 1 }, layout: panel.querySelector('[data-testid="workflow-run-inspector"]')?.getAttribute('data-layout') || 'setup', documentOverflow: document.documentElement.scrollWidth > innerWidth + 1, overflowing, renderedTaskRows: panel.querySelectorAll('[data-workflow-task-id]').length, scrollable: !!scroller && scroller.scrollHeight > scroller.clientHeight };
    }, name);
    assert.equal(measurement.pane.overflow, false, `${name}: pane overflow`);
    assert.deepEqual(measurement.overflowing, [], `${name}: child content is clipped`);
    measurement.navigationCount = navigations.length;
    results.push(measurement);
    await page.screenshot({ path: path.join(evidence, `${name}.png`) });
    const panel = await page.$('[data-testid="workflow-panel"]');
    // Capture the visible pane without Chrome temporarily changing its viewport.
    await page.screenshot({ path: path.join(evidence, `${name}-pane.png`), clip: await panel.boundingBox(), captureBeyondViewport: false });
    const finalLayout = await page.$eval('[data-testid="workflow-panel"]', element => element.querySelector('[data-testid="workflow-run-inspector"]')?.getAttribute('data-layout') || 'setup');
    assert.equal(finalLayout, measurement.layout, `${name}: layout changed while capturing evidence`);
  };

  await paneWidth(360);
  await inspect('setup-narrow');
  await click('[data-testid="workflow-route-claude"]');
  await click('[data-testid="workflow-route-codex"]');
  await click('[data-testid="workflow-route-opencode"]');
  await click('[data-testid="workflow-route-cursor"]');
  await click('[data-testid="workflow-route-settings"] > summary');
  assert.equal(await page.$eval('[data-testid="workflow-model-opencode"]', element => element.placeholder), 'provider/model');
  assert.equal(await page.$eval('[aria-label="Cursor effort"]', element => element.placeholder), 'Leave empty');
  assert.equal(await page.$eval('[data-testid="workflow-dry-run"]', element => element.disabled), false);
  assert.equal(await page.$eval('[data-testid="workflow-live-run"]', element => element.disabled), true);
  await inspect('setup-provider-requirements-narrow');
  await page.setViewport({ width: 2560, height: 1440, deviceScaleFactor: 1 });
  await click('[data-testid="right-panel-maximize"]');
  await inspect('setup-provider-requirements-wide');
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  await click('[data-testid="right-panel-maximize"]');
  await click('[data-testid="workflow-route-opencode"]');
  await click('[data-testid="workflow-route-cursor"]');
  await click('[data-testid="workflow-route-claude"]');
  await click('[data-testid="workflow-route-codex"]');
  await click('[data-testid="workflow-route-settings"] > summary');
  await paneWidth(360);
  await page.locator('[data-testid="workflow-dry-run"]').click();
  await page.waitForFunction(() => document.querySelector('[data-testid="workflow-run-status"]')?.getAttribute('data-status') === 'completed');
  const base = await page.evaluate(() => JSON.parse(localStorage.getItem('codemux:dev-workflows:v1')).runs[0]);
  assert.equal(base.usage.total_tokens, 0);
  const workspaceId = base.spec.workspace_id;
  const providers = ['claude', 'codex', 'hermes', 'opencode', 'cursor', 'grok'];
  const routes = Array.from({ length: 24 }, (_, i) => ({ id: `route-${i}-${'long-route-name'.repeat(3)}`, provider: providers[i % providers.length], model: `${providers[i % providers.length]}-${'model-identifier'.repeat(5)}` }));
  const seed = async snapshot => {
    await page.evaluate(run => {
      const state = JSON.parse(localStorage.getItem('codemux:dev-workflows:v1'));
      state.runs = [run, ...state.runs.filter(item => item.id !== run.id)];
      localStorage.setItem('codemux:dev-workflows:v1', JSON.stringify(state));
    }, snapshot);
    await page.reload({ waitUntil: 'networkidle0' });
    await page.waitForSelector('[data-testid="workflow-run-picker"]');
    await page.select('[data-testid="workflow-run-picker"]', snapshot.id);
    await page.waitForSelector('[data-testid="workflow-run-inspector"]');
  };
  const crowded = structuredClone(base);
  crowded.id = 'responsive-large-graph';
  crowded.status = 'paused';
  crowded.pause_requested = true;
  crowded.spec.title = 'Review-' + 'unbroken-workflow-title'.repeat(6);
  crowded.spec.goal = 'Long context '.repeat(1000);
  crowded.spec.routes = routes;
  crowded.spec.limits.max_tasks = 1000;
  crowded.resolved_limits = { ...crowded.resolved_limits, max_tasks: 1000, concurrency: 256 };
  crowded.script = { ...crowded.script, status: 'paused', phase: 'Coordinate-' + 'unbroken-phase-name'.repeat(6) };
  crowded.tasks = Array.from({ length: 1000 }, (_, i) => {
    const item = structuredClone(base.tasks[0]);
    const status = i === 1 ? 'stopping' : i % 4 === 0 ? 'failed' : i % 4 === 1 ? 'queued' : i % 4 === 2 ? 'succeeded' : 'unknown';
    item.spec = { ...item.spec, id: `task-${i}`, title: `${i}: ` + 'UnbrokenTaskName'.repeat(10), route_id: routes[i % routes.length].id, dependencies: [] };
    item.status = status;
    item.result = status === 'succeeded' ? { summary: 'Readable evidence '.repeat(i === 2 ? 1500 : 1) } : null;
    item.error = status === 'failed' || status === 'unknown' ? 'UnbrokenProviderError'.repeat(20) : null;
    item.attempts = [];
    item.current_attempt = null;
    if (status !== 'queued') {
      const attempt = { ...structuredClone(base.tasks[0].attempts.at(-1)), id: `attempt-${i}`, status, route_id: item.spec.route_id, output: item.result, error: item.error, artifacts: [] };
      item.current_attempt = attempt;
      item.attempts.push(attempt);
    }
    return item;
  });
  await seed(crowded);
  await inspect('large-graph-narrow');
  const motion = await page.$$eval('[data-testid="workflow-panel"] .animate-spin', elements => elements.map(element => getComputedStyle(element).animationName));
  assert.ok(motion.length > 0 && motion.every(name => name === 'none'), 'Worker motion is disabled under reduced-motion preference');
  assert.ok(results.at(-1).renderedTaskRows <= 200, 'Checkpoint groups page a large graph');
  // Pick a row near the bottom of the first page so a tall detail must reset
  // the carried list scroll position instead of covering its Back control.
  await click('[data-testid="workflow-task-task-196"]');
  await page.waitForFunction(() => document.activeElement?.hasAttribute('data-workflow-detail-title'));
  const detailNavigationVisible = () => page.$eval('[data-testid="workflow-back-checkpoints"]', element => {
    const scroller = document.querySelector('[data-testid="workflow-panel"]').lastElementChild.getBoundingClientRect();
    const button = element.getBoundingClientRect();
    return button.top >= scroller.top && button.bottom <= scroller.bottom;
  });
  assert.equal(await detailNavigationVisible(), true, 'Deep-row selection keeps Back visible');
  await inspect('detail-long-content');
  await click('[data-testid="workflow-back-checkpoints"]');
  await page.waitForFunction(() => document.activeElement?.getAttribute('data-workflow-task-id') === 'task-196');
  await click('[data-testid="workflow-settled"] > summary');
  assert.ok(await page.$$eval('[data-workflow-task-id]', rows => rows.length) <= 300);
  await inspect('large-graph-settled');
  await click('[data-testid="workflow-task-task-2"]');
  await inspect('long-result-narrow');

  await page.setViewport({ width: 2560, height: 1440, deviceScaleFactor: 1 });
  await click('[data-testid="right-panel-maximize"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="workflow-run-inspector"]')?.getAttribute('data-layout') === 'split');
  await inspect('wide-split');
  await page.setViewport({ width: 1280, height: 480, deviceScaleFactor: 1 });
  await page.waitForFunction(() => document.querySelector('[data-workflow-task-id="task-2"]')?.getAttribute('aria-pressed') === 'true');
  await page.waitForSelector('[data-testid="workflow-task-detail"]', { timeout: 3000 });
  await click('[data-testid="right-panel-maximize"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="workflow-run-inspector"]')?.getAttribute('data-layout') === 'single');
  await page.waitForSelector('[data-testid="workflow-task-detail"]');
  assert.equal(await detailNavigationVisible(), true, 'Split-to-single resize keeps Back visible in a short pane');
  await inspect('short-height-detail');
  await click('[data-testid="workflow-back-checkpoints"]');
  await inspect('short-height-checkpoints');
  await click('[data-testid="workflow-new"]');

  await page.evaluate(({ id, routeOptions, sourceDraft }) => {
    const state = JSON.parse(localStorage.getItem('codemux:workflow-ui:v1') || '{"state":{"drafts":{},"selectedRuns":{}},"version":0}');
    const original = state.state.drafts[id] || sourceDraft;
    state.state.drafts[id] = { ...original, title: 'Long workflow name '.repeat(8), goal: 'Long goal '.repeat(1000), source: 'return await agent(goal);', routes: routeOptions };
    localStorage.setItem('codemux:workflow-ui:v1', JSON.stringify(state));
  }, { id: workspaceId, routeOptions: routes, sourceDraft: { title: base.spec.title, goal: base.spec.goal, source: base.spec.script?.source || 'return await agent(goal);', allowWrites: false, routes: base.spec.routes, limits: base.spec.limits } });
  await page.reload({ waitUntil: 'networkidle0' });
  await page.waitForSelector('[data-testid="workflow-setup"]');
  await click('[data-testid="workflow-route-settings"] > summary');
  await click('[data-testid="workflow-limit-settings"] > summary');
  await click('[data-testid="workflow-script-settings"] > summary');
  await paneWidth(360);
  await inspect('many-routes-short-height');
  await click('[data-testid="workflow-dry-run"]');
  await page.waitForFunction(() => document.querySelector('[data-testid="workflow-run-status"]')?.getAttribute('data-status') === 'completed');

  // Emulate the CSS viewport and pixel density of 200% desktop zoom.
  // deviceScaleFactor alone would only test screenshot DPI.
  await page.setViewport({ width: 2048, height: 1536, deviceScaleFactor: 1 });
  const cdp = await page.createCDPSession();
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1024, height: 768, deviceScaleFactor: 2, mobile: false });
  await paneWidth(360);
  await inspect('zoom-200-percent');
  results.at(-1).zoom = { physicalSize: [2048, 1536], effectiveCssViewport: [1024, 768], deviceScaleFactor: 2, note: '200% desktop reflow equivalent using CDP CSS viewport and pixel density; not a browser setting mutation' };
  await cdp.send('Emulation.clearDeviceMetricsOverride');
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });

  // Mobile browser shell is a separate product boundary. Existing local-mock
  // state allows inspecting the pane without claiming remote live support.
  for (const width of [1023, 768, 375]) {
    await page.setViewport({ width, height: 844, deviceScaleFactor: 1 });
    const panel = await page.$('[data-testid="workflow-panel"]');
    const boundary = await page.evaluate(() => ({ mobile: document.documentElement.dataset.mobile === 'true', browserClient: document.documentElement.dataset.browserClient === 'true', workflowVisible: !!document.querySelector('[data-testid="workflow-panel"]')?.getClientRects().length, hasWorkflowComposerEntry: !!document.querySelector('[data-testid="composer-workflows-toggle"]'), documentOverflow: document.documentElement.scrollWidth > innerWidth + 1 }));
    results.push({ name: `browser-mobile-${width}`, viewport: { width, height: 844 }, boundary });
    assert.equal(boundary.documentOverflow, false, `Mobile shell overflows at ${width}px`);
    assert.equal(boundary.hasWorkflowComposerEntry, false, 'Mobile shell does not imply local managed-worker support');
    if (panel && boundary.workflowVisible) await inspect(`browser-mobile-pane-${width}`);
    else await page.screenshot({ path: path.join(evidence, `browser-mobile-${width}.png`) });
  }
  // The dev mock installs Tauri IPC after browser bootstrap. Remove only its
  // browser presentation hint to exercise native window layout; IPC stays mock.
  await page.evaluate(() => {
    delete document.documentElement.dataset.browserClient;
    delete document.documentElement.dataset.mobile;
    const observer = new MutationObserver(() => {
      if (document.documentElement.dataset.mobile) delete document.documentElement.dataset.mobile;
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-mobile"] });
  });
  await page.setViewport({ width: 1440, height: 1000, deviceScaleFactor: 1 });
  if (!await page.$('[data-testid="workflow-panel"]')) await click('[data-testid="composer-workflows-toggle"]');
  await page.waitForSelector('[data-testid="workflow-panel"]');
  await click('[aria-label="Toggle sidebar"]');
  const maximized = await page.$eval('[data-testid="right-panel-maximize"]', element => element.getAttribute('aria-pressed') === 'true');
  if (!maximized) await click('[data-testid="right-panel-maximize"]');
  for (const width of [960, 640, 480, 375]) {
    await page.setViewport({ width, height: 720, deviceScaleFactor: 1 });
    await page.waitForSelector('[data-testid="workflow-panel"]');
    await inspect(`native-layout-${width}`);
    assert.equal(results.at(-1).documentOverflow, false, `Native-layout mock overflows at ${width}px`);
  }
  assert.deepEqual(errors, []);
  assert.ok(failedRequests.every(request => request.resourceType === 'image'), 'Only existing mock favicon image requests may be attempted externally; all are blocked');
  await fs.writeFile(path.join(evidence, 'verification.json'), JSON.stringify({ providerCalls: 0, modelTokens: 0, checked: ['360px pane', 'provider model/effort requirements at narrow and wide widths', '2560px wide split', '480px height', '200% effective reflow', 'long unbroken names and provider errors', '24 routes', '1000 task graph with bounded visible rows', 'large result', 'keyboard detail focus and Back restoration', 'reduced-motion media', 'mobile browser shell boundaries', 'native layout emulation down to 375px with collapsed sidebar'], results, blockedExternalRequests: failedRequests, pageErrors: errors }, null, 2) + '\n');
  console.log(`Workflow responsive review passed: ${results.length} measured layouts, no provider calls or tokens.`);
} catch (error) {
  if (page && !page.isClosed()) {
    await page.screenshot({ path: path.join(evidence, 'failure.png') });
    await fs.writeFile(path.join(evidence, 'failure.json'), JSON.stringify({ error: String(error), results, pageErrors: errors, navigations, browserConsole }, null, 2) + '\n');
  }
  throw error;
} finally {
  await browser.close();
}
