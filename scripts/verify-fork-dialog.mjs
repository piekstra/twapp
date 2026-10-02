// Run against Vite with Playwright installed; the native Tauri boundary is mocked.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.default || playwright;
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/fork-dialog.json', import.meta.url), 'utf8'));
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-fork-dialog-'));
await fs.mkdir(dir, { recursive: true });
const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (const theme of ['light', 'dark']) for (const width of [260, 340, 480]) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: theme });
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, width, snapshot }) => {
      localStorage.setItem('twapp-layout', JSON.stringify({ mode: 'right', sidebarWidth: width, switcherCollapsedLanes: [] }));
      window.__calls = [];
      let counter = 0;
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++counter,
        unregisterCallback: () => {},
        invoke: async (cmd, args) => {
          if (cmd === 'hub_snapshot') return snapshot;
          if (cmd === 'get_theme_preference') return theme;
          if (cmd === 'get_global_config') return { agent_providers: ['claude', 'codex', 'antigravity'] };
          if (cmd === 'get_notes' || cmd === 'get_session_history') return [];
          if (cmd === 'get_ticket') return null;
          if (cmd === 'get_font_family_preference') return 'monospace';
          if (cmd === 'plugin:app|version') return '0.0.0';
          if (cmd === 'fork_session') {
            window.__calls.push({ cmd, args });
            return 'Converted copy';
          }
          if (cmd === 'plugin:event|listen') return ++counter;
          return null;
        },
      };
    }, { theme, width, snapshot });
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1437');
    const dialog = page.locator('.fork-panel');
    const ticket = dialog.getByPlaceholder('Ticket, e.g. ABC-123');
    const name = dialog.getByPlaceholder('Name, e.g. refactor auth');
    async function open() {
      await page.getByRole('button', { name: 'Fork', exact: true }).click();
      await dialog.waitFor();
    }
    async function checkReset(provider) {
      assert.equal(await dialog.getByRole('combobox').inputValue(), provider);
      assert.equal(await ticket.inputValue(), '');
      assert.equal(await name.inputValue(), '');
    }
    for (const close of ['cancel', 'cross', 'overlay']) {
      await open();
      await checkReset('claude');
      await dialog.getByRole('combobox').selectOption('codex');
      await name.fill('Canceled conversion');
      await ticket.fill('example/project#2');
      if (close === 'cancel') await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
      if (close === 'cross') await dialog.locator('.config-close').click();
      if (close === 'overlay') await page.locator('.config-overlay').click({ position: { x: 5, y: 5 } });
      await dialog.waitFor({ state: 'hidden' });
      await open();
      await checkReset('claude');
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    }
    // A canceled target must not carry into a different source session.
    await open();
    await dialog.getByRole('combobox').selectOption('antigravity');
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await page.locator('.rail-row').filter({ hasText: 'Other conversation' }).click();
    await open();
    assert.equal(await dialog.locator('.config-title').textContent(), 'Fork Other conversation');
    await checkReset('codex');
    await dialog.getByRole('combobox').selectOption('claude');
    await name.fill('Claude continuation');
    await page.screenshot({ path: path.join(dir, `fork-${theme}-${width}.png`) });
    const overflow = await dialog.evaluate(el => [...el.querySelectorAll('input,select,button')]
      .filter(e => e.getBoundingClientRect().right > el.getBoundingClientRect().right + 1).length);
    assert.equal(overflow, 0);
    await dialog.getByRole('button', { name: 'Fork and convert', exact: true }).click();
    await dialog.waitFor({ state: 'hidden' });
    assert.deepEqual(await page.evaluate(() => window.__calls), [{ cmd: 'fork_session', args: {
      directory: '/work/other', ticketKey: null, name: 'Claude continuation', provider: 'claude',
    } }]);
    await open();
    await checkReset('codex');
    results.push({ theme, width, cancellationPaths: 3, switchedSource: true, successReset: true, overflow });
    await page.close();
  }
  await fs.writeFile(path.join(dir, 'results.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify({ artifacts: dir, results }));
} finally {
  await browser.close();
}
