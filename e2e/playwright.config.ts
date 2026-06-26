import { defineConfig } from '@playwright/test';

// WebGPU in headless Chromium:
// - On a developer machine the real GPU is used.
// - On GPU-less CI runners, Chromium falls back to SwiftShader; --enable-unsafe-webgpu plus
//   the swiftshader WebGPU adapter keeps the API available there.
const launchArgs = [
  '--enable-unsafe-webgpu',
  '--enable-features=Vulkan',
];

export default defineConfig({
  testDir: './tests',
  timeout: 60_000,
  retries: 0,
  use: {
    baseURL: 'http://127.0.0.1:8931',
    // Bundled Chromium ships without GPU support on some platforms; the stable Chrome channel
    // has working headless WebGPU. CI: `npx playwright install chrome`.
    channel: 'chrome',
    viewport: { width: 1280, height: 900 },
  },
  // launchOptions live per-project (they deep-merge, and the GL project must NOT inherit
  // --enable-unsafe-webgpu, which would force navigator.gpu back on).
  projects: [
    {
      name: 'webgpu',
      testMatch: /softblit\.spec\.ts/,
      use: { launchOptions: { args: launchArgs } },
    },
    {
      // WebGL2 fallback: WebGPU disabled, wgpu falls back to its GL backend and packed formats
      // take the CPU-expand path (requires the demo built with `--features webgl`).
      name: 'webgl',
      testMatch: /webgl\.spec\.ts/,
      use: {
        launchOptions: {
          args: ['--disable-features=WebGPU,WebGPUService', '--disable-blink-features=WebGPU'],
        },
      },
    },
  ],
  webServer: {
    command: 'python -m http.server 8931 --directory ../crates/softblit-demo',
    url: 'http://127.0.0.1:8931/www/index.html',
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
