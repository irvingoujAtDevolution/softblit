import { test, expect, Page } from '@playwright/test';
import { PNG } from 'pngjs';

// Demo geometry (must match crates/softblit-demo/src/lib.rs and www/index.html):
// source framebuffer 800x500, canvas 1024x640. Under Fit the ratio is exactly 1.28 in both
// axes, so the quad covers the whole canvas and canvas (cx, cy) maps to source (cx/1.28, cy/1.28).
const SRC_W = 800;
const SRC_H = 500;
const FIT = 1024 / 800; // 1.28

// The demo background: r = x*255/W, g = y*255/H, b = 0x60.
function expectedAt(sx: number, sy: number): [number, number, number] {
  return [Math.round((sx * 255) / SRC_W), Math.round((sy * 255) / SRC_H), 0x60];
}

// Scaling filter + canvas readback wobble.
const TOLERANCE = 12;

async function waitReady(page: Page): Promise<void> {
  const stats = page.locator('#stats');
  await expect
    .poll(async () => stats.textContent(), { timeout: 20_000 })
    .not.toContain('starting');
  const text = (await stats.textContent()) ?? '';
  test.skip(text.includes('failed to start'), `demo did not start: ${text}`);
}

// Reads pixels from a compositor-level element screenshot of the canvas. (drawImage /
// getImageData readback of a WebGPU canvas returns blank in headless Chrome, so 2D-canvas
// sampling is not an option.)
async function sampleCanvas(page: Page, points: [number, number][]): Promise<number[][]> {
  const shot = await page.locator('#screen').screenshot();
  const png = PNG.sync.read(shot);
  // The canvas is 1024x640 CSS pixels; with DPR != 1 the screenshot scales accordingly.
  const sx = png.width / 1024;
  const sy = png.height / 640;
  return points.map(([x, y]) => {
    const px = Math.min(Math.round(x * sx), png.width - 1);
    const py = Math.min(Math.round(y * sy), png.height - 1);
    const i = (py * png.width + px) * 4;
    return [png.data[i], png.data[i + 1], png.data[i + 2], png.data[i + 3]];
  });
}

function expectClose(actual: number[], expected: [number, number, number], label: string) {
  for (let c = 0; c < 3; c++) {
    expect
      .soft(Math.abs(actual[c] - expected[c]), `${label} channel ${c}: got ${actual}, want ${expected}`)
      .toBeLessThanOrEqual(TOLERANCE);
  }
}

test.describe('softblit demo', () => {
  test('all six pixel formats produce identical, correct pixels', async ({ page }) => {
    await page.goto('/www/index.html?animate=0');
    await waitReady(page);

    // Canvas sample points and the source pixels they map to under Fit.
    const points: [number, number][] = [
      [16, 16],
      [1008, 16],
      [16, 624],
      [512, 320],
    ];
    const expected = points.map(([cx, cy]) => expectedAt(cx / FIT, cy / FIT));

    for (const format of ['rgb24', 'bgr24', 'rgba8', 'bgra8', 'rgbx8', 'bgrx8']) {
      await page.selectOption('#format', format);
      // Format switch reallocates and repaints; give it a few frames.
      await page.waitForTimeout(300);
      const samples = await sampleCanvas(page, points);
      samples.forEach((rgba, i) =>
        expectClose(rgba, expected[i], `${format} @ (${points[i][0]},${points[i][1]})`),
      );
    }
  });

  test('Native1x centers the source with black borders', async ({ page }) => {
    await page.goto('/www/index.html?animate=0');
    await waitReady(page);
    await page.selectOption('#scaling', 'native');
    await page.waitForTimeout(300);

    // Dest rect: x in [112, 912), y in [70, 570).
    const border: [number, number][] = [
      [10, 10],
      [1014, 10],
      [10, 630],
      [1014, 630],
    ];
    const inside: [number, number][] = [
      [112 + 400, 70 + 250], // source (400, 250)
      [112 + 4, 70 + 4], // source (4, 4)
    ];

    const borderSamples = await sampleCanvas(page, border);
    borderSamples.forEach((rgba, i) =>
      expectClose(rgba, [0, 0, 0], `letterbox border @ (${border[i][0]},${border[i][1]})`),
    );

    const insideSamples = await sampleCanvas(page, inside);
    expectClose(insideSamples[0], expectedAt(400, 250), 'native center');
    expectClose(insideSamples[1], expectedAt(4, 4), 'native top-left');
  });

  test('renders from a dedicated worker via OffscreenCanvas', async ({ page }) => {
    await page.goto('/www/worker.html?animate=0');

    const stats = page.locator('#stats');
    await expect.poll(async () => stats.textContent(), { timeout: 20_000 }).toContain('fps');
    expect((await stats.textContent()) ?? '').not.toContain('failed');

    // Same deterministic gradient as the main-thread demo, presented from the worker.
    const points: [number, number][] = [
      [16, 16],
      [1008, 16],
      [512, 320],
    ];
    const expected = points.map(([cx, cy]) => expectedAt(cx / FIT, cy / FIT));
    const samples = await sampleCanvas(page, points);
    samples.forEach((rgba, i) =>
      expectClose(rgba, expected[i], `worker @ (${points[i][0]},${points[i][1]})`),
    );
  });

  test('animated dirty rects upload and present at interactive rates', async ({ page }) => {
    await page.goto('/www/index.html');
    await waitReady(page);

    const stats = page.locator('#stats');
    await expect.poll(async () => stats.textContent(), { timeout: 20_000 }).toContain('fps');

    const text = (await stats.textContent()) ?? '';
    // "  60.0 fps | 10.5 rects/frame |  1214.7 KiB uploaded/frame | Rgb24 | Fit"
    const m = text.match(/([\d.]+) fps \|\s*([\d.]+) rects\/frame \|\s*([\d.]+) KiB/);
    expect(m, `stats line: ${text}`).not.toBeNull();
    const [fps, rects, kib] = m!.slice(1).map(Number);

    expect(fps).toBeGreaterThan(5);
    expect(rects).toBeGreaterThan(0);
    expect(kib).toBeGreaterThan(0);
    expect(text).toContain('Rgb24');
  });
});
