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
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1420');
    const lane = process.env.TWAPP_BEFORE === '1'
      ? page.getByRole('radiogroup', { name: 'Lane' })
      : page.getByRole('group', { name: 'Session category' });
    await lane.waitFor();
    const overflow = await lane.evaluate(el => [...el.querySelectorAll('button')]
      .filter(e => e.getBoundingClientRect().right > el.getBoundingClientRect().right + 1).length);
    assert.equal(overflow, 0);
    await page.screenshot({ path: path.join(dir, `controls-${theme}-${width}.png`) });
    let labelContrast = null;
    if (process.env.TWAPP_BEFORE !== '1') {
      assert.equal(await page.locator('.panel-lane-label').textContent(), 'This session is:');
      labelContrast = await page.locator('.panel-lane-label').evaluate(el => {
        const luminance = color => {
          const channels = color.match(/[\d.]+/g).slice(0, 3).map(Number).map(value => {
            const channel = value / 255;
            return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
          });
          return channels[0] * 0.2126 + channels[1] * 0.7152 + channels[2] * 0.0722;
        };
        let surface = el;
        while (surface.parentElement && ['transparent', 'rgba(0, 0, 0, 0)'].includes(getComputedStyle(surface).backgroundColor)) {
          surface = surface.parentElement;
        }
        const foreground = luminance(getComputedStyle(el).color);
        const background = luminance(getComputedStyle(surface).backgroundColor);
        return (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05);
      });
      assert.ok(labelContrast >= 4.5, `category label contrast is ${labelContrast}:1`);
      assert.equal(await lane.locator('.lane-dot').count(), 4, 'all category icons remain');
      assert.doesNotMatch(await lane.innerText(), /→/, 'category assignment uses the label without arrows');
      const rowCount = await lane.evaluate(el => new Set([...el.querySelectorAll('button')]
        .map(button => Math.round(button.getBoundingClientRect().top))).size);
      assert.equal(rowCount, width === 260 ? 2 : 1, 'compact controls wrap only at narrow widths');
      const fill = await lane.evaluate(el => {
        const panel = el.closest('.panel-lane');
        const panelStyle = getComputedStyle(panel);
        const available = panel.getBoundingClientRect().width - parseFloat(panelStyle.paddingLeft) - parseFloat(panelStyle.paddingRight);
        const bounds = el.getBoundingClientRect();
        const style = getComputedStyle(el);
        const rows = new Map();
        for (const button of el.querySelectorAll('button')) {
          const rect = button.getBoundingClientRect();
          const top = Math.round(rect.top);
          const row = rows.get(top) || { left: rect.left, right: rect.right };
          row.left = Math.min(row.left, rect.left);
          row.right = Math.max(row.right, rect.right);
          rows.set(top, row);
        }
        return {
          unused: available - bounds.width,
          rowGaps: [...rows.values()].map(row => Math.max(
            Math.abs(row.left - bounds.left - parseFloat(style.paddingLeft)),
            Math.abs(bounds.right - parseFloat(style.paddingRight) - row.right),
          )),
        };
      });
      assert.ok(Math.abs(fill.unused) <= 1, 'category control fills the panel content width');
      assert.ok(fill.rowGaps.every(gap => gap <= 1), 'buttons fill each row from left to right');
      const retained = page.locator('.lane-parked .rail-row');
      assert.equal(await retained.count(), 0, 'new lane starts folded for existing layouts');
      await page.locator('.lane-parked .lane-head').click();
      assert.equal(await retained.count(), 1);
      await page.reload();
      await lane.waitFor();
      assert.equal(await retained.count(), 1, 'unfolding survives reload');
      const beforeParking = await page.evaluate(() => window.__calls.length);
      await lane.getByRole('button', { name: 'Move this session to Parked', exact: true }).click();
      assert.equal(await lane.getByRole('button', { name: 'Current category: Parked', exact: true }).getAttribute('aria-pressed'), 'true');
      assert.equal(await page.locator('.rail-attention-count').count(), 0);
      // Xterm may fit asynchronously after reload; resizing does not stop a PTY.
      assert.deepEqual(await page.evaluate(offset => window.__calls.slice(offset).filter(c => c.cmd !== 'hub_resize'), beforeParking), [
        { cmd: 'hub_set_lane', args: { key: '/work/source', lane: 'parked' } },
      ]);
      await page.screenshot({ path: path.join(dir, `parked-${theme}-${width}.png`) });
      for (const [label, value] of [['Background', 'background'], ['Blocked', 'blocked'], ['Priority', 'priority'], ['Parked', 'parked']]) {
        const offset = await page.evaluate(() => window.__calls.length);
        const button = lane.getByRole('button', { name: `Move this session to ${label}`, exact: true });
        if (value === 'background') {
          await button.focus();
          await button.press('Enter');
        } else await button.click();
        assert.equal(await lane.getByRole('button', { name: `Current category: ${label}`, exact: true }).getAttribute('aria-pressed'), 'true');
        assert.equal(await lane.locator('.segment.active .lane-dot').count(), 1);
        assert.deepEqual(await page.evaluate(start => window.__calls.slice(start).filter(c => c.cmd !== 'hub_resize'), offset), [
          { cmd: 'hub_set_lane', args: { key: '/work/source', lane: value } },
        ], 'one activation changes only the current session category');
      }
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
    results.push({ theme, width, overflow, labelContrast, parkedInteraction: process.env.TWAPP_BEFORE !== '1' });
    await page.close();
  }
  await fs.writeFile(path.join(dir, 'results.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify({ artifacts: dir, results }));
} finally {
  await browser.close();
}
