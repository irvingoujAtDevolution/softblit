//! Browser demo for softblit: a VNC-style workload (static background, several independently
//! moving rects, each producing small dirty regions per frame) decoded straight into the
//! library-owned framebuffer, in every supported pixel format.

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
    stats_sink: StatsSink,
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
        stats_sink: StatsSink::Dom,
    }));

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
        stats_sink: StatsSink::Worker(scope.clone()),
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
    let text = format!(
        "{fps:5.1} fps | {rects:4.1} rects/frame | {kib:7.1} KiB uploaded/frame | {:?} | {:?}",
        app.surface.format(),
        app.surface.scaling(),
    );
    app.stats_sink.publish(&text);
    app.last_stats_at = now;
    app.frames_since_stats = 0;
    app.rects_acc = 0;
    app.bytes_acc = 0;
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
    let offset = y as usize * stride + x as usize * format.bytes_per_pixel();
    match format {
        PixelFormat::Rgb24 => bytes[offset..offset + 3].copy_from_slice(&[r, g, b]),
        PixelFormat::Bgr24 => bytes[offset..offset + 3].copy_from_slice(&[b, g, r]),
        PixelFormat::Rgba8 => bytes[offset..offset + 4].copy_from_slice(&[r, g, b, 0xff]),
        PixelFormat::Bgra8 => bytes[offset..offset + 4].copy_from_slice(&[b, g, r, 0xff]),
        // Deliberately write 0 in the X byte: if the blit did not force alpha to 1, the screen
        // would be black/transparent, so the demo doubles as a visual test of force_opaque.
        PixelFormat::Rgbx8 => bytes[offset..offset + 4].copy_from_slice(&[r, g, b, 0x00]),
        PixelFormat::Bgrx8 => bytes[offset..offset + 4].copy_from_slice(&[b, g, r, 0x00]),
        _ => unreachable!("demo covers all v0.1 formats"),
    }
}
