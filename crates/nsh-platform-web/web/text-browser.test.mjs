import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import test from 'node:test';

let chromium;
try { execFileSync('chromium', ['--version'], { stdio: 'ignore' }); chromium = 'chromium'; } catch {}

test('tabular digits are measured as wide as the DOM draws them', { skip: !chromium }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'nsh-text-browser-'));
  try {
    const source = await readFile(new URL('./nsh.js', import.meta.url), 'utf8');
    const module = 'data:text/javascript;base64,' + Buffer.from(source + '\nexport { measureNumericText };').toString('base64');
    await writeFile(join(dir, 'index.html'), `<!doctype html><style>body { margin:0; }</style>
    <script type="module">
      import { measureNumericText } from '${module}';
      try {
        const font = '400 20px -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
        // A run as the mounter draws it: its class's font, then the variant.
        const drawn = (text) => {
          const run = document.createElement('span');
          run.style.cssText = 'position:absolute;white-space:pre;font:' + font + ';font-variant-numeric:tabular-nums';
          run.textContent = text;
          document.body.append(run);
          const width = run.getBoundingClientRect().width;
          run.remove();
          return width;
        };
        for (const text of ['530.92', '111.11', '1,234.56', 'Price']) {
          const measured = measureNumericText(font, 'tabular-nums', text);
          if (Math.abs(measured - drawn(text)) > 0.01) throw Error(text + ': measured ' + measured + ', drawn ' + drawn(text));
        }
        document.body.dataset.result = 'passed';
      } catch (e) { document.body.dataset.result = 'failed: ' + e.message; }
    </script>`);
    const html = execFileSync(chromium, ['--headless', '--no-sandbox', '--disable-dev-shm-usage',
      '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--virtual-time-budget=1000',
      '--user-data-dir=' + join(dir, 'profile'), '--dump-dom', pathToFileURL(join(dir, 'index.html')).href],
      { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 20000, maxBuffer: 1024 * 1024 });
    assert.equal(html.match(/data-result="([^"]*)"/)?.[1], 'passed');
  } finally { await rm(dir, { recursive: true, force: true }); }
});
