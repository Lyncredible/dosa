use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{Fullscreen, Theme, Window, WindowId};

mod audio;
mod input;
mod menu;
mod pointer;
mod renderer;

pub use input::{Input, Key, Modifiers};
pub use menu::{CpuMode, MenuAction, MenuState, MountKind, Scaling, Speed, TextMode, Volume};
pub use pointer::{PointerEvent, PointerMode};
use renderer::Renderer;

const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// An emulator controlled by the native desktop event loop.
pub trait App {
    /// Advance the emulator by one time slice. The desktop calls this at up to 60 Hz.
    fn tick(&mut self);

    fn frame(&self) -> Frame<'_>;

    fn input(&mut self, event: Input);

    /// Drain mono 48 kHz audio produced by the guest since the previous tick.
    fn audio(&mut self) -> Vec<f32> {
        Vec::new()
    }

    fn audio_error(&mut self, _error: &str) {}

    fn pointer(&mut self, _event: PointerEvent) {}

    fn pointer_mode(&self) -> PointerMode {
        PointerMode::Host
    }

    /// Controls and current guest settings shown in the desktop toolbar.
    fn menu(&self) -> MenuState {
        MenuState::default()
    }

    fn menu_action(&mut self, _action: MenuAction) {}

    fn should_exit(&self) -> bool {
        false
    }

    /// Override the display aspect for modes with non-square pixels.
    fn display_aspect(&self) -> Option<f32> {
        None
    }
}

/// A tightly packed, top-to-bottom RGBA8 framebuffer.
pub struct Frame<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// Run the native event loop on the calling thread until the window closes.
pub fn run(app: impl App + 'static) -> Result<(), Box<dyn Error>> {
    let event_loop = EventLoop::new()?;
    let mut desktop = Desktop {
        app,
        window: None,
        renderer: None,
        next_tick: Instant::now(),
        keyboard: input::Keyboard::default(),
        pointer: pointer::Pointer::default(),
        menu: menu::Menu::default(),
        menu_keys: HashSet::new(),
        error: None,
        audio: None,
        audio_attempted: false,
    };

    event_loop.run_app(&mut desktop)?;
    match desktop.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct Desktop<A> {
    app: A,
    audio: Option<audio::Audio>,
    audio_attempted: bool,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    next_tick: Instant,
    keyboard: input::Keyboard,
    pointer: pointer::Pointer,
    menu: menu::Menu,
    menu_keys: HashSet<PhysicalKey>,
    error: Option<Box<dyn Error>>,
}

impl<A: App> Desktop<A> {
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), Box<dyn Error>> {
        let window = match &self.window {
            Some(window) => Arc::clone(window),
            None => Arc::new(
                event_loop.create_window(
                    Window::default_attributes()
                        .with_title("Dosa")
                        .with_window_icon(window_icon())
                        .with_theme(Some(Theme::Dark))
                        .with_inner_size(LogicalSize::new(928, 624))
                        .with_min_inner_size(LogicalSize::new(384, 360))
                        .with_visible(false),
                )?,
            ),
        };
        self.renderer = Some(pollster::block_on(Renderer::new(
            Arc::clone(&window),
            event_loop.owned_display_handle(),
        ))?);
        window.set_visible(true);
        self.pointer.set_focus(window.has_focus());
        window.request_redraw();
        self.window = Some(window);
        self.next_tick = Instant::now();
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: Box<dyn Error>) {
        self.error = Some(error);
        event_loop.exit();
    }

    fn use_menu_command(&mut self, window: &Window, command: Option<menu::Command>) {
        match command {
            Some(menu::Command::QuickLaunch) => {
                self.pointer.set_position(None);
                self.sync_pointer(window, true);
                if let Some(path) = rfd::FileDialog::new()
                    .set_parent(window)
                    .set_title("Launch a DOS program")
                    .add_filter("DOS programs", &["com", "exe", "bat"])
                    .pick_file()
                {
                    self.app.menu_action(MenuAction::QuickLaunch(path));
                }
            }
            Some(menu::Command::Screenshot) => {
                self.pointer.set_position(None);
                self.sync_pointer(window, true);
                if let Some(path) = rfd::FileDialog::new()
                    .set_parent(window)
                    .add_filter("PNG image", &["png"])
                    .set_file_name("dosa.png")
                    .save_file()
                {
                    self.app.menu_action(MenuAction::SaveScreenshot(path));
                }
            }
            Some(menu::Command::RecordAudio) => {
                self.pointer.set_position(None);
                self.sync_pointer(window, true);
                if let Some(path) = rfd::FileDialog::new()
                    .set_parent(window)
                    .set_title("Record DOS audio")
                    .add_filter("WAV audio", &["wav"])
                    .set_file_name("dosa.wav")
                    .save_file()
                {
                    self.app.menu_action(MenuAction::StartRecording(path));
                }
            }
            Some(menu::Command::Mount(kind, drive)) => {
                self.pointer.set_position(None);
                self.sync_pointer(window, true);
                if let Some(source) = menu::pick_path(window, kind) {
                    self.app.menu_action(MenuAction::Mount {
                        drive,
                        source,
                        kind,
                    });
                }
            }
            Some(menu::Command::Fullscreen) => {
                window.set_fullscreen(if window.fullscreen().is_some() {
                    None
                } else {
                    Some(Fullscreen::Borderless(None))
                });
            }
            Some(menu::Command::App(action)) => self.app.menu_action(action),
            None => {}
        }
        window.request_redraw();
    }

    fn sync_pointer(&mut self, window: &Window, blocked: bool) {
        let blocked = blocked || self.menu.blocks_pointer() && self.app.menu().enabled;
        let mode = if blocked {
            PointerMode::Host
        } else {
            self.app.pointer_mode()
        };
        let viewport = self
            .renderer
            .as_ref()
            .filter(|_| mode != PointerMode::Host)
            .map(Renderer::presented_viewport);
        if let Some(event) = self.pointer.update(window, mode, viewport) {
            self.app.pointer(event);
        }
    }
}

fn window_icon() -> Option<winit::window::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/dosa.png")));
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    if frame.color_type != png::ColorType::Rgba || frame.bit_depth != png::BitDepth::Eight {
        return None;
    }
    pixels.truncate(frame.buffer_size());
    winit::window::Icon::from_rgba(pixels, frame.width, frame.height).ok()
}

#[cfg(test)]
#[test]
fn bundled_window_icon_decodes_as_rgba() {
    assert!(window_icon().is_some());
}

impl<A: App> ApplicationHandler for Desktop<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_none()
            && let Err(error) = self.open(event_loop)
        {
            self.fail(event_loop, error);
        }
    }

    fn suspended(&mut self, _: &ActiveEventLoop) {
        self.pointer.set_focus(false);
        if let Some(window) = self.window.clone() {
            self.sync_pointer(&window, true);
        }
        self.renderer = None;
        self.menu.close();
        self.menu.leave();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self
            .window
            .as_ref()
            .filter(|window| window.id() == window_id)
            .cloned()
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                self.sync_pointer(&window, true);
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size);
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => window.request_redraw(),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.keyboard.set_modifiers(modifiers.state())
            }
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } => {
                if !is_synthetic {
                    let physical = event.physical_key;
                    if event.state.is_pressed() {
                        if event.repeat && self.menu_keys.contains(&physical) {
                            return;
                        }
                        self.menu_keys.remove(&physical);
                    } else if self.menu_keys.remove(&physical) {
                        return;
                    }
                    let input = self.keyboard.translate(event);
                    let state = self.app.menu();
                    let (handled, command) = if state.enabled {
                        self.menu.key(&input, &state, window.fullscreen().is_some())
                    } else {
                        (false, None)
                    };
                    if handled {
                        self.menu_keys.insert(physical);
                        self.keyboard.forget(physical);
                        self.use_menu_command(&window, command);
                    } else {
                        self.app.input(input);
                    }
                    if self.app.should_exit() {
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.menu.move_pointer(position, window.scale_factor());
                self.pointer.set_position(Some(position));
                window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.menu.leave();
                self.pointer.set_position(None);
                window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let menu_state = self.app.menu();
                let (handled, command) = if menu_state.enabled {
                    self.menu.mouse(
                        state.is_pressed(),
                        button == MouseButton::Left,
                        &menu_state,
                        window.fullscreen().is_some(),
                    )
                } else {
                    (false, None)
                };
                self.sync_pointer(&window, handled);
                if handled {
                    self.use_menu_command(&window, command);
                } else if let Some(event) = self.pointer.button(button, state) {
                    self.app.pointer(event);
                }
                if self.app.should_exit() {
                    event_loop.exit();
                }
                return;
            }
            WindowEvent::Focused(focused) => {
                self.pointer.set_focus(focused);
                if !focused {
                    self.menu.close();
                    for event in self.keyboard.release_all() {
                        self.app.input(event);
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = &mut self.renderer {
                    let state = self.app.menu();
                    let overlay = state.enabled.then(|| {
                        self.menu.paint(
                            window.inner_size(),
                            window.scale_factor(),
                            &state,
                            window.fullscreen().is_some(),
                        )
                    });
                    if let Err(error) = renderer.draw(
                        &window,
                        self.app.frame(),
                        self.app.display_aspect(),
                        state.scaling,
                        overlay,
                    ) {
                        self.fail(event_loop, error);
                    }
                }
            }
            _ => {}
        }
        self.sync_pointer(&window, false);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if self.renderer.is_none() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }

        let now = Instant::now();
        if now >= self.next_tick {
            self.app.tick();
            let samples = self.app.audio();
            if !samples.is_empty() && !self.audio_attempted {
                self.audio_attempted = true;
                match audio::Audio::open() {
                    Ok(audio) => self.audio = Some(audio),
                    Err(error) => self.app.audio_error(&error.to_string()),
                }
            }
            if let Some(audio) = &self.audio {
                let menu = self.app.menu();
                if menu.enabled && (menu.paused || !menu.running) {
                    audio.clear();
                } else {
                    audio.push(samples, menu.audio_gain());
                }
            }
            self.sync_pointer(&window, false);
            if self.app.should_exit() {
                event_loop.exit();
                return;
            }
            window.request_redraw();
            self.next_tick += FRAME_INTERVAL;
            if self.next_tick <= now {
                self.next_tick = now + FRAME_INTERVAL;
            }
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_tick));
    }
}
