// Headless UI verification of watched blockers in the session panel, with
// synthetic session data. Run against `npm run dev`.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.default || playwright;
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/watched-blockers.json', import.meta.url), 'utf8'));
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-watched-ui-'));
await fs.mkdir(dir, { recursive: true });
const browser = await chromium.launch({ headless: true });
try {
  for (const theme of ['light', 'dark']) for (const width of [260, 340, 480]) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 1100 }, colorScheme: theme });
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, width, snapshot }) => {
      localStorage.setItem('twapp-layout', JSON.stringify({ mode: 'right', sidebarWidth: width, switcherCollapsedLanes: [] }));
      window.__THEME = theme;
      window.__calls = [];
      let counter = 0;
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => ++counter,
        unregisterCallback: () => {},
        invoke: async (cmd, args) => {
          if (cmd === 'hub_snapshot') return snapshot;
          if (cmd === 'get_theme_preference') return theme;
          if (cmd === 'get_global_config') return { agent_providers: ['claude', 'codex'] };
          if (cmd === 'get_notes' || cmd === 'get_session_history') return [];
          if (cmd === 'get_ticket') return null;
          if (cmd === 'get_font_family_preference') return 'monospace';
          if (cmd === 'plugin:app|version') return '0.0.0';
          if (cmd === 'plugin:event|listen') return ++counter;
          window.__calls.push({ cmd, args });
          if (cmd === 'hub_watch_poll') return true;
          return null;
        },
      };
    }, { theme, width, snapshot });
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1420');
    // The panel's "Waiting on" row opens the session's details on its blockers.
    const waiting = page.locator('.panel-row', { hasText: 'Waiting on' });
    await waiting.waitFor();
    assert.match(await waiting.innerText(), /updated/i);
    await waiting.click();
    const rows = page.locator('.blocker-row');
    await rows.first().waitFor();
    assert.equal(await rows.count(), 4);
    const text = await page.locator('.blocker-list').first().innerText();
    assert.match(text, /vendor-case:CASE-4411/);
    assert.match(text, /not polled on its own until you allow vendor-case/);
    assert.match(text, /status Open -> Waiting on customer/);
    assert.match(text, /Watching merge-request:group\/repo!275: opened/);
    assert.match(text, /the last poll failed/);
    assert.match(text, /no source tracker is installed/);
    const wide = await page.evaluate(() => [...document.querySelectorAll('.blocker-row *')]
      .filter(el => {
        const row = el.closest('.blocker-row').getBoundingClientRect();
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.right > row.right + 1;
      })
      .map(el => `${el.tagName}.${el.className}: ${el.textContent.slice(0, 40)}`));
    assert.deepEqual(wide, [], 'nothing wider than its blocker card');
    await page.screenshot({ path: path.join(dir, `watched-${theme}-${width}.png`), fullPage: true });

    // An unapproved source shows its command before anything runs.
    const first = rows.filter({ hasText: 'Vendor to confirm which token scope' });
    await first.getByRole('button', { name: 'Check now' }).click();
    const ask = first.locator('.check-ask');
    await ask.waitFor();
    assert.match(await ask.innerText(), /vendor-cli cases get \{id\}/);
    await page.screenshot({ path: path.join(dir, `ask-${theme}-${width}.png`), fullPage: true });
    const offset = await page.evaluate(() => window.__calls.length);
    await ask.getByRole('button', { name: 'Allow vendor-case' }).click();
    await first.getByText('Checked just now: the item changed').waitFor();
    const calls = await page.evaluate(o => window.__calls.slice(o).filter(c => c.cmd.startsWith('hub_watch')), offset);
    assert.deepEqual(calls, [
      { cmd: 'hub_watch_approve', args: { source: 'vendor-case', digest: 'a'.repeat(64) } },
      { cmd: 'hub_watch_poll', args: { topic: 'vendor-case:CASE-4411' } },
    ]);
    // An approved source polls at once; one with no source has no button.
    const second = rows.filter({ hasText: 'Review of the merge request' });
    await second.getByRole('button', { name: 'Check now' }).click();
    await second.getByText('Checked just now: the item changed').waitFor();
    assert.equal(await rows.filter({ hasText: 'Ticket in a tracker' }).getByRole('button', { name: 'Check now' }).count(), 0);
    await page.close();
  }
  console.log(`ok: screenshots in ${dir}`);
} finally {
  await browser.close();
}
