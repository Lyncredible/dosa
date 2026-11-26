use std::time::{Duration, Instant};

use font8x8::{BASIC_FONTS, UnicodeFonts};
use winit::{
    application::ApplicationHandler,
    dpi::PhysicalPosition,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    platform::windows::EventLoopBuilderExtWindows,
    window::WindowId,
};

use super::*;
use crate::pointer::normalize;

const WIDTH: u32 = 132 * 8;
const HEIGHT: u32 = 50 * 8;
const CASES: [(PhysicalSize<u32>, Scaling); 3] = [
    (PhysicalSize::new(800, 480), Scaling::Fit),
    (PhysicalSize::new(1360, 660), Scaling::Integer),
    (PhysicalSize::new(640, 420), Scaling::Integer),
];

#[test]
#[ignore = "requires a Windows desktop session and GPU backend"]
fn presents_wide_text_across_native_window_resizes() {
    let event_loop = EventLoop::builder().with_any_thread(true).build().unwrap();
    let mut check = ResizeCheck {
        deadline: Instant::now(),
        window: None,
        pixels: text_grid(),
        case: 0,
    };
    event_loop.run_app(&mut check).unwrap();
    assert_eq!(check.case, CASES.len());
}

struct ResizeCheck {
    deadline: Instant,
    window: Option<(Arc<Window>, Renderer)>,
    pixels: Vec<u8>,
    case: usize,
}

impl ApplicationHandler for ResizeCheck {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Dosa text resize test")
                        .with_visible(false)
                        .with_inner_size(CASES[0].0),
                )
                .unwrap(),
        );
        let renderer = pollster::block_on(Renderer::new(
            Arc::clone(&window),
            event_loop.owned_display_handle(),
        ))
        .unwrap();
        window.set_visible(true);
        self.window = Some((window, renderer));
        self.deadline = Instant::now() + Duration::from_secs(15);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some((window, renderer)) = &mut self.window else {
            return;
        };
        assert_eq!(id, window.id());
        match event {
            WindowEvent::Resized(size) => renderer.resize(size),
            WindowEvent::RedrawRequested if self.case < CASES.len() => {
                let (size, scaling) = CASES[self.case];
                if renderer.size != size || window.inner_size() != size {
                    return;
                }
                renderer
                    .draw(
                        window,
                        Frame {
                            width: WIDTH,
                            height: HEIGHT,
                            rgba: &self.pixels,
                        },
                        None,
                        scaling,
                        None,
                        false,
                    )
                    .unwrap();
                let [x, y, width, height] = renderer.presented_viewport();
                if width == 0.0 {
                    return;
                }
                let frame = renderer.framebuffer.as_ref().unwrap();
                assert_eq!((frame.extent.width, frame.extent.height), (WIDTH, HEIGHT));
                assert!((width - height * WIDTH as f32 / HEIGHT as f32).abs() < 2.0);
                assert!((x * 2.0 + width - size.width as f32).abs() <= 1.0);
                assert!((y * 2.0 + height - size.height as f32).abs() <= 1.0);
                if self.case == 1 {
                    assert_eq!((width, height), (WIDTH as f32, HEIGHT as f32));
                } else {
                    assert!(width < WIDTH as f32);
                }
                assert_eq!(
                    normalize(
                        PhysicalPosition::new(
                            f64::from(x + width - 1.0),
                            f64::from(y + height - 1.0)
                        ),
                        renderer.presented_viewport(),
                        false,
                    ),
                    Some((u16::MAX, u16::MAX))
                );
                assert_eq!(
                    normalize(
                        PhysicalPosition::new(f64::from(x + width), f64::from(y)),
                        renderer.presented_viewport(),
                        false,
                    ),
                    None
                );
                self.case += 1;
                if let Some(&(next_size, _)) = CASES.get(self.case) {
                    if let Some(actual) = window.request_inner_size(next_size) {
                        renderer.resize(actual);
                    }
                } else {
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some((window, renderer)) = &self.window {
            let now = Instant::now();
            assert!(
                now < self.deadline || self.case == CASES.len(),
                "resize case {} stalled at {:?}",
                self.case,
                renderer.size
            );
            window.request_redraw();
            event_loop.set_control_flow(ControlFlow::WaitUntil(now + Duration::from_millis(16)));
        }
    }
}

fn text_grid() -> Vec<u8> {
    let glyphs = ['D', 'O', 'S', 'A'].map(|letter| BASIC_FONTS.get(letter).unwrap());
    let mut pixels = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let glyph = glyphs[((x / 8 + y / 8) % 4) as usize];
            let ink = glyph[(y % 8) as usize] & (1 << (x % 8)) != 0;
            pixels.extend_from_slice(if ink {
                &[205, 212, 224, 255]
            } else {
                &[17, 20, 28, 255]
            });
        }
    }
    pixels
}
