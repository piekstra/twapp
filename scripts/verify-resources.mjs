// Headless Resources tab checks; native commands use synthetic sessions.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.default || playwright;
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/resources.json', import.meta.url), 'utf8'));
const disk = snapshot.sessions.map((s, i) => ({ key: s.key, dir_kb: 1024 * 1024 * (i + 1), temp_kb: i % 2 ? null : 300 * 1024 }));
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-resources-ui-'));
await fs.mkdir(dir, { recursive: true });
const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (const theme of ['light', 'dark']) for (const width of [260, 340, 480]) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: theme });
    await page.clock.setFixedTime(new Date('2026-11-03T12:00:00Z'));
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, width, snapshot, disk }) => {
      localStorage.setItem('twapp-layout', JSON.stringify({ mode: 'right', sidebarWidth: width, switcherCollapsedLanes: [] }));
      window.__calls = [];
      let counter = 0;
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++counter,
        unregisterCallback: () => {},
        invoke: async (cmd, args) => {
          if (cmd === 'hub_snapshot') return snapshot;
          if (cmd === 'hub_disk_usage') return disk;
          if (cmd === 'get_theme_preference') return theme;
          if (cmd === 'get_global_config') return { agent_providers: ['claude', 'codex', 'antigravity'] };
          if (cmd === 'get_notes' || cmd === 'get_session_history') return [];
          if (cmd === 'get_ticket') return null;
          if (cmd === 'get_font_family_preference') return 'monospace';
          if (cmd === 'plugin:app|version') return '0.0.0';
          if (cmd === 'plugin:event|listen') return ++counter;
          window.__calls.push({ cmd, args });
          return null;
        },
      };
    }, { theme, width, snapshot, disk });
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1420');
    await page.locator('.rail-home').click();
    await page.getByRole('button', { name: 'Resources', exact: true }).click();
    const table = page.locator('.resources-table');
    await table.waitFor();

    const rows = table.locator('tbody tr');
    assert.equal(await rows.count(), snapshot.sessions.length);
    assert.equal(await rows.first().locator('.board-session').textContent(), 'Release build', 'the heaviest session comes first');
    assert.deepEqual(await table.locator('.resources-flag').allTextContents(), ['Idle 5d', 'Parked, still running']);
    // Only running sessions that are not at work or waiting on a prompt can be stopped.
    assert.equal(await table.getByRole('button', { name: 'Stop', exact: true }).count(), 3);
    assert.match(await page.locator('.resources .overview-stat').first().textContent(), /^4\.3 GB\s*memory in 5 running$/);

    await page.getByRole('button', { name: 'Measure disk', exact: true }).click();
    await page.getByText('on disk across these sessions.').waitFor();
    assert.equal(await rows.filter({ hasText: 'Stopped session' }).locator('td.num').last().textContent(), '6.0 GB');

    const overflow = await page.locator('.overview-body').evaluate(el => {
      const box = el.getBoundingClientRect();
      return [...el.querySelectorAll('.resources-table, .resources-table td, .overview-summary-row')]
        .filter(e => e.getBoundingClientRect().right > box.right + 1).length;
    });
    assert.equal(overflow, 0, 'nothing runs past the overview');
    await page.screenshot({ path: path.join(dir, `resources-${theme}-${width}.png`) });

    const docs = rows.filter({ hasText: 'Docs refresh' });
    await docs.getByRole('button', { name: 'Stop', exact: true }).click();
    await docs.getByText('Opening the session resumes it.').waitFor();
    await page.screenshot({ path: path.join(dir, `resources-confirm-${theme}-${width}.png`) });
    const before = await page.evaluate(() => window.__calls.length);
    await docs.getByRole('button', { name: 'Cancel', exact: true }).click();
    assert.equal(await page.evaluate(() => window.__calls.length), before, 'Cancel stops nothing');
    await docs.getByRole('button', { name: 'Stop', exact: true }).click();
    await docs.locator('.resources-confirm').getByRole('button', { name: 'Stop', exact: true }).click();
    await page.waitForFunction(() => window.__calls.some(c => c.cmd === 'hub_close_tab'));
    assert.deepEqual(
      await page.evaluate(() => window.__calls.filter(c => c.cmd === 'hub_close_tab')),
      [{ cmd: 'hub_close_tab', args: { key: '/work/docs', tab: 'main' } }],
    );
    results.push({ theme, width, rows: snapshot.sessions.length, stopButtons: 3, overflow });
    await page.close();
  }
} finally {
  await browser.close();
}
console.log(JSON.stringify({ dir, results }, null, 2));
