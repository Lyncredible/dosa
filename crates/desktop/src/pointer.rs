use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton};
use winit::window::{CursorGrabMode, Window};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PointerMode {
    /// Leave the native cursor visible and keep pointer events in the desktop.
    #[default]
    Host,
    /// Feed the guest while the pointer is over its display.
    Integrated,
    /// Confine the native cursor when supported, with integrated input as the fallback.
    Captured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerEvent {
    /// Coordinates normalized to the guest viewport, with 65535 at the far edge.
    Moved { x: u16, y: u16 },
    /// The left, right and middle button state in bits 0, 1 and 2.
    Buttons { mask: u8 },
    /// Guest input ended and any guest buttons that are down must be released.
    Deactivated,
}

#[derive(Default)]
pub(crate) struct Pointer {
    position: Option<PhysicalPosition<f64>>,
    focused: bool,
    forwarded: Option<(u16, u16)>,
    buttons: u8,
    grab_attempted: bool,
    confined: bool,
}

impl Pointer {
    pub(crate) fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.position = None;
        }
    }

    pub(crate) fn set_position(&mut self, position: Option<PhysicalPosition<f64>>) {
        self.position = position;
    }

    pub(crate) fn update(
        &mut self,
        window: &Window,
        mode: PointerMode,
        viewport: Option<[f32; 4]>,
    ) -> Option<PointerEvent> {
        let wants_grab = mode == PointerMode::Captured;
        if !wants_grab {
            self.release_grab(window);
        }
        let position = if self.focused && mode != PointerMode::Host {
            self.position
                .zip(viewport)
                .and_then(|(position, viewport)| normalize(position, viewport, self.confined))
        } else {
            None
        };

        let Some((x, y)) = position else {
            self.release_grab(window);
            let event = self.stop_forwarding();
            if event.is_some() {
                window.set_cursor_visible(true);
            }
            return event;
        };

        if wants_grab && !self.grab_attempted {
            self.grab_attempted = true;
            self.confined = window.set_cursor_grab(CursorGrabMode::Confined).is_ok();
        }
        let previous = self.forwarded.replace((x, y));
        if previous.is_none() {
            window.set_cursor_visible(false);
        }
        (previous != Some((x, y))).then_some(PointerEvent::Moved { x, y })
    }

    pub(crate) fn button(
        &mut self,
        button: MouseButton,
        state: ElementState,
    ) -> Option<PointerEvent> {
        self.forwarded?;
        let bit = match button {
            MouseButton::Left => 1,
            MouseButton::Right => 2,
            MouseButton::Middle => 4,
            _ => return None,
        };
        let previous = self.buttons;
        if state == ElementState::Pressed {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
        (previous != self.buttons).then_some(PointerEvent::Buttons { mask: self.buttons })
    }

    fn release_grab(&mut self, window: &Window) {
        if std::mem::take(&mut self.confined) {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
        }
        self.grab_attempted = false;
    }

    fn stop_forwarding(&mut self) -> Option<PointerEvent> {
        self.buttons = 0;
        self.forwarded.take().map(|_| PointerEvent::Deactivated)
    }
}

fn normalize(
    position: PhysicalPosition<f64>,
    viewport: [f32; 4],
    confined: bool,
) -> Option<(u16, u16)> {
    let [x, y, width, height] = viewport.map(f64::from);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let x = position.x - x;
    let y = position.y - y;
    if !confined && (x < 0.0 || y < 0.0 || x >= width || y >= height) {
        return None;
    }
    let axis = |position: f64, length: f64| {
        ((position / (length - 1.0).max(1.0)).clamp(0.0, 1.0) * f64::from(u16::MAX)).round() as u16
    };
    Some((axis(x, width), axis(y, height)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_offsets_and_letterbox_are_respected() {
        let viewport = [25.0, 40.0, 320.0, 200.0];
        assert_eq!(
            normalize(PhysicalPosition::new(24.0, 40.0), viewport, false),
            None
        );
        assert_eq!(
            normalize(PhysicalPosition::new(25.0, 40.0), viewport, false),
            Some((0, 0))
        );
        assert_eq!(
            normalize(PhysicalPosition::new(344.0, 239.0), viewport, false),
            Some((65535, 65535))
        );
        assert_eq!(
            normalize(PhysicalPosition::new(500.0, 0.0), viewport, true),
            Some((65535, 0))
        );
    }

    #[test]
    fn leaving_guest_input_clears_held_buttons_once() {
        let mut pointer = Pointer {
            forwarded: Some((0, 0)),
            ..Pointer::default()
        };
        assert_eq!(
            pointer.button(MouseButton::Left, ElementState::Pressed),
            Some(PointerEvent::Buttons { mask: 1 })
        );
        assert_eq!(
            pointer.button(MouseButton::Right, ElementState::Pressed),
            Some(PointerEvent::Buttons { mask: 3 })
        );
        assert_eq!(pointer.stop_forwarding(), Some(PointerEvent::Deactivated));
        assert_eq!(pointer.stop_forwarding(), None);
        assert_eq!(
            pointer.button(MouseButton::Left, ElementState::Pressed),
            None
        );
        pointer.forwarded = Some((10, 10));
        assert_eq!(
            pointer.button(MouseButton::Middle, ElementState::Pressed),
            Some(PointerEvent::Buttons { mask: 4 })
        );
    }
}
