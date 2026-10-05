// Headless UI verification. Native commands use synthetic session data.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.default || playwright;
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/parked-lane.json', import.meta.url), 'utf8'));
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-parked-ui-'));
await fs.mkdir(dir, { recursive: true });
const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (const theme of ['light', 'dark']) for (const width of [260, 340, 480]) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: theme });
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, width, snapshot }) => {
      if (!localStorage.getItem('twapp-layout')) {
        localStorage.setItem('twapp-layout', JSON.stringify({ mode: 'right', sidebarWidth: width, switcherCollapsedLanes: [] }));
        localStorage.setItem('twapp-overview-folded', JSON.stringify(['background']));
      }
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
          if (cmd === 'plugin:event|listen') return ++counter;
          window.__calls.push({ cmd, args });
          return null;
        },
      };
    }, { theme, width, snapshot });
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1422');
    const lane = page.getByRole('radiogroup', { name: 'Lane' });
    await lane.waitFor();
    const overflow = await lane.evaluate(el => [...el.querySelectorAll('button')]
      .filter(e => e.getBoundingClientRect().right > el.getBoundingClientRect().right + 1).length);
    assert.equal(overflow, 0);
    await page.screenshot({ path: path.join(dir, `controls-${theme}-${width}.png`) });
    if (process.env.TWAPP_BEFORE !== '1') {
      const retained = page.locator('.lane-parked .rail-row');
      assert.equal(await retained.count(), 0, 'new lane starts folded for existing layouts');
      await page.locator('.lane-parked .lane-head').click();
      assert.equal(await retained.count(), 1);
      await page.reload();
      await lane.waitFor();
      assert.equal(await retained.count(), 1, 'unfolding survives reload');
      await lane.getByRole('radio', { name: 'Parked', exact: true }).click();
      assert.equal(await lane.getByRole('radio', { name: 'Parked', exact: true }).getAttribute('aria-checked'), 'true');
      assert.equal(await page.locator('.rail-attention-count').count(), 0);
      assert.deepEqual(await page.evaluate(() => window.__calls.filter(c => c.cmd === 'hub_set_lane')), [
        { cmd: 'hub_set_lane', args: { key: '/work/source', lane: 'parked' } },
      ]);
      assert.equal(await page.evaluate(() => window.__calls.filter(c => /close_session|suspend|restart_session|delete_session/.test(c.cmd)).length), 0);
      await page.screenshot({ path: path.join(dir, `parked-${theme}-${width}.png`) });
      await page.locator('.rail-home').click();
      const heading = page.locator('.overview-lane-head').filter({ hasText: 'Parked' });
      await heading.waitFor();
      const section = heading.locator('..');
      assert.equal(await section.locator('.overview-card').count(), 0);
      await heading.click();
      assert.equal(await section.locator('.overview-card').count(), 2);
      assert.equal(await section.locator('.overview-card.attention').count(), 0);
      await heading.click();
      await page.locator('.overview-return').click();
      await page.locator('.rail-home').click();
      assert.equal(await section.locator('.overview-card').count(), 0, 'overview folding survives reopening');
      await heading.click();
      await page.screenshot({ path: path.join(dir, `overview-${theme}-${width}.png`) });
    }
    results.push({ theme, width, overflow, parkedInteraction: process.env.TWAPP_BEFORE !== '1' });
    await page.close();
  }
  await fs.writeFile(path.join(dir, 'results.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify({ artifacts: dir, results }));
} finally {
  await browser.close();
}
