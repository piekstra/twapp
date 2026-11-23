// Headless checks of a session terminal stopped and started outside the window
// (`twapp suspend`, `twapp wake`); native commands and hub events are synthetic.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
const playwright = await import(process.env.PLAYWRIGHT_MODULE || 'playwright');
const { chromium } = playwright.default || playwright;
const snapshot = JSON.parse(await fs.readFile(new URL('./fixtures/suspend-wake.json', import.meta.url), 'utf8'));
const dir = process.env.TWAPP_ARTIFACT_DIR || await fs.mkdtemp(path.join(os.tmpdir(), 'twapp-suspend-wake-ui-'));
await fs.mkdir(dir, { recursive: true });
const SUSPENDED = '[Suspended. Press Enter to resume.]';
const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (const theme of ['light', 'dark']) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 }, colorScheme: theme });
    await page.route('https://api.github.com/**', r => r.abort());
    await page.addInitScript(({ theme, snapshot }) => {
      // Without WebGL the terminal draws with DOM rows, whose text can be read.
      const getContext = HTMLCanvasElement.prototype.getContext;
      HTMLCanvasElement.prototype.getContext = function (type, ...rest) {
        return String(type).startsWith('webgl') ? null : getContext.call(this, type, ...rest);
      };
      window.__starts = [];
      window.__writes = [];
      const callbacks = {};
      const listeners = [];
      let counter = 0;
      window.__emit = (event, payload) =>
        listeners.filter(l => l.event === event).forEach(l => callbacks[l.handler]?.({ event, id: 0, payload }));
      // Dev mode mounts effects twice; a listener left behind would hear each event twice.
      window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
        unregisterListener: (event, eventId) => {
          const at = listeners.findIndex(l => l.event === event && l.eventId === eventId);
          if (at >= 0) listeners.splice(at, 1);
        },
      };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: (cb) => { callbacks[++counter] = cb; return counter; },
        unregisterCallback: () => {},
        invoke: async (cmd, args) => {
          if (cmd === 'hub_snapshot') return snapshot;
          if (cmd === 'hub_session_disk') return { running: false, sessions: [] };
          if (cmd === 'hub_outside_agents') return [];
          if (cmd === 'get_theme_preference') return theme;
          if (cmd === 'get_global_config') return { agent_providers: ['claude'] };
          if (cmd === 'get_notes' || cmd === 'get_session_history') return [];
          if (cmd === 'get_ticket') return null;
          if (cmd === 'get_font_family_preference') return 'monospace';
          if (cmd === 'plugin:app|version') return '0.0.0';
          if (cmd === 'plugin:event|listen') {
            const eventId = ++counter;
            listeners.push({ event: args.event, handler: args.handler, eventId });
            return eventId;
          }
          if (cmd === 'plugin:event|unlisten') return null;
          if (cmd === 'hub_write') window.__writes.push(args);
          if (cmd === 'hub_start') {
            // Each start answers on its channel, so the screen shows which start it came from.
            const n = window.__starts.push(args.key);
            const text = Array.from(new TextEncoder().encode(`start ${n} of ${args.key}\r\n`));
            setTimeout(() => callbacks[args.channel.id]?.({ index: 0, message: text }), 20);
          }
          return null;
        },
      };
    }, { theme, snapshot });

    const starts = () => page.evaluate(() => window.__starts.slice());
    const screen = () => page.evaluate(() =>
      [...document.querySelectorAll('.xterm-rows')].filter(e => e.offsetParent !== null).map(e => e.innerText).join('\n'));
    const waitScreen = (text) => page.waitForFunction((text) =>
      [...document.querySelectorAll('.xterm-rows')].some(e => e.offsetParent !== null && e.innerText.includes(text)), text);
    const emit = (event, key) => page.evaluate(([event, key]) => window.__emit(event, key), [event, key]);
    const pressEnter = async () => {
      await page.evaluate(() => [...document.querySelectorAll('.xterm-helper-textarea')].find(e => e.closest('.xterm')?.offsetParent !== null)?.focus());
      await page.keyboard.press('Enter');
    };
    const open = (name) => page.locator('.rail-row', { hasText: name }).click();
    const settle = () => page.evaluate(() => new Promise(r => setTimeout(r, 300)));

    await page.goto(process.env.TWAPP_URL || 'http://127.0.0.1:1420');
    await waitScreen('start 1 of /work/alpha');

    // On screen: suspended keeps the text and offers Enter; Enter resumes without typing into the agent.
    await emit('hub:suspended', '/work/alpha');
    await waitScreen(SUSPENDED);
    assert.match(await screen(), /start 1 of \/work\/alpha[\s\S]*\[Suspended\. Press Enter to resume\.\]/, 'the text before the suspend stays');
    assert.equal((await screen()).split(SUSPENDED).length - 1, 1, 'one line per suspend');
    await page.screenshot({ path: path.join(dir, `suspended-${theme}.png`) });
    await pressEnter();
    await waitScreen('start 2 of /work/alpha');
    assert.deepEqual(await page.evaluate(() => window.__writes), [], 'the Enter that resumes is not sent to the agent');

    // On screen: a wake attaches at once, with no key pressed.
    await emit('hub:suspended', '/work/alpha');
    await waitScreen(SUSPENDED);
    await emit('hub:woken', '/work/alpha');
    await waitScreen('start 3 of /work/alpha');

    // A wake for a terminal that is already attached starts nothing.
    await emit('hub:woken', '/work/alpha');
    await settle();
    assert.equal((await starts()).length, 3);

    // Off screen: suspended clears the terminal, and showing it starts it again.
    await open('Beta cleanup');
    await waitScreen('start 4 of /work/beta');
    await emit('hub:suspended', '/work/alpha');
    await settle();
    assert.equal((await starts()).length, 4, 'nothing starts while the terminal is off screen');
    await open('Alpha research');
    await waitScreen('start 5 of /work/alpha');
    assert.doesNotMatch(await screen(), /start 3|Suspended/, 'the suspended screen was cleared');

    // Off screen: a wake waits for the terminal to be shown, which attaches it.
    await open('Beta cleanup');
    await emit('hub:suspended', '/work/alpha');
    await emit('hub:woken', '/work/alpha');
    await settle();
    assert.equal((await starts()).length, 5);
    await open('Alpha research');
    await waitScreen('start 6 of /work/alpha');
    assert.deepEqual(await starts(), ['/work/alpha', '/work/alpha', '/work/alpha', '/work/beta', '/work/alpha', '/work/alpha']);
    results.push({ theme, starts: (await starts()).length });
    await page.close();
  }
} finally {
  await browser.close();
}
console.log(JSON.stringify({ dir, results }, null, 2));
