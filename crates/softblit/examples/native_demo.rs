//! Native presentation smoke test: the web demo's workload (gradient background + bouncing
//! square, Rgb24 packed format with GPU compute unpack) presented to a winit window through
//! the same `Surface` used on wasm.
//!
//! Runs for a fixed number of frames and exits, printing upload stats — so it doubles as a
//! "native target works" check: `cargo run --example native_demo`.

#[cfg(target_arch = "wasm32")]
fn main() {
    unimplemented!("native example only");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use winit::event_loop::EventLoop;

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = native::App::default();
    event_loop.run_app(&mut app).expect("run event loop");
    app.report();
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::Arc;

    use softblit::{PixelFormat, Rect, ScalingMode, Surface, SurfaceDescriptor};
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::ActiveEventLoop;
    use winit::window::{Window, WindowId};

    const SRC_W: u32 = 800;
    const SRC_H: u32 = 500;
    const FRAMES: u32 = 300;

    #[derive(Default)]
    pub struct App {
        window: Option<Arc<Window>>,
        surface: Option<Surface>,
        pos: (f32, f32),
        vel: (f32, f32),
        frames: u32,
        bytes_uploaded: u64,
        rects_uploaded: u64,
    }

    impl App {
        pub fn report(&self) {
            assert!(
                self.frames >= FRAMES,
                "presented only {} frames",
                self.frames
            );
            println!(
                "native demo OK: {} frames presented, {} rects, {} KiB uploaded",
                self.frames,
                self.rects_uploaded,
                self.bytes_uploaded / 1024,
            );
        }
    }

    fn paint_bg(bytes: &mut [u8], rect: Rect) {
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let o = ((y * SRC_W + x) * 3) as usize;
                bytes[o] = (x * 255 / SRC_W) as u8;
                bytes[o + 1] = (y * 255 / SRC_H) as u8;
                bytes[o + 2] = 0x60;
            }
        }
    }

    fn paint_square(bytes: &mut [u8], rect: Rect) {
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let o = ((y * SRC_W + x) * 3) as usize;
                bytes[o..o + 3].copy_from_slice(&[0x3c, 0xe6, 0xdc]);
            }
        }
    }

    impl ApplicationHandler for App {
        fn resumed(&mut self, event_loop: &ActiveEventLoop) {
            let window = Arc::new(
                event_loop
                    .create_window(
                        Window::default_attributes()
                            .with_title("softblit native demo")
                            .with_inner_size(winit::dpi::PhysicalSize::new(1024, 640)),
                    )
                    .expect("create window"),
            );
            let size = window.inner_size();
            let mut surface = pollster::block_on(Surface::new_windowed(
                window.clone(),
                (size.width, size.height),
                SurfaceDescriptor {
                    source_size: (SRC_W, SRC_H),
                    format: PixelFormat::Rgb24,
                    scaling: ScalingMode::Fit,
                },
            ))
            .expect("create surface");

            let mut frame = surface.frame_mut();
            paint_bg(frame.bytes_mut(), Rect::new(0, 0, SRC_W, SRC_H));
            frame.mark_full_dirty();
            drop(frame);

            self.pos = (60.0, 45.0);
            self.vel = (3.1, 2.3);
            self.surface = Some(surface);
            window.request_redraw();
            self.window = Some(window);
        }

        fn window_event(
            &mut self,
            event_loop: &ActiveEventLoop,
            _id: WindowId,
            event: WindowEvent,
        ) {
            match event {
                WindowEvent::CloseRequested => event_loop.exit(),
                WindowEvent::Resized(size) => {
                    if let Some(surface) = &mut self.surface {
                        surface.resize_target(size.width, size.height);
                    }
                }
                WindowEvent::RedrawRequested => {
                    let (Some(surface), Some(window)) = (&mut self.surface, &self.window) else {
                        return;
                    };

                    const SIZE: u32 = 56;
                    let old = Rect::new(self.pos.0 as u32, self.pos.1 as u32, SIZE, SIZE);
                    self.pos.0 += self.vel.0;
                    self.pos.1 += self.vel.1;
                    if self.pos.0 < 0.0 || self.pos.0 + SIZE as f32 > SRC_W as f32 {
                        self.vel.0 = -self.vel.0;
                        self.pos.0 = self.pos.0.clamp(0.0, (SRC_W - SIZE) as f32);
                    }
                    if self.pos.1 < 0.0 || self.pos.1 + SIZE as f32 > SRC_H as f32 {
                        self.vel.1 = -self.vel.1;
                        self.pos.1 = self.pos.1.clamp(0.0, (SRC_H - SIZE) as f32);
                    }
                    let new = Rect::new(self.pos.0 as u32, self.pos.1 as u32, SIZE, SIZE);

                    let mut frame = surface.frame_mut();
                    paint_bg(frame.bytes_mut(), old);
                    paint_square(frame.bytes_mut(), new);
                    frame.mark_dirty(old);
                    frame.mark_dirty(new);
                    drop(frame);

                    let stats = surface.present().expect("present");
                    self.bytes_uploaded += stats.bytes_uploaded;
                    self.rects_uploaded += u64::from(stats.rects_uploaded);
                    self.frames += 1;
                    if self.frames >= FRAMES {
                        event_loop.exit();
                    } else {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }
    }
}
