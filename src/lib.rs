pub mod image;
mod traits;

pub use self::traits::*;

pub mod winit {
    pub use glium::glutin::event::VirtualKeyCode;
    pub use glium::glutin::event_loop::EventLoopProxy;
}

use egui_glium::egui_winit::WindowSettings;
use epi::{file_storage::FileStorage, Storage};
use glium::glutin::{
    self,
    event::{ElementState, Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::Window,
};
use std::{
    marker::PhantomData,
    sync::Arc,
    time::{Duration, Instant},
};

struct RepaintSignal<T: RepaintSignalMessage> {
    proxy: std::sync::Mutex<glutin::event_loop::EventLoopProxy<T>>,
    pd: PhantomData<T>,
}

impl<T: RepaintSignalMessage> epi::backend::RepaintSignal for RepaintSignal<T> {
    fn request_repaint(&self) {
        self.proxy
            .lock()
            .unwrap()
            .send_event(T::repaint_signal())
            .unwrap();
    }
}

pub fn run<T: App>(app: T) {
    let title = app.title();
    let mut persistence = Persistence::from_app_name(title);
    let event_loop = EventLoop::with_user_event();

    let background = app.spawn_background(event_loop.create_proxy());
    let display = create_display(&persistence, &event_loop, title);

    let repaint_signal = RepaintSignal {
        proxy: std::sync::Mutex::new(event_loop.create_proxy()),
        pd: PhantomData,
    };

    let mut integration = Integration::new(
        title,
        egui_glium::EguiGlium::new(&display),
        app,
        Arc::new(repaint_signal),
        background,
    );

    let mut last_image_cleanup = Instant::now();

    event_loop.run(move |event, _, control_flow| {
        let mut redraw = || {
            if last_image_cleanup.elapsed().as_secs() >= 1 {
                image::cleanup(&integration.frame);
                last_image_cleanup = Instant::now();
            }
            let (needs_repaint, mut tex_allocation_data, shapes) =
                integration.update(display.gl_window().window());
            let clipped_meshes = integration.egui_glium.egui_ctx.tessellate(shapes);

            let painter = &mut integration.egui_glium.painter;

            for (id, image) in tex_allocation_data.creations {
                painter.set_texture(&display, id, &image);
            }
            {
                use glium::Surface as _;
                let mut target = display.draw();
                let color: f32 = 3.0 / 255.0;
                target.clear_color(color, color, color, 1.0);

                painter.paint_meshes(
                    &display,
                    &mut target,
                    integration.egui_glium.egui_ctx.pixels_per_point(),
                    clipped_meshes,
                    &integration.egui_glium.egui_ctx.font_image(),
                );

                target.finish().unwrap();
            }

            for id in tex_allocation_data.destructions.drain(..) {
                log::info!(target: "image", "Destroying texture {}", id);
                painter.free_texture(id);
            }

            *control_flow = if !integration.app.is_running() {
                ControlFlow::Exit
            } else if needs_repaint {
                display.gl_window().window().request_redraw();
                ControlFlow::Poll
            } else {
                ControlFlow::Wait
            };
        };

        match event {
            // Platform-dependent event handlers to workaround a winit bug
            // See: https://github.com/rust-windowing/winit/issues/987
            // See: https://github.com/rust-windowing/winit/issues/1619
            Event::RedrawEventsCleared if cfg!(windows) => redraw(),
            Event::RedrawRequested(_) if !cfg!(windows) => redraw(),

            Event::WindowEvent { event, .. } => {
                if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {
                    *control_flow = glutin::event_loop::ControlFlow::Exit;
                }

                let egui_consumed = integration.egui_glium.on_event(&event);
                if !egui_consumed {
                    match event {
                        WindowEvent::KeyboardInput { input, .. } => {
                            match (input.virtual_keycode, input.state) {
                                (Some(virtual_keycode), ElementState::Pressed) => integration
                                    .app
                                    .key_pressed(&mut integration.background, virtual_keycode),
                                (Some(virtual_keycode), ElementState::Released) => integration
                                    .app
                                    .key_released(&mut integration.background, virtual_keycode),
                                _ => {}
                            }
                        }
                        e => log::trace!(target: "Event", "Unhandled {:?}", e),
                    }
                }

                display.gl_window().window().request_redraw();
            }

            glutin::event::Event::UserEvent(e) if e.is_repaint_signal() => {
                display.gl_window().window().request_redraw();
            }
            glutin::event::Event::UserEvent(msg) => {
                if let Some(img) = msg.is_image_loaded_response() {
                    img.finish_load(&mut integration.frame);
                } else {
                    integration
                        .app
                        .handle_message(&mut integration.background, msg);
                    display.gl_window().window().request_redraw();
                }
            }

            _ => (),
        }
        persistence.maybe_autosave(&display);
    });
}

pub struct Persistence {
    storage: Option<FileStorage>,
    last_auto_save: std::time::Instant,
}

impl Persistence {
    const WINDOW_KEY: &'static str = "window";
    const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(5 * 60);

    pub fn from_app_name(app_name: &str) -> Self {
        Self {
            storage: FileStorage::from_app_name(app_name),
            last_auto_save: std::time::Instant::now(),
        }
    }

    pub fn save(&mut self, display: &glium::Display) {
        if let Some(storage) = &mut self.storage {
            epi::set_value(
                storage,
                Self::WINDOW_KEY,
                &WindowSettings::from_display(display.gl_window().window()),
            );
            storage.flush();
        }
    }

    pub fn maybe_autosave(&mut self, display: &glium::Display) {
        let now = std::time::Instant::now();
        if now - self.last_auto_save > Self::AUTO_SAVE_INTERVAL {
            self.save(display);
            self.last_auto_save = now;
        }
    }

    pub fn load_window_settings(&self) -> Option<crate::WindowSettings> {
        epi::get_value(self.storage.as_ref()?, Self::WINDOW_KEY)
    }
}

pub struct Context<'a, BG> {
    pub ctx: &'a egui::CtxRef,
    pub frame: &'a epi::Frame,
    pub background: &'a mut BG,
}

pub struct Integration<APP: App> {
    frame: epi::Frame,
    background: <APP as App>::Background,
    pub egui_glium: egui_glium::EguiGlium,
    pub app: APP,
}

impl<APP: App> Integration<APP> {
    fn new(
        title: &'static str,
        egui_glium: egui_glium::EguiGlium,
        app: APP,
        repaint_signal: Arc<dyn epi::backend::RepaintSignal>,
        background: <APP as App>::Background,
    ) -> Self {
        let frame = epi::Frame::new(epi::backend::FrameData {
            info: epi::IntegrationInfo {
                name: title,
                web_info: None,
                prefer_dark_mode: Some(true),
                cpu_usage: None,
                native_pixels_per_point: Some(egui_glium.egui_winit.pixels_per_point()),
            },
            output: Default::default(),
            repaint_signal,
        });
        Self {
            frame,
            egui_glium,
            app,
            background,
        }
    }

    pub fn update(
        &mut self,
        window: &Window,
    ) -> (
        bool,
        epi::backend::TexAllocationData,
        Vec<egui::epaint::ClippedShape>,
    ) {
        let frame_start = std::time::Instant::now();

        let raw_input = self.egui_glium.egui_winit.take_egui_input(window);
        let (egui_output, shapes) = self.egui_glium.egui_ctx.run(raw_input, |egui_ctx| {
            self.app.draw(&mut Context {
                ctx: egui_ctx,
                frame: &mut self.frame,
                background: &mut self.background,
            });
        });

        let needs_repaint = egui_output.needs_repaint;
        self.egui_glium
            .egui_winit
            .handle_output(window, &self.egui_glium.egui_ctx, egui_output);

        let app_output = self.frame.take_app_output();
        let tex_allocation_data = egui_glium::egui_winit::epi::handle_app_output(
            window,
            self.egui_glium.egui_ctx.pixels_per_point(),
            app_output,
        );

        let frame_time = (std::time::Instant::now() - frame_start).as_secs_f64() as f32;
        self.frame.lock().info.cpu_usage = Some(frame_time);

        (needs_repaint, tex_allocation_data, shapes)
    }
}

fn create_display<MSG>(
    persistence: &Persistence,
    event_loop: &glutin::event_loop::EventLoop<MSG>,
    title: &str,
) -> glium::Display {
    let window_settings = persistence.load_window_settings();
    let window_builder = egui_glium::egui_winit::epi::window_builder(
        &epi::NativeOptions {
            maximized: true,
            ..Default::default()
        },
        &window_settings,
    )
    .with_title(title);
    let context_builder = glutin::ContextBuilder::new()
        .with_depth_buffer(0)
        .with_srgb(true)
        .with_stencil_buffer(0)
        .with_vsync(true);

    glium::Display::new(window_builder, context_builder, event_loop).unwrap()
}
