// Headless UI verification of dragging Overview cards to reorder them.
// Native commands use synthetic session data.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
// TWAPP_ENGINE=webkit runs the engine twapp's window uses (WKWebView); chromium by default.
const engine = (playwright.default || playwright)[process.env.TWAPP_ENGINE || 'chromium'];
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/overview-drag.json', import.meta.url), 'utf8'));
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-overview-drag-'));
await fs.mkdir(dir, { recursive: true });
const keys = snapshot.sessions.map(s => s.key);
const k = name => `/work/${name}`;
const browser = await engine.launch({ headless: true });
const results = [];

const lane = page => page.locator('.overview-lane.lane-priority');
const card = (page, name) => lane(page).locator('.overview-card').filter({ hasText: snapshot.sessions.find(s => s.key === k(name)).name });
const order = page => lane(page).locator('.overview-card .overview-card-name').allTextContents();
const calls = (page, offset, cmd) => page.evaluate(([o, c]) => window.__calls.slice(o).filter(x => x.cmd === c), [offset, cmd]);
const mark = page => page.evaluate(() => window.__calls.length);
async function drag(page, from, to, side) {
  const box = await to.boundingBox();
  await from.dragTo(to, { targetPosition: { x: box.width * (side === 'before' ? 0.25 : 0.75), y: box.height / 2 } });
}

try {
  for (const theme of ['light', 'dark']) for (const width of [260, 340, 480]) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: theme });
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, width, snapshot }) => {
      localStorage.setItem('twapp-layout', JSON.stringify({ mode: 'right', sidebarWidth: width, switcherCollapsedLanes: [] }));
      localStorage.setItem('twapp-overview-folded-v2', JSON.stringify([]));
      window.__calls = [];
      window.__snapshot = snapshot;
      window.__callbacks = {};
      window.__listeners = {};
      window.__emit = (event, payload) => (window.__listeners[event] || []).forEach(h => window.__callbacks[h]?.({ event, id: 0, payload }));
      let counter = 0;
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: cb => { const id = ++counter; window.__callbacks[id] = cb; return id; },
        unregisterCallback: () => {},
        invoke: async (cmd, args) => {
          if (cmd === 'hub_snapshot') return JSON.parse(JSON.stringify(window.__snapshot));
          if (cmd === 'get_theme_preference') return theme;
          if (cmd === 'get_global_config') return { agent_providers: ['claude', 'codex', 'antigravity'] };
          if (cmd === 'get_notes' || cmd === 'get_session_history') return [];
          if (cmd === 'get_ticket') return null;
          if (cmd === 'get_font_family_preference') return 'monospace';
          if (cmd === 'plugin:app|version') return '0.0.0';
          if (cmd === 'plugin:event|listen') { (window.__listeners[args.event] ||= []).push(args.handler); return ++counter; }
          window.__calls.push({ cmd, args });
          return null;
        },
      };
    }, { theme, width, snapshot });
    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1420');
    await lane(page).locator('.overview-card').first().waitFor();

    // Cards are draggable; no card is wider than its grid.
    assert.equal(await lane(page).locator('.overview-card[draggable="true"]').count(), 5, 'every priority card is draggable');
    const overflow = await page.locator('.overview-grid').evaluateAll(grids => grids.flatMap(g =>
      [...g.children].filter(c => c.getBoundingClientRect().right > g.getBoundingClientRect().right + 1)).length);
    assert.equal(overflow, 0, 'no card is wider than its grid');

    // Mid-drag: the dragged card dims and the target shows a bar on the side it
    // will land. The drag is then abandoned over the lane header.
    let at = await mark(page);
    await card(page, 'p5').hover();
    await page.mouse.down();
    const target = await card(page, 'p2').boundingBox();
    await page.mouse.move(target.x + target.width * 0.3, target.y + target.height / 2, { steps: 8 });
    await page.mouse.move(target.x + target.width * 0.25, target.y + target.height / 2, { steps: 2 });
    await page.screenshot({ path: path.join(dir, `drag-${theme}-${width}.png`) });
    const midDrag = {
      dragging: await lane(page).locator('.overview-card.dragging').count(),
      dropBefore: await card(page, 'p2').evaluate(el => el.classList.contains('drop-before')),
    };
    const head = await lane(page).locator('.overview-lane-head').boundingBox();
    await page.mouse.move(head.x + head.width / 2, head.y + head.height / 2, { steps: 8 });
    await page.mouse.up();
    assert.deepEqual(await calls(page, at, 'hub_reorder'), [], 'an abandoned drag saves nothing');

    // Released on the very move that enters an element inside the target card
    // (WebKit sends only dragenter for that move, no dragover).
    at = await mark(page);
    await card(page, 'p5').hover();
    await page.mouse.down();
    const t = await card(page, 'p2').boundingBox();
    await page.mouse.move(t.x + t.width * 0.75, t.y + 4, { steps: 10 });
    for (let i = 1; i <= 3; i++) await page.mouse.move(t.x + t.width * 0.75 + i, t.y + 4, { steps: 1 });
    const nameBox = await card(page, 'p2').locator('.overview-card-name').boundingBox();
    await page.mouse.move(t.x + t.width * 0.75, nameBox.y + nameBox.height / 2, { steps: 1 });
    await page.mouse.up();
    assert.deepEqual((await calls(page, at, 'hub_reorder'))[0]?.args.keys,
      [k('p1'), k('p2'), k('p3'), k('p5'), k('p4'), k('b1'), k('b2')], 'a drop released on entering a child element lands');
    await page.reload();
    await lane(page).locator('.overview-card').first().waitFor();

    // Dragging a card over the sidebar is refused there and leaves no highlight.
    at = await mark(page);
    await card(page, 'p4').hover();
    await page.mouse.down();
    const row = await page.locator('.rail-row').nth(1).boundingBox();
    await page.mouse.move(row.x + row.width / 2, row.y + row.height / 2, { steps: 10 });
    await page.mouse.move(row.x + row.width / 2 + 2, row.y + row.height / 2, { steps: 2 });
    await page.mouse.move(400, 860, { steps: 10 });
    await page.mouse.up();
    assert.equal(await page.locator('.rail-row.drop-target, .lane.drop-target').count(), 0, 'the sidebar keeps no drop highlight');
    assert.deepEqual(await calls(page, at, 'hub_reorder'), [], 'a card dragged over the sidebar saves nothing');

    // Drop on the right half of a card in the same group: lands right after it.
    at = await mark(page);
    await drag(page, card(page, 'p5'), card(page, 'p2'), 'after');
    const expected = [k('p1'), k('p2'), k('p3'), k('p5'), k('p4'), k('b1'), k('b2')];
    assert.deepEqual(await calls(page, at, 'hub_reorder'), [{ cmd: 'hub_reorder', args: { keys: expected } }], 'drop after saves the new order');
    assert.deepEqual(await calls(page, at, 'hub_set_lane'), [], 'a drop never changes the lane');
    const names = Object.fromEntries(snapshot.sessions.map(s => [s.key, s.name]));
    assert.deepEqual(await order(page), [k('p1'), k('p3'), k('p2'), k('p5'), k('p4')].map(x => names[x]), 'cards re-render in the new order');

    // Drop on the left half of a card: lands right before it.
    at = await mark(page);
    await drag(page, card(page, 'p4'), card(page, 'p2'), 'before');
    assert.deepEqual((await calls(page, at, 'hub_reorder'))[0]?.args.keys,
      [k('p1'), k('p4'), k('p3'), k('p2'), k('p5'), k('b1'), k('b2')], 'drop before saves the new order');

    // Within an effort group.
    at = await mark(page);
    await drag(page, card(page, 'p3'), card(page, 'p1'), 'before');
    assert.deepEqual((await calls(page, at, 'hub_reorder'))[0]?.args.keys,
      [k('p3'), k('p4'), k('p1'), k('p2'), k('p5'), k('b1'), k('b2')], 'effort group members trade places in their own slots');

    // Across groups, across lanes, onto the lane header: nothing is saved and no drag styling stays.
    at = await mark(page);
    await drag(page, card(page, 'p1'), card(page, 'p4'), 'after');
    await card(page, 'p4').dragTo(page.locator('.overview-lane.lane-background .overview-card').first());
    await card(page, 'p4').dragTo(lane(page).locator('.overview-lane-head'));
    assert.deepEqual(await calls(page, at, 'hub_reorder'), [], 'refused drops save nothing');
    assert.equal(await page.locator('.overview-card.dragging, .overview-card.drop-before, .overview-card.drop-after').count(), 0, 'no drag styling remains');
    assert.equal(await lane(page).locator('.overview-card').count(), 5, 'the priority lane keeps its cards');

    // A groupmate moving to another lane mid-drag is no longer a valid target.
    at = await mark(page);
    await card(page, 'p5').hover();
    await page.mouse.down();
    const p4 = await card(page, 'p4').boundingBox();
    await page.mouse.move(p4.x + p4.width / 2, p4.y + p4.height / 2, { steps: 10 });
    await page.evaluate(() => {
      window.__snapshot.sessions.find(s => s.key === '/work/p2').lane = 'background';
      window.__emit('hub:changed', null);
    });
    const moved = page.locator('.overview-lane.lane-background .overview-card').filter({ hasText: 'A very long session name' });
    await moved.waitFor();
    const m = await moved.boundingBox();
    await page.mouse.move(m.x + m.width * 0.75, m.y + m.height / 2, { steps: 10 });
    await page.mouse.move(m.x + m.width * 0.75 + 2, m.y + m.height / 2, { steps: 2 });
    await page.mouse.up();
    assert.deepEqual(await calls(page, at, 'hub_set_lane'), [], 'a drop never changes the dragged card\'s lane');
    assert.deepEqual(await calls(page, at, 'hub_reorder'), [], 'a card that left the group is not a target');

    // The sidebar's own drags still reorder.
    at = await mark(page);
    await page.locator('.rail-row').nth(2).dragTo(page.locator('.rail-row').nth(0));
    assert.equal((await calls(page, at, 'hub_reorder')).length, 1, 'sidebar drags still reorder');

    // A plain click still opens the session.
    at = await mark(page);
    await card(page, 'p5').click();
    // Hub also re-sends the selection when the terminal shows (Hub.tsx's terminal effect).
    const selects = await calls(page, at, 'hub_select');
    assert.ok(selects.length > 0 && selects.every(c => c.args.key === k('p5')), 'click opens the session');
    assert.deepEqual(await calls(page, at, 'hub_reorder'), [], 'a click does not reorder');

    assert.equal(midDrag.dragging, 1, 'the dragged card dims');
    assert.ok(midDrag.dropBefore, 'the target shows the drop bar on its left');
    results.push({ theme, width, overflow, midDrag });
    await page.close();
  }
  await fs.writeFile(path.join(dir, 'results.json'), JSON.stringify(results, null, 2));
  console.log(JSON.stringify({ artifacts: dir, keys: keys.length, results }));
} finally {
  await browser.close();
}
