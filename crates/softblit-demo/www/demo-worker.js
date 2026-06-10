// Dedicated worker: owns the transferred OffscreenCanvas and runs the softblit demo loop
// entirely off the main thread. Stats lines are posted back to the page.
import init, { start_offscreen } from '../pkg/softblit_demo.js';

self.onmessage = async (e) => {
  try {
    await init();
    await start_offscreen(e.data.canvas, e.data.animate);
  } catch (err) {
    self.postMessage('failed to start: ' + err);
  }
};
