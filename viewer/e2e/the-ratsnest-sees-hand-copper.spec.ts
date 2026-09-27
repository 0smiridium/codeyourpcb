import { test, expect, type Page } from '@playwright/test';
import { execFileSync } from 'child_process';
import fs from 'fs';
import os from 'os';
import path from 'path';
import { fileURLToPath } from 'url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(__dirname, '../..');

/**
 * The ratsnest counts copper already on the board as a connection.
 *
 * Three pads on one net: a trace joins R1.2 and C1.1, and R2.1 has nothing.
 * The viewer used to skip the ratsnest of every net with a single trace on
 * it, so this board showed no line at all while `cypcb check` reported R2.1
 * unrouted. The engine now draws one line for each connection the checker
 * finds missing, from the same pieces of copper the router reads.
 *
 * A `.ses` file took a second road: the viewer parsed it in TypeScript, kept
 * the traces in its own copy of the board and drew its own ratsnest over
 * them, and the checker never saw that copper. It now goes into the engine
 * and from there into the design text, so it has to show what `check` says
 * about that text.
 */
const HAND = fs.readFileSync(
  path.join(REPO, 'crates/cypcb-render/tests/fixtures/hand-copper.cypcb'),
  'utf-8',
);
const BARE = HAND.replace(/trace VCC \{[\s\S]*?\n\}\n/, '');

/** Tenth-mils, the unit of `(resolution mil 10)`. */
const tenthMil = (mm: number): string => ((mm * 1_000_000) / 2540).toFixed(4);

/** A session file routing R1.2 to C1.1 on the top layer, as FreeRouting writes one. */
const SES = `(session "hand_copper"
  (routes
    (resolution mil 10)
    (network_out
      (net "VCC"
        (wire (path F.Cu ${tenthMil(0.3)} ${tenthMil(5.5)} ${tenthMil(10)} ${tenthMil(19.5)} ${tenthMil(10)}))
      )
    )
  )
)
`;

interface Line { start_x: number; start_y: number; end_x: number; end_y: number }
interface Row { kind: string; message: string; x_nm: number; y_nm: number }

async function ready(page: Page): Promise<void> {
  await page.goto('/');
  await expect(page.locator('#status-text')).toContainText('Ready', { timeout: 15_000 });
}

async function load(page: Page, source: string): Promise<void> {
  await page.evaluate((src) => (window as any).__loadBoard(src, 'cypcb'), source);
  await page.waitForTimeout(300);
}

async function seen(page: Page): Promise<{ traces: number; lines: string[]; rows: string[] }> {
  const snapshot = await page.evaluate(() => {
    const s = (window as any).__pcbEngine.get_snapshot();
    return { traces: s.traces.length, ratsnest: s.ratsnest as Line[], violations: s.violations as Row[] };
  });
  return {
    traces: snapshot.traces,
    lines: snapshot.ratsnest.map(lineInMm).sort(),
    rows: snapshot.violations.map((v) => row(v.kind, v.message, v.x_nm / 1e6, v.y_nm / 1e6)).sort(),
  };
}

/** A line as its two ends in mm, the lower end first. */
function lineInMm(line: Line): string {
  const ends = [
    `${(line.start_x / 1e6).toFixed(3)},${(line.start_y / 1e6).toFixed(3)}`,
    `${(line.end_x / 1e6).toFixed(3)},${(line.end_y / 1e6).toFixed(3)}`,
  ].sort();
  return ends.join(' - ');
}

function row(kind: string, message: string, x_mm: number, y_mm: number): string {
  return `${kind} | ${message} | ${x_mm.toFixed(3)},${y_mm.toFixed(3)}`;
}

/** What `cypcb check -o json` reports about a design, as the same rows. */
function checkRows(design: string): string[] {
  const binary = path.join(process.env.CARGO_TARGET_DIR ?? path.join(REPO, 'target'), 'debug', 'cypcb');
  expect(fs.existsSync(binary), `the checker is built at ${binary}: run cargo build -p cypcb-cli`).toBe(true);
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ratsnest-'));
  const file = path.join(dir, 'board.cypcb');
  fs.writeFileSync(file, design);
  let out: string;
  try {
    out = execFileSync(binary, ['check', '-o', 'json', file], { encoding: 'utf-8', stdio: ['ignore', 'pipe', 'ignore'] });
  } catch (error: any) {
    // A board with violations exits non-zero and still prints the report.
    out = error.stdout;
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
  const report = JSON.parse(out) as { violations: { kind: string; message: string; x_mm: number; y_mm: number }[] };
  return report.violations.map((v) => row(v.kind, v.message, v.x_mm, v.y_mm)).sort();
}

test('a pad the hand trace does not reach has the one line', async ({ page }) => {
  await ready(page);
  await load(page, HAND);
  const hand = await seen(page);
  expect(hand.traces, 'the hand trace is on the board').toBe(1);
  expect(hand.lines).toEqual(['11.500,16.000 - 5.500,10.000']);
});

test('control: the same net with no copper has a line to every pad', async ({ page }) => {
  await ready(page);
  await load(page, BARE);
  const bare = await seen(page);
  expect(bare.traces).toBe(0);
  expect(bare.lines).toEqual(['11.500,16.000 - 19.500,10.000', '11.500,16.000 - 5.500,10.000']);
});

test('a session file shows what check says about the design it becomes', async ({ page }) => {
  await ready(page);
  await load(page, BARE);

  await page.locator('input[type="file"][accept=".cypcb,.ses"]').setInputFiles({
    name: 'hand_copper.ses',
    mimeType: 'text/plain',
    buffer: Buffer.from(SES),
  });
  await expect(page.locator('#status-text')).toContainText('Loaded routes', { timeout: 15_000 });
  const opened = await seen(page);
  expect(opened.traces, 'the session file put its trace on the board').toBe(1);
  expect(opened.lines).toEqual(['11.500,16.000 - 5.500,10.000']);

  // The design the session file became, as the editor holds it.
  const design: string = await page.evaluate(() => (window as any).__editor.getModel().getValue());
  expect(design, 'the routed copper reached the design text').toMatch(/^\s*trace\s+VCC\b/m);

  // The checker's rows on that text, and the same text loaded again.
  const rows = checkRows(design);
  expect(rows.some((r) => r.startsWith('unrouted-pin | R2.1')), rows.join('\n')).toBe(true);
  expect(opened.rows).toEqual(rows);
  await load(page, design);
  expect((await seen(page)).lines).toEqual(opened.lines);
});
