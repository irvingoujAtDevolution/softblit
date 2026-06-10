// Live end-to-end check: drives the iron-svelte-client login flow against local
// gateway/tokengen infrastructure and verifies the session canvas is (a) rendering real
// content and (b) owned by the softblit WebGPU presenter (not the Canvas2D fallback).
//
// Usage: node session-check.mjs <url> <outPrefix> [waitMs]
//   e.g. node session-check.mjs "http://localhost:5173/?protocol=vnc" vnc 12000
//
// Exit code 0 = session rendered on WebGPU. Non-zero = failure (see stdout).
import { chromium } from '@playwright/test';

const [url, outPrefix = 'session', waitMsArg] = process.argv.slice(2);
const waitMs = Number(waitMsArg ?? 12000);
if (!url) {
    console.error('usage: node session-check.mjs <url> <outPrefix> [waitMs]');
    process.exit(2);
}

const browser = await chromium.launch({
    channel: 'chrome',
    args: ['--enable-unsafe-webgpu', '--enable-features=Vulkan'],
});
const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });

const consoleLines = [];
page.on('console', (msg) => consoleLines.push(`[${msg.type()}] ${msg.text()}`));
page.on('pageerror', (err) => consoleLines.push(`[pageerror] ${err.message}`));

let failure = null;
try {
    await page.goto(url, { waitUntil: 'networkidle' });

    // The login form is prefilled from protocol-aware defaults; just click Login.
    await page.locator('button', { hasText: 'Login' }).first().click();

    // Wait for the session to establish and paint.
    await page.waitForTimeout(waitMs);

    await page.screenshot({ path: `${outPrefix}-page.png`, fullPage: false });

    // Find the session canvas, piercing shadow DOM.
    const probe = await page.evaluate(() => {
        const findCanvases = (root, acc) => {
            for (const el of root.querySelectorAll('*')) {
                if (el.tagName === 'CANVAS') acc.push(el);
                if (el.shadowRoot) findCanvases(el.shadowRoot, acc);
            }
            return acc;
        };
        const canvases = findCanvases(document, []);
        return canvases.map((c) => {
            // A canvas whose context type is already claimed by WebGPU returns null for '2d'.
            let twoD = 'unknown';
            try {
                twoD = c.getContext('2d') === null ? 'null' : 'available';
            } catch {
                twoD = 'throws';
            }
            return { width: c.width, height: c.height, visible: c.checkVisibility?.() ?? true, twoD };
        });
    });

    console.log('canvases:', JSON.stringify(probe));

    const session = probe.find((c) => c.width > 300 && c.height > 200 && c.visible);
    if (!session) {
        failure = 'no visible session canvas found';
    } else if (session.twoD !== 'null') {
        failure = `session canvas still yields a 2d context (twoD=${session.twoD}) — softblit/WebGPU is NOT the active presenter`;
    }

    // Pixel content check via screenshot of the canvas element region.
    const canvasShot = await page.evaluate(() => {
        const find = (root) => {
            for (const el of root.querySelectorAll('*')) {
                if (el.tagName === 'CANVAS' && el.width > 300) return el.getBoundingClientRect();
                if (el.shadowRoot) {
                    const r = find(el.shadowRoot);
                    if (r) return r;
                }
            }
            return null;
        };
        const r = find(document);
        return r ? { x: r.x, y: r.y, width: r.width, height: r.height } : null;
    });
    if (canvasShot && canvasShot.width > 50) {
        const buf = await page.screenshot({ clip: canvasShot });
        const { PNG } = await import('pngjs');
        const png = PNG.sync.read(buf);
        let nonBlack = 0;
        const total = png.width * png.height;
        for (let i = 0; i < total; i++) {
            const o = i * 4;
            if (png.data[o] > 16 || png.data[o + 1] > 16 || png.data[o + 2] > 16) nonBlack++;
        }
        const ratio = nonBlack / total;
        console.log(`canvas non-black pixel ratio: ${(ratio * 100).toFixed(1)}%`);
        if (!failure && ratio < 0.05) {
            failure = `canvas is ${(100 - ratio * 100).toFixed(1)}% black — no session content rendered`;
        }
    } else if (!failure) {
        failure = 'could not locate session canvas bounding box';
    }
} catch (err) {
    failure = `script error: ${err}`;
}

const interesting = consoleLines.filter(
    (l) =>
        /softblit|webgpu|error|warn|panic|present/i.test(l) && !/Download the Svelte|vite|favicon/i.test(l),
);
console.log('--- console (filtered) ---');
for (const line of interesting.slice(0, 40)) console.log(line);

await browser.close();

if (failure) {
    console.error(`FAIL: ${failure}`);
    process.exit(1);
}
console.log('PASS: session canvas rendered via WebGPU (softblit)');
