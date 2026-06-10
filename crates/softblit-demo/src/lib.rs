//! Browser demo for softblit: a VNC-style workload (static background, several independently
//! moving rects, each producing small dirty regions per frame) decoded straight into the
//! library-owned framebuffer, in every supported pixel format.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;

use softblit::{
    FrameMut, PixelFormat, Rect, ScalingMode, Surface, SurfaceDescriptor, SurfaceTarget,
};
use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;

const SRC_W: u32 = 800;
const SRC_H: u32 = 500;
const STATS_EVERY: u32 = 30;

struct Square {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    size: u32,
    color: [u8; 3],
}

/// Where the stats line goes: the page's `#stats` element (main thread) or a `postMessage` to
/// the page (worker mode).
enum StatsSink {
    Dom,
    Worker(web_sys::DedicatedWorkerGlobalScope),
}

impl StatsSink {
    fn publish(&self, text: &str) {
        match self {
            Self::Dom => {
                if let Some(stats) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.get_element_by_id("stats"))
                {
                    stats.set_text_content(Some(text));
                }
            }
            Self::Worker(scope) => {
                let _ = scope.post_message(&JsValue::from_str(text));
            }
        }
    }
}

struct App {
    surface: Surface,
    squares: Vec<Square>,
    frames_since_stats: u32,
    last_stats_at: f64,
    rects_acc: u64,
    bytes_acc: u64,
    cpu_ms_acc: f64,
    stats_sink: StatsSink,
    cursor_angle: f32,
    cursor_animated: bool,
}

thread_local! {
    /// Main-thread demo app handle, for the JS-callable test hooks below.
    static APP: RefCell<Option<Rc<RefCell<App>>>> = const { RefCell::new(None) };
}

/// Test/demo hook: imports an `ImageBitmap` into the persistent texture (the WebCodecs
/// `VideoFrame` integration path, exercised from JS) and presents.
#[wasm_bindgen]
pub fn demo_import_bitmap(bitmap: web_sys::ImageBitmap, x: u32, y: u32) -> Result<(), JsValue> {
    APP.with(|app| {
        let app = app.borrow();
        let app = app
            .as_ref()
            .ok_or_else(|| JsValue::from_str("demo not started"))?;
        let mut app = app.borrow_mut();
        app.surface
            .import_image_bitmap(&bitmap, (x, y))
            .map_err(|e| JsValue::from_str(&e.to_string()))
    })
}

/// Test/demo hook: shows a cursor overlay at `(x, y)` — a 24x24 opaque red square with an
/// 8px transparent border (so alpha blending is visually verifiable), optionally animated.
#[wasm_bindgen]
pub fn demo_set_cursor(x: i32, y: i32, animated: bool) -> Result<(), JsValue> {
    APP.with(|app| {
        let app = app.borrow();
        let app = app
            .as_ref()
            .ok_or_else(|| JsValue::from_str("demo not started"))?;
        let mut app = app.borrow_mut();
        let cursor = make_cursor_image();
        app.surface
            .set_cursor(Some((&cursor, CURSOR_SIZE, CURSOR_SIZE)));
        app.surface.set_cursor_position(x, y);
        app.cursor_animated = animated;
        Ok(())
    })
}

const CURSOR_SIZE: u32 = 40;

/// 40x40 RGBA: an 8px fully transparent border around a 24x24 opaque red core.
fn make_cursor_image() -> Vec<u8> {
    let mut image = vec![0u8; (CURSOR_SIZE * CURSOR_SIZE * 4) as usize];
    for y in 8..CURSOR_SIZE - 8 {
        for x in 8..CURSOR_SIZE - 8 {
            let o = ((y * CURSOR_SIZE + x) * 4) as usize;
            image[o..o + 4].copy_from_slice(&[0xff, 0x00, 0x00, 0xff]);
        }
    }
    image
}

#[wasm_bindgen]
pub async fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    // `?animate=0` renders the static background only — deterministic pixels for e2e tests.
    let animate = !window
        .location()
        .search()
        .unwrap_or_default()
        .contains("animate=0");
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas: web_sys::HtmlCanvasElement = document
        .get_element_by_id("screen")
        .ok_or_else(|| JsValue::from_str("no #screen canvas"))?
        .dyn_into()?;

    // `?hidpi=1`: render at physical resolution (CSS size stays the same, the backing store and
    // swapchain scale by devicePixelRatio) — crisp output on high-DPI displays, which the
    // Canvas2D path cannot do without re-rendering at the higher resolution.
    if window
        .location()
        .search()
        .unwrap_or_default()
        .contains("hidpi=1")
    {
        let dpr = window.device_pixel_ratio();
        let (css_w, css_h) = (canvas.width(), canvas.height());
        let style = canvas.style();
        let _ = style.set_property("width", &format!("{css_w}px"));
        let _ = style.set_property("height", &format!("{css_h}px"));
        canvas.set_width((f64::from(css_w) * dpr) as u32);
        canvas.set_height((f64::from(css_h) * dpr) as u32);
    }

    let mut surface = Surface::new(
        SurfaceTarget::Canvas(canvas),
        SurfaceDescriptor {
            source_size: (SRC_W, SRC_H),
            // Default to the packed path: this is IronVNC's RGB8 framebuffer, uploaded raw and
            // unpacked on the GPU.
            format: PixelFormat::Rgb24,
            scaling: ScalingMode::Fit,
        },
    )
    .await
    .map_err(|e| JsValue::from_str(&e.to_string()))?;

    repaint_full(&mut surface);

    let now = performance_now();
    let app = Rc::new(RefCell::new(App {
        surface,
        squares: make_squares(animate),
        frames_since_stats: 0,
        last_stats_at: now,
        rects_acc: 0,
        bytes_acc: 0,
        cpu_ms_acc: 0.0,
        stats_sink: StatsSink::Dom,
        cursor_angle: 0.0,
        cursor_animated: false,
    }));

    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));
    hook_controls(&document, &app)?;
    run_loop(app);
    Ok(())
}

/// Worker entry: the same animated workload presenting to an `OffscreenCanvas` transferred from
/// the main thread — decode and present run entirely off the main thread ("background update").
/// Stats are posted back to the page as messages.
#[wasm_bindgen]
pub async fn start_offscreen(
    canvas: web_sys::OffscreenCanvas,
    animate: bool,
) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let scope: web_sys::DedicatedWorkerGlobalScope = js_sys::global()
        .dyn_into()
        .map_err(|_| JsValue::from_str("start_offscreen must run inside a dedicated worker"))?;

    let mut surface = Surface::new(
        SurfaceTarget::OffscreenCanvas(canvas),
        SurfaceDescriptor {
            source_size: (SRC_W, SRC_H),
            format: PixelFormat::Rgb24,
            scaling: ScalingMode::Fit,
        },
    )
    .await
    .map_err(|e| JsValue::from_str(&e.to_string()))?;

    repaint_full(&mut surface);

    let now = performance_now();
    let app = Rc::new(RefCell::new(App {
        surface,
        squares: make_squares(animate),
        frames_since_stats: 0,
        last_stats_at: now,
        rects_acc: 0,
        bytes_acc: 0,
        cpu_ms_acc: 0.0,
        stats_sink: StatsSink::Worker(scope.clone()),
        cursor_angle: 0.0,
        cursor_animated: false,
    }));

    // Workers have no reliable rAF across browsers; a 16ms interval approximates 60 Hz.
    let tick_closure = Closure::<dyn FnMut()>::new(move || tick(&app));
    let _interval_id = scope.set_interval_with_callback_and_timeout_and_arguments_0(
        tick_closure.as_ref().unchecked_ref(),
        16,
    )?;
    tick_closure.forget();

    Ok(())
}

fn make_squares(animate: bool) -> Vec<Square> {
    let palette: [[u8; 3]; 6] = [
        [0xe6, 0x3c, 0x3c],
        [0x3c, 0xe6, 0x73],
        [0x3c, 0x8c, 0xe6],
        [0xe6, 0xd2, 0x3c],
        [0xc8, 0x3c, 0xe6],
        [0x3c, 0xe6, 0xdc],
    ];
    let square_count = if animate { 12usize } else { 0 };
    (0..square_count)
        .map(|i| {
            let fi = i as f32;
            Square {
                x: 30.0 + fi * 60.0,
                y: 25.0 + (fi * 37.0) % 380.0,
                vx: 1.0 + (i % 5) as f32 * 0.8,
                vy: 0.7 + (i % 7) as f32 * 0.6,
                size: 24 + (i % 4) as u32 * 14,
                color: palette[i % palette.len()],
            }
        })
        .collect()
}

fn hook_controls(document: &web_sys::Document, app: &Rc<RefCell<App>>) -> Result<(), JsValue> {
    let format_select: web_sys::HtmlSelectElement = document
        .get_element_by_id("format")
        .ok_or_else(|| JsValue::from_str("no #format select"))?
        .dyn_into()?;
    let scaling_select: web_sys::HtmlSelectElement = document
        .get_element_by_id("scaling")
        .ok_or_else(|| JsValue::from_str("no #scaling select"))?
        .dyn_into()?;

    {
        let app = app.clone();
        let select = format_select.clone();
        let on_format = Closure::<dyn FnMut()>::new(move || {
            let format = match select.value().as_str() {
                "rgba8" => PixelFormat::Rgba8,
                "bgra8" => PixelFormat::Bgra8,
                "rgbx8" => PixelFormat::Rgbx8,
                "bgrx8" => PixelFormat::Bgrx8,
                "bgr24" => PixelFormat::Bgr24,
                "rgb565" => PixelFormat::Rgb565,
                "rgb555" => PixelFormat::Rgb555,
                "gray8" => PixelFormat::Gray8,
                "gray16" => PixelFormat::Gray16,
                "i420" => PixelFormat::I420,
                _ => PixelFormat::Rgb24,
            };
            let mut app = app.borrow_mut();
            app.surface.set_format(format);
            repaint_full(&mut app.surface);
        });
        format_select.set_onchange(Some(on_format.as_ref().unchecked_ref()));
        on_format.forget();
    }

    {
        let app = app.clone();
        let select = scaling_select.clone();
        let on_scaling = Closure::<dyn FnMut()>::new(move || {
            let scaling = match select.value().as_str() {
                "fill" => ScalingMode::Fill,
                "stretch" => ScalingMode::Stretch,
                "integer" => ScalingMode::Integer,
                "native" => ScalingMode::Native1x,
                _ => ScalingMode::Fit,
            };
            app.borrow_mut().surface.set_scaling(scaling);
        });
        scaling_select.set_onchange(Some(on_scaling.as_ref().unchecked_ref()));
        on_scaling.forget();
    }

    Ok(())
}

type LoopClosure = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

fn run_loop(app: Rc<RefCell<App>>) {
    let cell: LoopClosure = Rc::new(RefCell::new(None));
    let starter = cell.clone();
    *starter.borrow_mut() = Some(Closure::new(move || {
        tick(&app);
        request_animation_frame(cell.borrow().as_ref().expect("loop closure"));
    }));
    request_animation_frame(starter.borrow().as_ref().expect("loop closure"));
}

fn request_animation_frame(closure: &Closure<dyn FnMut()>) {
    web_sys::window()
        .expect("window")
        .request_animation_frame(closure.as_ref().unchecked_ref())
        .expect("requestAnimationFrame");
}

fn tick(app: &Rc<RefCell<App>>) {
    let mut app = app.borrow_mut();
    let t_start = performance_now();

    if app.cursor_animated {
        app.cursor_angle += 0.05;
        let (cx, cy) = (
            (SRC_W as f32 / 2.0 + app.cursor_angle.cos() * 180.0) as i32,
            (SRC_H as f32 / 2.0 + app.cursor_angle.sin() * 120.0) as i32,
        );
        app.surface.set_cursor_position(cx, cy);
    }

    let App {
        surface, squares, ..
    } = &mut *app;

    let format = surface.format();
    if squares.is_empty() {
        // Static test mode: keep presenting (blit-only, zero upload) so canvas readback in e2e
        // tests always finds a freshly presented frame.
        surface.request_redraw();
    }
    {
        let mut frame = surface.frame_mut();
        for square in squares.iter_mut() {
            let old = square_rect(square);

            square.x += square.vx;
            square.y += square.vy;
            if square.x < 0.0 || square.x + square.size as f32 > SRC_W as f32 {
                square.vx = -square.vx;
                square.x = square.x.clamp(0.0, (SRC_W - square.size) as f32);
            }
            if square.y < 0.0 || square.y + square.size as f32 > SRC_H as f32 {
                square.vy = -square.vy;
                square.y = square.y.clamp(0.0, (SRC_H - square.size) as f32);
            }
            let new = square_rect(square);

            paint_background_rect(&mut frame, format, old);
            fill_rect(&mut frame, format, new, square.color);
            frame.mark_dirty(old);
            frame.mark_dirty(new);
        }
    }

    match surface.present() {
        Ok(stats) => {
            app.rects_acc += u64::from(stats.rects_uploaded);
            app.bytes_acc += stats.bytes_uploaded;
            app.cpu_ms_acc += performance_now() - t_start;
            app.frames_since_stats += 1;
            if app.frames_since_stats == STATS_EVERY {
                update_stats(&mut app);
            }
        }
        Err(e) => web_sys::console::error_1(&JsValue::from_str(&format!("present failed: {e}"))),
    }
}

fn update_stats(app: &mut App) {
    let now = performance_now();
    let elapsed_ms = now - app.last_stats_at;
    let fps = f64::from(STATS_EVERY) * 1000.0 / elapsed_ms.max(1.0);
    let rects = app.rects_acc as f64 / f64::from(STATS_EVERY);
    let kib = app.bytes_acc as f64 / f64::from(STATS_EVERY) / 1024.0;
    let cpu_ms = app.cpu_ms_acc / f64::from(STATS_EVERY);
    let text = format!(
        "{fps:5.1} fps | {rects:4.1} rects/frame | {kib:7.1} KiB uploaded/frame | {cpu_ms:5.2} cpu ms/frame | {:?} | {:?} | gpu",
        app.surface.format(),
        app.surface.scaling(),
    );
    app.stats_sink.publish(&text);
    app.last_stats_at = now;
    app.frames_since_stats = 0;
    app.rects_acc = 0;
    app.bytes_acc = 0;
    app.cpu_ms_acc = 0.0;
}

fn performance_now() -> f64 {
    if let Some(window) = web_sys::window() {
        return window.performance().map(|p| p.now()).unwrap_or(0.0);
    }
    js_sys::global()
        .dyn_into::<web_sys::WorkerGlobalScope>()
        .ok()
        .and_then(|scope| scope.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}

fn square_rect(square: &Square) -> Rect {
    Rect::new(square.x as u32, square.y as u32, square.size, square.size)
}

fn repaint_full(surface: &mut Surface) {
    let format = surface.format();
    let mut frame = surface.frame_mut();
    let full = Rect::new(0, 0, frame.width(), frame.height());
    paint_background_rect(&mut frame, format, full);
    frame.mark_full_dirty();
}

/// Background gradient, recomputable for any sub-rect so moving squares can be erased.
fn paint_background_rect(frame: &mut FrameMut<'_>, format: PixelFormat, rect: Rect) {
    let stride = frame.stride();
    let width = frame.width();
    let height = frame.height();
    let bytes = frame.bytes_mut();
    for y in rect.y..(rect.y + rect.height).min(height) {
        for x in rect.x..(rect.x + rect.width).min(width) {
            let r = (x * 255 / SRC_W) as u8;
            let g = (y * 255 / SRC_H) as u8;
            put_pixel(bytes, stride, format, x, y, [r, g, 0x60]);
        }
    }
}

fn fill_rect(frame: &mut FrameMut<'_>, format: PixelFormat, rect: Rect, color: [u8; 3]) {
    let stride = frame.stride();
    let width = frame.width();
    let height = frame.height();
    let bytes = frame.bytes_mut();
    for y in rect.y..(rect.y + rect.height).min(height) {
        for x in rect.x..(rect.x + rect.width).min(width) {
            put_pixel(bytes, stride, format, x, y, color);
        }
    }
}

fn put_pixel(bytes: &mut [u8], stride: usize, format: PixelFormat, x: u32, y: u32, rgb: [u8; 3]) {
    let [r, g, b] = rgb;
    if format == PixelFormat::I420 {
        put_pixel_i420(bytes, x, y, rgb);
        return;
    }
    let offset = y as usize * stride
        + x as usize
            * format
                .bytes_per_pixel()
                .expect("non-I420 formats are interleaved");
    match format {
        PixelFormat::Rgb24 => bytes[offset..offset + 3].copy_from_slice(&[r, g, b]),
        PixelFormat::Bgr24 => bytes[offset..offset + 3].copy_from_slice(&[b, g, r]),
        PixelFormat::Rgba8 => bytes[offset..offset + 4].copy_from_slice(&[r, g, b, 0xff]),
        PixelFormat::Bgra8 => bytes[offset..offset + 4].copy_from_slice(&[b, g, r, 0xff]),
        // Deliberately write 0 in the X byte: if the blit did not force alpha to 1, the screen
        // would be black/transparent, so the demo doubles as a visual test of force_opaque.
        PixelFormat::Rgbx8 => bytes[offset..offset + 4].copy_from_slice(&[r, g, b, 0x00]),
        PixelFormat::Bgrx8 => bytes[offset..offset + 4].copy_from_slice(&[b, g, r, 0x00]),
        PixelFormat::Rgb565 => {
            let v = (u16::from(r >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(b >> 3);
            bytes[offset..offset + 2].copy_from_slice(&v.to_le_bytes());
        }
        PixelFormat::Rgb555 => {
            let v = (u16::from(r >> 3) << 10) | (u16::from(g >> 3) << 5) | u16::from(b >> 3);
            bytes[offset..offset + 2].copy_from_slice(&v.to_le_bytes());
        }
        PixelFormat::Gray8 => bytes[offset] = luma(rgb),
        PixelFormat::Gray16 => {
            let v = u16::from(luma(rgb)) * 257;
            bytes[offset..offset + 2].copy_from_slice(&v.to_le_bytes());
        }
        PixelFormat::I420 => unreachable!("handled above"),
        _ => unreachable!("demo covers all formats"),
    }
}

/// Canvas2D reference presenter: the tuned pack-and-`putImageData` path (what ironvnc-web
/// shipped before softblit) running the **identical** workload, for honest A/B benchmarking
/// against the GPU path. Selected with `?renderer=canvas2d`.
#[wasm_bindgen]
pub async fn start_canvas2d() -> Result<(), JsValue> {
    use wasm_bindgen::Clamped;

    console_error_panic_hook::set_once();

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let animate = !window
        .location()
        .search()
        .unwrap_or_default()
        .contains("animate=0");
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("no document"))?;
    let canvas: web_sys::HtmlCanvasElement = document
        .get_element_by_id("screen")
        .ok_or_else(|| JsValue::from_str("no #screen canvas"))?
        .dyn_into()?;
    // Canvas2D cannot scale putImageData: backing = source size (real clients do the same).
    canvas.set_width(SRC_W);
    canvas.set_height(SRC_H);
    let ctx: web_sys::CanvasRenderingContext2d = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into()?;

    struct C2d {
        ctx: web_sys::CanvasRenderingContext2d,
        fb: Vec<u8>,
        rgba: Vec<u8>,
        squares: Vec<Square>,
        frames: u32,
        last_stats_at: f64,
        bytes_acc: u64,
        cpu_ms_acc: f64,
    }

    let mut fb = vec![0u8; PixelFormat::Rgb24.frame_len(SRC_W, SRC_H)];
    for y in 0..SRC_H {
        for x in 0..SRC_W {
            let r = (x * 255 / SRC_W) as u8;
            let g = (y * 255 / SRC_H) as u8;
            put_pixel(
                &mut fb,
                SRC_W as usize * 3,
                PixelFormat::Rgb24,
                x,
                y,
                [r, g, 0x60],
            );
        }
    }

    let state = Rc::new(RefCell::new(C2d {
        ctx,
        fb,
        rgba: Vec::new(),
        squares: make_squares(animate),
        frames: 0,
        last_stats_at: performance_now(),
        bytes_acc: 0,
        cpu_ms_acc: 0.0,
    }));

    fn blit_rect(s: &mut C2d, rect: Rect) -> Result<(), JsValue> {
        let needed = (rect.width * rect.height * 4) as usize;
        if s.rgba.len() < needed {
            s.rgba.resize(needed, 0);
        }
        // The pack pass softblit eliminates: RGB8 -> RGBA per dirty pixel, on the CPU.
        for dy in 0..rect.height as usize {
            for dx in 0..rect.width as usize {
                let src = ((rect.y as usize + dy) * SRC_W as usize + rect.x as usize + dx) * 3;
                let dst = (dy * rect.width as usize + dx) * 4;
                s.rgba[dst..dst + 3].copy_from_slice(&s.fb[src..src + 3]);
                s.rgba[dst + 3] = 0xff;
            }
        }
        let image = web_sys::ImageData::new_with_u8_clamped_array_and_sh(
            Clamped(&s.rgba[..needed]),
            rect.width,
            rect.height,
        )?;
        s.ctx
            .put_image_data(&image, f64::from(rect.x), f64::from(rect.y))?;
        s.bytes_acc += u64::from(rect.width * rect.height * 4);
        Ok(())
    }

    fn tick_c2d(state: &Rc<RefCell<C2d>>) {
        let mut s = state.borrow_mut();
        let t0 = performance_now();

        let mut rects: Vec<Rect> = Vec::new();
        let mut moved: Vec<(Rect, [u8; 3])> = Vec::new();
        for square in &mut s.squares {
            let old = square_rect(square);
            square.x += square.vx;
            square.y += square.vy;
            if square.x < 0.0 || square.x + square.size as f32 > SRC_W as f32 {
                square.vx = -square.vx;
                square.x = square.x.clamp(0.0, (SRC_W - square.size) as f32);
            }
            if square.y < 0.0 || square.y + square.size as f32 > SRC_H as f32 {
                square.vy = -square.vy;
                square.y = square.y.clamp(0.0, (SRC_H - square.size) as f32);
            }
            let new = square_rect(square);
            rects.push(old);
            rects.push(new);
            moved.push((new, square.color));
        }
        // Repaint background over old positions, then squares at new ones (same as GPU tick).
        let old_rects: Vec<Rect> = rects.iter().step_by(2).copied().collect();
        for rect in old_rects {
            for y in rect.y..(rect.y + rect.height).min(SRC_H) {
                for x in rect.x..(rect.x + rect.width).min(SRC_W) {
                    let r = (x * 255 / SRC_W) as u8;
                    let g = (y * 255 / SRC_H) as u8;
                    put_pixel(
                        &mut s.fb,
                        SRC_W as usize * 3,
                        PixelFormat::Rgb24,
                        x,
                        y,
                        [r, g, 0x60],
                    );
                }
            }
        }
        let moved_now = core::mem::take(&mut moved);
        for (rect, color) in moved_now {
            for y in rect.y..(rect.y + rect.height).min(SRC_H) {
                for x in rect.x..(rect.x + rect.width).min(SRC_W) {
                    put_pixel(
                        &mut s.fb,
                        SRC_W as usize * 3,
                        PixelFormat::Rgb24,
                        x,
                        y,
                        color,
                    );
                }
            }
        }
        for rect in rects {
            if let Err(e) = blit_rect(&mut s, rect) {
                web_sys::console::error_1(&e);
            }
        }

        s.cpu_ms_acc += performance_now() - t0;
        s.frames += 1;
        if s.frames == STATS_EVERY {
            let now = performance_now();
            let fps = f64::from(STATS_EVERY) * 1000.0 / (now - s.last_stats_at).max(1.0);
            let kib = s.bytes_acc as f64 / f64::from(STATS_EVERY) / 1024.0;
            let cpu = s.cpu_ms_acc / f64::from(STATS_EVERY);
            let text = format!(
                "{fps:5.1} fps | {kib:7.1} KiB uploaded/frame | {cpu:5.2} cpu ms/frame | Rgb24 | canvas2d"
            );
            if let Some(stats) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("stats"))
            {
                stats.set_text_content(Some(&text));
            }
            s.frames = 0;
            s.bytes_acc = 0;
            s.cpu_ms_acc = 0.0;
            s.last_stats_at = now;
        }
    }

    // Initial full paint.
    {
        let mut s = state.borrow_mut();
        if let Err(e) = blit_rect(&mut s, Rect::new(0, 0, SRC_W, SRC_H)) {
            web_sys::console::error_1(&e);
        }
    }

    let cell: LoopClosure = Rc::new(RefCell::new(None));
    let starter = cell.clone();
    *starter.borrow_mut() = Some(Closure::new(move || {
        tick_c2d(&state);
        request_animation_frame(cell.borrow().as_ref().expect("loop closure"));
    }));
    request_animation_frame(starter.borrow().as_ref().expect("loop closure"));
    Ok(())
}

/// BT.601 full-range luma, matching the e2e tests' expectations for gray formats.
fn luma([r, g, b]: [u8; 3]) -> u8 {
    ((u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000) as u8
}

/// Writes one pixel into the I420 planes (BT.601 limited range). Chroma cells are shared by
/// 2x2 pixels; the gradient varies slowly, so last-writer-wins is fine for the demo.
fn put_pixel_i420(bytes: &mut [u8], x: u32, y: u32, [r, g, b]: [u8; 3]) {
    let (w, h) = (SRC_W as usize, SRC_H as usize);
    let (rf, gf, bf) = (f32::from(r), f32::from(g), f32::from(b));
    let yv = 16.0 + (65.738 * rf + 129.057 * gf + 25.064 * bf) / 256.0;
    let u = 128.0 + (-37.945 * rf - 74.494 * gf + 112.439 * bf) / 256.0;
    let v = 128.0 + (112.439 * rf - 94.154 * gf - 18.285 * bf) / 256.0;

    let cw = w.div_ceil(2);
    let y_len = w * h;
    let chroma_len = cw * h.div_ceil(2);
    bytes[y as usize * w + x as usize] = yv.clamp(0.0, 255.0) as u8;
    let c = (y as usize / 2) * cw + x as usize / 2;
    bytes[y_len + c] = u.clamp(0.0, 255.0) as u8;
    bytes[y_len + chroma_len + c] = v.clamp(0.0, 255.0) as u8;
}
