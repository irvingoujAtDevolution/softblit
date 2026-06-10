import { test, expect, chromium, Page } from '@playwright/test';
import { PNG } from 'pngjs';

// WebGL2 fallback: Chrome 149 no longer honors the WebGPU kill-switch flags, so the test
// removes `navigator.gpu` with an init script before the app loads — wgpu then detects no
// WebGPU and uses its GL backend, and packed formats take the CPU-expand path.
// Requires the demo built with `--features webgl`.

const BASE = 'http://127.0.0.1:8931';
const SRC_W = 800;
const SRC_H = 500;
const FIT = 1024 / 800;

function expectedAt(sx: number, sy: number): [number, number, number] {
  return [Math.round((sx * 255) / SRC_W), Math.round((sy * 255) / SRC_H), 0x60];
}

async function sampleCanvas(page: Page, points: [number, number][]): Promise<number[][]> {
  const shot = await page.locator('#screen').screenshot();
  const png = PNG.sync.read(shot);
  const sx = png.width / 1024;
  const sy = png.height / 640;
  return points.map(([x, y]) => {
    const px = Math.min(Math.round(x * sx), png.width - 1);
    const py = Math.min(Math.round(y * sy), png.height - 1);
    const i = (py * png.width + px) * 4;
    return [png.data[i], png.data[i + 1], png.data[i + 2], png.data[i + 3]];
  });
}

test('packed formats render correctly on the WebGL2 CPU-expand fallback', async () => {
  const browser = await chromium.launch({ channel: 'chrome' });
  try {
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 } });
    await context.addInitScript(() => {
      delete (Navigator.prototype as unknown as Record<string, unknown>).gpu;
      delete (WorkerNavigator?.prototype as unknown as Record<string, unknown> | undefined)?.gpu;
    });
    const page = await context.newPage();
    await page.goto(`${BASE}/www/index.html?animate=0`);

    const gpuPresent = await page.evaluate(() => 'gpu' in navigator && (navigator as any).gpu !== undefined);
    expect(gpuPresent, 'WebGPU must be disabled for the GL fallback to be exercised').toBe(false);

    const stats = page.locator('#stats');
    await expect.poll(async () => stats.textContent(), { timeout: 30_000 }).not.toContain('starting');
    const text = (await stats.textContent()) ?? '';
    expect(text, `demo failed on GL backend: ${text}`).not.toContain('failed');

    const points: [number, number][] = [
      [16, 16],
      [1008, 16],
      [512, 320],
    ];
    const expected = points.map(([cx, cy]) => expectedAt(cx / FIT, cy / FIT));

    for (const format of ['rgb24', 'rgb565', 'gray8', 'i420']) {
      await page.selectOption('#format', format);
      await page.waitForTimeout(300);
      const samples = await sampleCanvas(page, points);
      const tol = format === 'i420' ? 26 : 18;
      samples.forEach((rgba, i) => {
        let want = expected[i];
        if (format === 'gray8') {
          const l = Math.round((want[0] * 299 + want[1] * 587 + want[2] * 114) / 1000);
          want = [l, l, l];
        }
        for (let c = 0; c < 3; c++) {
          expect
            .soft(Math.abs(rgba[c] - want[c]), `${format} @ ${points[i]}: got ${rgba}, want ${want}`)
            .toBeLessThanOrEqual(tol);
        }
      });
    }
  } finally {
    await browser.close();
  }
});
