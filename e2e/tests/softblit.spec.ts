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

// BT.601 full-range luma, matching the demo's gray conversion.
function lumaOf([r, g, b]: [number, number, number]): [number, number, number] {
  const l = Math.round((r * 299 + g * 587 + b * 114) / 1000);
  return [l, l, l];
}

// Per-format expected-color transform and tolerance: 16-bit formats quantize (up to 8 per
// channel), gray formats broadcast luma, I420 round-trips a lossy color conversion.
const FORMATS: { value: string; tol: number; map?: (rgb: [number, number, number]) => [number, number, number] }[] = [
  { value: 'rgb24', tol: TOLERANCE },
  { value: 'bgr24', tol: TOLERANCE },
  { value: 'rgba8', tol: TOLERANCE },
  { value: 'bgra8', tol: TOLERANCE },
  { value: 'rgbx8', tol: TOLERANCE },
  { value: 'bgrx8', tol: TOLERANCE },
  { value: 'rgb565', tol: 18 },
  { value: 'rgb555', tol: 18 },
  { value: 'gray8', tol: TOLERANCE, map: lumaOf },
  { value: 'gray16', tol: TOLERANCE, map: lumaOf },
  { value: 'i420', tol: 26 },
];

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

function expectClose(actual: number[], expected: [number, number, number], label: string, tol = TOLERANCE) {
  for (let c = 0; c < 3; c++) {
    expect
      .soft(Math.abs(actual[c] - expected[c]), `${label} channel ${c}: got ${actual}, want ${expected}`)
      .toBeLessThanOrEqual(tol);
  }
}

test.describe('softblit demo', () => {
  test('all eleven pixel formats produce correct pixels', async ({ page }) => {
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

    for (const { value, tol, map } of FORMATS) {
      await page.selectOption('#format', value);
      // Format switch reallocates and repaints; give it a few frames.
      await page.waitForTimeout(300);
      const samples = await sampleCanvas(page, points);
      samples.forEach((rgba, i) =>
        expectClose(rgba, map ? map(expected[i]) : expected[i], `${value} @ (${points[i][0]},${points[i][1]})`, tol),
      );
    }
  });

  test('cursor overlay composites with alpha over the source', async ({ page }) => {
    await page.goto('/www/index.html?animate=0');
    await waitReady(page);

    // 40x40 overlay at source (200, 150): 8px transparent border around an opaque red core.
    await page.evaluate(() => (window as any).softblitDemo.demo_set_cursor(200, 150, false));
    await page.waitForTimeout(200);

    // Core: source (220, 170) -> canvas (281.6, 217.6).
    const [core, border] = await sampleCanvas(page, [
      [282, 218],
      [259, 195], // source (202.3, 152.3): inside the transparent border -> background gradient
    ]);
    expectClose(core, [255, 0, 0], 'cursor core');
    expectClose(border, expectedAt(259 / FIT, 195 / FIT), 'cursor transparent border');

    // Moving the cursor is a uniform write + re-blit; the old position must show background.
    await page.evaluate(() => (window as any).softblitDemo.demo_set_cursor(600, 300, false));
    await page.waitForTimeout(200);
    const [oldSpot] = await sampleCanvas(page, [[282, 218]]);
    expectClose(oldSpot, expectedAt(282 / FIT, 218 / FIT), 'cursor old position restored');
  });

  test('external image import (ImageBitmap / VideoFrame path) lands in the persistent texture', async ({
    page,
  }) => {
    await page.goto('/www/index.html?animate=0');
    await waitReady(page);

    await page.evaluate(async () => {
      const off = new OffscreenCanvas(64, 64);
      const ctx = off.getContext('2d')!;
      ctx.fillStyle = '#00ff80';
      ctx.fillRect(0, 0, 64, 64);
      const bitmap = await createImageBitmap(off);
      (window as any).softblitDemo.demo_import_bitmap(bitmap, 600, 400);
      bitmap.close();
    });
    await page.waitForTimeout(200);

    // Center of the imported block: source (632, 432) -> canvas (808.96, 552.96).
    const [imported, outside] = await sampleCanvas(page, [
      [809, 553],
      [700, 553], // left of the block: still background
    ]);
    expectClose(imported, [0, 255, 128], 'imported bitmap pixels');
    expectClose(outside, expectedAt(700 / FIT, 553 / FIT), 'pixels outside the import');
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

  test('A/B benchmark: GPU presenter vs Canvas2D pack+putImageData (informational)', async ({ page }) => {
    const results: Record<string, string> = {};
    for (const [name, url] of [
      ['gpu', '/www/index.html'],
      ['canvas2d', '/www/index.html?renderer=canvas2d'],
    ] as const) {
      await page.goto(url);
      const stats = page.locator('#stats');
      await expect.poll(async () => stats.textContent(), { timeout: 20_000 }).toContain('cpu ms');
      // Let the numbers settle past warmup, then take the latest line.
      await page.waitForTimeout(2_000);
      results[name] = (await stats.textContent()) ?? '';
      const cpu = Number(results[name].match(/([\d.]+) cpu ms/)?.[1]);
      expect(cpu).toBeGreaterThan(0);
    }
    console.log(`[bench] gpu:      ${results.gpu.trim()}`);
    console.log(`[bench] canvas2d: ${results.canvas2d.trim()}`);
  });

  test('hidpi mode renders at physical resolution', async ({ browser }) => {
    const context = await browser.newContext({ deviceScaleFactor: 2, viewport: { width: 1280, height: 900 } });
    const page = await context.newPage();
    await page.goto('/www/index.html?animate=0&hidpi=1');
    await waitReady(page);

    const backing = await page.evaluate(() => {
      const c = document.getElementById('screen') as HTMLCanvasElement;
      // clientWidth = CSS content box (excludes the 1px border).
      return { w: c.width, h: c.height, cssW: c.clientWidth };
    });
    expect(backing.w).toBe(2048);
    expect(backing.h).toBe(1280);
    expect(backing.cssW).toBe(1024);

    // Content still correct (sampleCanvas normalizes coordinates by the screenshot scale).
    const [sample] = await sampleCanvas(page, [[512, 320]]);
    expectClose(sample, expectedAt(512 / FIT, 320 / FIT), 'hidpi center');
    await context.close();
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
