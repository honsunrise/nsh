import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import test from "node:test";

let chromium;
try { execFileSync("chromium", ["--version"], { stdio: "ignore" }); chromium = "chromium"; } catch {}

test("real browser key events carry their own key and code, and a captured key prevents the default", { skip: !chromium }, async () => {
  const dir = await mkdtemp(join(tmpdir(), "nsh-keyboard-browser-"));
  try {
    const source = await readFile(new URL('./nsh.js', import.meta.url), 'utf8');
    const module = 'data:text/javascript;base64,' + Buffer.from(source + '\nexport { Player };').toString('base64');
    await writeFile(join(dir, 'index.html'), `<!doctype html><div id="root"><div id="one" tabindex="0"></div><div id="two" tabindex="0"></div></div>
    <script type="module">
      import { Player } from '${module}';
      try {
        const root = document.querySelector('#root');
        const one = document.querySelector('#one'), two = document.querySelector('#two');
        root._id = 1; one._id = 7; two._id = 8;
        root._events = new Set();
        for (const el of [one, two]) { el._events = new Set(['onKeyDown', 'onKeyUp']); el._props = {}; }
        // What the core answers: Escape is captured, everything else is not.
        const got = [];
        let last = false;
        const player = Object.create(Player.prototype);
        player.root = root;
        player.modifiersChanged = () => {};
        player.wasm = {
          nsh_key: () => -1,
          nsh_key_event(id, down, key, keyLen, text, textLen, physical, physicalLen, modifiers, repeat) {
            got.push({ id, down, key, text, physical, modifiers, repeat });
            last = key === 'Escape';
            return -1;
          },
          nsh_key_captured: () => (last ? 1 : 0),
        };
        player.call = (f) => f(player.wasm);
        player.withStrings = (strings, f) => f(...strings.flatMap((s) => [s, s.length]));
        player.installInput();
        one.focus();
        const send = (target, type, init) => {
          const event = new KeyboardEvent(type, { bubbles: true, cancelable: true, ...init });
          return target.dispatchEvent(event);
        };
        const allowed = send(one, 'keydown', { key: 'A', code: 'KeyA', shiftKey: true });
        const captured = send(one, 'keydown', { key: 'Escape', code: 'Escape' });
        send(one, 'keyup', { key: 'A', code: 'KeyA', shiftKey: true });
        two.focus();
        send(two, 'keyup', { key: 'ArrowLeft', code: 'ArrowLeft' });
        const want = [
          { id: 7, down: 1, key: 'A', text: 'A', physical: 'KeyA', modifiers: 4, repeat: 0 },
          { id: 7, down: 1, key: 'Escape', text: '', physical: 'Escape', modifiers: 0, repeat: 0 },
          { id: 7, down: 0, key: 'A', text: '', physical: 'KeyA', modifiers: 4, repeat: 0 },
          // A release follows the focus the browser gives it, not its press.
          { id: 8, down: 0, key: 'ArrowLeft', text: '', physical: 'ArrowLeft', modifiers: 0, repeat: 0 },
        ];
        if (JSON.stringify(got) !== JSON.stringify(want)) throw Error('keys ' + JSON.stringify(got));
        if (allowed !== true) throw Error('an uncaptured key had its default prevented');
        if (captured !== false) throw Error('a captured key ran the browser default');
        document.body.dataset.result = 'passed';
      } catch (e) { document.body.dataset.result = 'failed: ' + e.message; }
    </script>`);
    const html = execFileSync(chromium, ['--headless', '--no-sandbox', '--disable-dev-shm-usage',
      '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--virtual-time-budget=1000',
      '--user-data-dir=' + join(dir, 'profile'), '--dump-dom', pathToFileURL(join(dir, 'index.html')).href],
      { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 20000, maxBuffer: 1024 * 1024 });
    assert.equal(html.match(/data-result="([^"]*)"/)?.[1], "passed");
  } finally { await rm(dir, { recursive: true, force: true }); }
});
