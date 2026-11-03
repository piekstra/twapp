// Headless Resources tab checks; native commands and hub events use synthetic sessions.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.default || playwright;
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/resources.json', import.meta.url), 'utf8'));
const outside = JSON.parse(await fs.readFile(new URL('./fixtures/outside-agents.json', import.meta.url), 'utf8'));
const disk = snapshot.sessions.map((s, i) => s.key === '/work/archive'
  ? { key: s.key, dir_kb: null, temp_kb: null, gave_up: true, measured_at: '2026-11-03T11:30:00Z' }
  : { key: s.key, dir_kb: 1024 * 1024 * (i + 1), temp_kb: i % 2 ? null : 300 * 1024, gave_up: false, measured_at: '2026-11-03T11:30:00Z' });
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-resources-ui-'));
await fs.mkdir(dir, { recursive: true });
const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (const theme of ['light', 'dark']) for (const width of [260, 340, 480]) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: theme });
    await page.clock.setFixedTime(new Date('2026-11-03T12:00:00Z'));
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, width, snapshot, outside }) => {
      localStorage.setItem('twapp-layout', JSON.stringify({ mode: 'right', sidebarWidth: width, switcherCollapsedLanes: [] }));
      window.__calls = [];
      window.__outside = outside.map(a => ({ ...a }));
      const callbacks = {};
      const listeners = [];
      let counter = 0;
      window.__emit = (event, payload) =>
        listeners.filter(l => l.event === event).forEach(l => callbacks[l.handler]?.({ event, id: 0, payload }));
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: (cb) => { callbacks[++counter] = cb; return counter; },
        unregisterCallback: () => {},
        invoke: async (cmd, args) => {
          if (cmd === 'hub_snapshot') return snapshot;
          if (cmd === 'hub_session_disk') return { running: false, sessions: [] };
          if (cmd === 'hub_outside_agents') return window.__outside;
          if (cmd === 'get_theme_preference') return theme;
          if (cmd === 'get_global_config') return { agent_providers: ['claude', 'codex', 'antigravity'] };
          if (cmd === 'get_notes' || cmd === 'get_session_history') return [];
          if (cmd === 'get_ticket') return null;
          if (cmd === 'get_font_family_preference') return 'monospace';
          if (cmd === 'plugin:app|version') return '0.0.0';
          if (cmd === 'plugin:event|listen') { listeners.push({ event: args.event, handler: args.handler }); return ++counter; }
          if (cmd === 'plugin:event|unlisten') return null;
          window.__calls.push({ cmd, args });
          if (cmd === 'hub_measure_disk') return true;
          if (cmd === 'hub_stop_outside_agent') {
            if (args.pid === 500) throw 'It is no longer idle, so nothing was stopped.';
            window.__outside = window.__outside.filter(a => a.pid !== args.pid);
            return true;
          }
          return null;
        },
      };
    }, { theme, width, snapshot, outside });
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1420');
    await page.locator('.rail-home').click();
    await page.getByRole('button', { name: 'Resources', exact: true }).click();
    const table = page.getByRole('table', { name: 'Resources by session' });
    await table.waitFor();

    const rows = table.locator('tbody tr');
    const names = async () => (await rows.locator('.resources-session').allTextContents());
    assert.equal(await rows.count(), snapshot.sessions.length);
    assert.deepEqual((await names()).slice(0, 2), ['Release build', 'Agent stopped, dev server tab still up'],
      'heaviest first, and a tab that outlives its agent still counts');
    assert.deepEqual(await table.locator('.resources-flag').allTextContents(), ['Idle 5d', 'Parked, still running']);
    // Stop only where the agent runs and is neither at work nor waiting on a prompt.
    assert.deepEqual(
      (await table.getByRole('button', { name: /^Stop / }).evaluateAll(els => els.map(e => e.getAttribute('aria-label')))).sort(),
      ['Stop Docs refresh', 'Stop Old investigation', 'Stop Payments retry worker with a deliberately long session name'],
    );
    assert.equal(await table.getByRole('columnheader', { name: 'Actions' }).count(), 1);
    assert.match(await page.locator('.resources .overview-stat').first().textContent(), /^5\.5 GB\s*memory in 6 running$/);

    // A refresh arrives as one event for every changed session.
    await page.evaluate(() => window.__emit('hub:resources', {
      '/work/docs': { rss_kb: 3 * 1024 * 1024, cpu_tenths: 5, processes: 4 },
      '/work/payments': null,
    }));
    await page.waitForFunction(() => document.querySelector('.resources-table tbody tr .resources-session')?.textContent === 'Docs refresh');
    assert.equal(await rows.filter({ hasText: 'Docs refresh' }).locator('td.num').first().textContent(), '3.0 GB');
    assert.equal(await rows.filter({ hasText: 'Payments retry' }).locator('td.num').first().textContent(), '');

    // Disk is measured by the hub; results land one session at a time.
    await page.getByRole('button', { name: 'Measure disk', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Measuring one session at a time' }).waitFor();
    assert.equal(await page.getByRole('button', { name: 'Measuring disk...', exact: true }).isDisabled(), true);
    await page.evaluate((disk) => {
      for (const session of disk) window.__emit('hub:disk', { session, running: true });
      window.__emit('hub:disk', { session: null, running: false });
    }, disk);
    const summary = page.getByRole('status').filter({ hasText: 'on disk across 7 of 8 sessions' });
    await summary.waitFor();
    assert.match(await summary.textContent(), /; 1 took over two minutes and is left out\. Measured 30m ago\.$/);
    assert.equal(await rows.filter({ hasText: 'Stopped session' }).locator('td.num').last().textContent(), '6.0 GB');
    assert.equal(await rows.filter({ hasText: 'Archived exploration' }).locator('td.num').last().textContent(), 'over 2 min',
      'a walk that was given up says so instead of reading as empty');
    assert.equal((await page.evaluate(() => window.__calls.filter(c => c.cmd === 'hub_measure_disk'))).length, 1);

    const overflow = await page.locator('.overview-body').evaluate(el => {
      const box = el.getBoundingClientRect();
      return [...el.querySelectorAll('.resources-table, .resources-table td, .overview-summary-row')]
        .filter(e => e.getBoundingClientRect().right > box.right + 1).length;
    });
    assert.equal(overflow, 0, 'nothing runs past the overview');
    await page.screenshot({ path: path.join(dir, `resources-${theme}-${width}.png`) });

    // Keyboard: focus follows the controls as they swap.
    const old = rows.filter({ hasText: 'Old investigation' });
    await old.getByRole('button', { name: 'Stop Old investigation' }).focus();
    await page.keyboard.press('Enter');
    await old.getByRole('group', { name: 'Stop Old investigation?' }).waitFor();
    assert.equal(await page.evaluate(() => document.activeElement?.getAttribute('aria-label')), 'Cancel stopping Old investigation');
    await page.screenshot({ path: path.join(dir, `resources-confirm-${theme}-${width}.png`) });
    const before = await page.evaluate(() => window.__calls.length);
    await page.keyboard.press('Enter');
    assert.equal(await page.evaluate(() => window.__calls.length), before, 'Cancel stops nothing');
    assert.equal(await page.evaluate(() => document.activeElement?.getAttribute('aria-label')), 'Stop Old investigation');
    await page.keyboard.press('Enter');
    await old.getByRole('group').getByRole('button', { name: 'Stop Old investigation' }).click();
    await page.waitForFunction(() => window.__calls.some(c => c.cmd === 'hub_close_tab'));
    assert.deepEqual(
      await page.evaluate(() => window.__calls.filter(c => c.cmd === 'hub_close_tab')),
      [{ cmd: 'hub_close_tab', args: { key: '/work/old', tab: 'main' } }],
    );
    assert.equal(await page.evaluate(() => document.activeElement?.classList.contains('board-session')), true, 'focus lands on the session name');
    // Agent processes outside every session: listed heaviest first as the hub
    // sends them, Stop only on idle ones, never on the daemon.
    const strays = page.getByRole('table', { name: 'Agent processes outside your sessions' });
    await strays.waitFor();
    assert.equal(await strays.locator('tbody tr').count(), outside.length);
    assert.match(await page.locator('#resources-outside-head .count').textContent(), /^3\.3 GB in 6 · 1\.6 GB in 3 idle$/);
    assert.deepEqual(
      (await strays.getByRole('button', { name: /^Stop / }).evaluateAll(els => els.map(e => e.getAttribute('aria-label')))).sort(),
      ['Stop claude --bg-pty-host 24099', 'Stop claude --bg-pty-host 24982', 'Stop codex exec 500'],
    );
    assert.equal(await strays.locator('tbody tr').filter({ hasText: '21980' }).locator('.resources-flag').textContent(), 'Daemon');
    await page.screenshot({ path: path.join(dir, `outside-${theme}-${width}.png`), fullPage: true });
    const host = strays.locator('tbody tr').filter({ hasText: '24982' });
    await host.getByRole('button', { name: 'Stop claude --bg-pty-host 24982' }).focus();
    await page.keyboard.press('Enter');
    assert.equal(await page.evaluate(() => document.activeElement?.getAttribute('aria-label')), 'Cancel stopping claude --bg-pty-host 24982');
    await host.getByRole('group').getByRole('button', { name: 'Stop claude --bg-pty-host 24982' }).click();
    await page.waitForFunction(() => window.__calls.some(c => c.cmd === 'hub_stop_outside_agent'));
    assert.deepEqual(
      await page.evaluate(() => window.__calls.filter(c => c.cmd === 'hub_stop_outside_agent')),
      [{ cmd: 'hub_stop_outside_agent', args: { pid: 24982 } }],
    );
    // Success is announced, the row goes, and focus moves to the next Stop.
    await page.locator('.resources-outside [role="status"]').filter({ hasText: 'claude --bg-pty-host 24982 stopped.' }).waitFor({ state: 'attached' });
    await page.waitForFunction(() => !document.querySelector('.resources-outside tbody')?.textContent?.includes('24982'));
    await page.waitForFunction(() => document.activeElement?.getAttribute('aria-label') === 'Stop claude --bg-pty-host 24099');
    // A refused stop says why, and a refresh does not clear it.
    const codex = strays.locator('tbody tr').filter({ hasText: 'codex exec' });
    await codex.getByRole('button', { name: 'Stop codex exec 500' }).click();
    await codex.getByRole('group').getByRole('button', { name: 'Stop codex exec 500' }).click();
    const refused = page.locator('.resources-outside [role="alert"]').filter({ hasText: 'It is no longer idle' });
    await refused.waitFor();
    await page.evaluate(() => new Promise(r => setTimeout(r, 300)));
    assert.equal(await refused.count(), 1, 'the refusal stays after the refresh that follows it');
    results.push({ theme, width, rows: snapshot.sessions.length, outside: outside.length, overflow });
    await page.close();
  }
} finally {
  await browser.close();
}
console.log(JSON.stringify({ dir, results }, null, 2));
