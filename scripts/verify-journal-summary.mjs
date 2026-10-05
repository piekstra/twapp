// Headless journal UI checks; native commands use synthetic saved entries.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.default || playwright;
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/fork-dialog.json', import.meta.url), 'utf8'));
const day = JSON.parse(await fs.readFile(new URL('../src-tauri/tests/fixtures/journal/day-bullets.json', import.meta.url), 'utf8'));
const savedPeriod = JSON.parse(await fs.readFile(new URL('../src-tauri/tests/fixtures/journal/period-legacy.json', import.meta.url), 'utf8'));
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-journal-ui-'));
await fs.mkdir(dir, { recursive: true });
const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (const theme of ['light', 'dark']) for (const width of [900, 1280]) {
    const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme: theme });
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, snapshot, day, savedPeriod }) => {
      localStorage.setItem('twapp-layout', JSON.stringify({ mode: 'right', sidebarWidth: 340 }));
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
          if (cmd === 'hub_journal_days') return [{ day: day.day, headline: day.digest.headline, sessions: 2, complete: true, pending: false }];
          if (cmd === 'hub_journal_day') {
            window.__calls.push({ cmd, args });
            return { record: day, path: '/work/journal/day.md' };
          }
          if (cmd === 'hub_journal_period') {
            window.__calls.push({ cmd, args });
            const kind = args.id === 'week' || args.id.includes('-W') ? 'week'
              : args.id === 'month' || /^\d{4}-\d{2}$/.test(args.id) ? 'month' : 'year';
            const id = kind === 'week' ? '2026-W40' : kind === 'month' ? '2026-09' : '2026';
            const record = { ...savedPeriod, kind, id, digest: day.digest };
            return { id, label: kind, previous: 'previous', next: null, record, path: '/work/journal/period.md' };
          }
          return null;
        },
      };
    }, { theme, snapshot, day, savedPeriod });
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1420');
    await page.locator('.rail-home').click();
    await page.getByRole('button', { name: 'Journal', exact: true }).click();
    await page.locator('.journal-headline').waitFor();
    await page.screenshot({ path: path.join(dir, `day-${theme}-${width}.png`) });
    if (process.env.TWAPP_BEFORE !== '1') {
      const format = page.getByRole('group', { name: 'Summary format' });
      const bullets = format.getByRole('button', { name: 'Bullets', exact: true });
      const paragraph = format.getByRole('button', { name: 'Paragraph', exact: true });
      assert.equal(await bullets.getAttribute('aria-pressed'), 'true');
      assert.deepEqual(await page.locator('.journal-bullets li').allTextContents(), day.digest.bullets);
      assert.equal(await page.locator('.journal-overview').count(), 0);
      assert.equal(await page.locator('.journal-efforts').isVisible(), false);
      await page.getByText('By effort', { exact: true }).click();
      assert.equal(await page.locator('.journal-efforts').isVisible(), true);
      await page.getByText('Recorded activity', { exact: true }).click();
      assert.equal(await page.locator('.journal-sessions').isVisible(), true);
      const calls = await page.evaluate(() => window.__calls.length);
      await bullets.focus();
      await page.keyboard.press('Tab');
      assert.equal(await paragraph.evaluate(el => el === document.activeElement), true);
      await page.keyboard.press('Space');
      assert.equal(await paragraph.getAttribute('aria-pressed'), 'true');
      assert.equal(await page.locator('.journal-overview').textContent(), day.digest.overview);
      assert.equal(await page.locator('.journal-bullets').count(), 0);
      await page.screenshot({ path: path.join(dir, `paragraph-${theme}-${width}.png`) });
      await page.keyboard.press('Shift+Tab');
      assert.equal(await bullets.evaluate(el => el === document.activeElement), true);
      await page.keyboard.press('Enter');
      assert.equal(await bullets.getAttribute('aria-pressed'), 'true');
      assert.equal(await page.evaluate(() => window.__calls.length), calls, 'format switching must not request a rewrite');
      for (const scope of ['Week', 'Month', 'Year']) {
        await page.getByRole('radiogroup', { name: 'Scope' }).getByRole('radio', { name: scope, exact: true }).click();
        await page.locator('.journal-period .journal-bullets').waitFor();
        assert.deepEqual(await page.locator('.journal-bullets li').allTextContents(), day.digest.bullets);
        await paragraph.click();
        assert.equal(await page.locator('.journal-overview').textContent(), day.digest.overview);
        await bullets.click();
      }
      await page.screenshot({ path: path.join(dir, `year-${theme}-${width}.png`) });
      const overflow = await page.locator('.journal').evaluate(el => [...el.querySelectorAll('.journal-controls button')]
        .filter(b => b.getBoundingClientRect().right > el.getBoundingClientRect().right + 1).length);
      assert.equal(overflow, 0);
      results.push({ theme, width, scopes: 4, keyboardFormatSwitch: true, formatSwitchWithoutRewrite: true, overflow });
    } else results.push({ theme, width, before: true });
    await page.close();
  }
  await fs.writeFile(path.join(dir, 'results.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify({ artifacts: dir, results }));
} finally {
  await browser.close();
}
