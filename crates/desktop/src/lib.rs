use std::collections::HashSet;
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as LogicalKey, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Fullscreen, Theme, Window, WindowId};

mod input;
mod menu;
mod renderer;

pub use input::{Input, Key, Modifiers};
pub use menu::{CpuMode, MenuAction, MenuState, MountKind, Speed, TextMode};
use renderer::Renderer;

const FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// An emulator controlled by the native desktop event loop.
pub trait App {
    /// Advance the emulator by one time slice. The desktop calls this at up to 60 Hz.
    fn tick(&mut self);

    fn frame(&self) -> Frame<'_>;

    fn input(&mut self, event: Input);

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
        menu: menu::Menu::default(),
        menu_keys: HashSet::new(),
        error: None,
    };

    event_loop.run_app(&mut desktop)?;
    match desktop.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct Desktop<A> {
    app: A,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    next_tick: Instant,
    keyboard: input::Keyboard,
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
                        .with_theme(Some(Theme::Dark))
                        .with_inner_size(LogicalSize::new(928, 624))
                        .with_min_inner_size(LogicalSize::new(384, 272))
                        .with_visible(false),
                )?,
            ),
        };
        self.renderer = Some(pollster::block_on(Renderer::new(
            Arc::clone(&window),
            event_loop.owned_display_handle(),
        ))?);
        window.set_visible(true);
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
            Some(menu::Command::Mount(kind, drive)) => {
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
            WindowEvent::CloseRequested => event_loop.exit(),
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
                    let shortcut = match event.key_without_modifiers() {
                        LogicalKey::Character(text) => text.chars().next(),
                        _ => None,
                    };
                    let input = self.keyboard.translate(event);
                    let state = self.app.menu();
                    let (handled, command) = if state.enabled {
                        self.menu
                            .key(&input, shortcut, &state, window.fullscreen().is_some())
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
                window.request_redraw();
            }
            WindowEvent::CursorLeft { .. } => {
                self.menu.leave();
                window.request_redraw();
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let menu_state = self.app.menu();
                if menu_state.enabled {
                    let (_, command) = self.menu.mouse(
                        state.is_pressed(),
                        button == MouseButton::Left,
                        menu::logical_width(window.inner_size(), window.scale_factor()),
                        &menu_state,
                        window.fullscreen().is_some(),
                    );
                    self.use_menu_command(&window, command);
                    if self.app.should_exit() {
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::Focused(false) => {
                self.menu.close();
                for event in self.keyboard.release_all() {
                    self.app.input(event);
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
                        overlay,
                    ) {
                        self.fail(event_loop, error);
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(window) = &self.window else {
            return;
        };
        if self.renderer.is_none() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }

        let now = Instant::now();
        if now >= self.next_tick {
            self.app.tick();
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
