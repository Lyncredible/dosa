use std::collections::HashMap;

use winit::event::KeyEvent;
use winit::keyboard::{Key as LogicalKey, KeyCode, ModifiersState, NamedKey, PhysicalKey};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Function(u8),
    Character(char),
    Other,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    pub key: Key,
    pub pressed: bool,
    pub text: Option<String>,
    pub modifiers: Modifiers,
    /// Low byte of the XT set-1 make code, or zero when unknown.
    pub scan_code: u8,
}

#[derive(Default)]
pub(crate) struct Keyboard {
    modifiers: Modifiers,
    held: HashMap<PhysicalKey, (Key, u8)>,
}

impl Keyboard {
    pub(crate) fn set_modifiers(&mut self, state: ModifiersState) {
        self.modifiers = Modifiers {
            shift: state.shift_key(),
            ctrl: state.control_key(),
            alt: state.alt_key(),
            logo: state.super_key(),
        };
    }

    pub(crate) fn translate(&mut self, event: KeyEvent) -> Input {
        let pressed = event.state.is_pressed();
        let mut key = logical_key(&event.logical_key);
        let mut scan_code = match event.physical_key {
            PhysicalKey::Code(code) => xt_scan_code(code),
            PhysicalKey::Unidentified(_) => 0,
        };
        if pressed {
            self.held.insert(event.physical_key, (key, scan_code));
        } else if let Some(original) = self.held.remove(&event.physical_key) {
            (key, scan_code) = original;
        }
        Input {
            key,
            scan_code,
            pressed,
            text: event.text.filter(|_| pressed).map(|text| text.to_string()),
            modifiers: self.modifiers,
        }
    }

    pub(crate) fn release_all(&mut self) -> impl Iterator<Item = Input> + '_ {
        self.modifiers = Modifiers::default();
        self.held.drain().map(|(_, (key, scan_code))| Input {
            key,
            scan_code,
            pressed: false,
            text: None,
            modifiers: Modifiers::default(),
        })
    }
}

fn logical_key(key: &LogicalKey) -> Key {
    match key {
        LogicalKey::Character(text) => text.chars().next().map_or(Key::Other, Key::Character),
        LogicalKey::Named(named) => match named {
            NamedKey::Enter => Key::Enter,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Insert => Key::Insert,
            NamedKey::Delete => Key::Delete,
            NamedKey::Space => Key::Character(' '),
            NamedKey::F1 => Key::Function(1),
            NamedKey::F2 => Key::Function(2),
            NamedKey::F3 => Key::Function(3),
            NamedKey::F4 => Key::Function(4),
            NamedKey::F5 => Key::Function(5),
            NamedKey::F6 => Key::Function(6),
            NamedKey::F7 => Key::Function(7),
            NamedKey::F8 => Key::Function(8),
            NamedKey::F9 => Key::Function(9),
            NamedKey::F10 => Key::Function(10),
            NamedKey::F11 => Key::Function(11),
            NamedKey::F12 => Key::Function(12),
            _ => Key::Other,
        },
        _ => Key::Other,
    }
}

fn xt_scan_code(code: KeyCode) -> u8 {
    use KeyCode::*;

    match code {
        Escape => 0x01,
        Digit1 => 0x02,
        Digit2 => 0x03,
        Digit3 => 0x04,
        Digit4 => 0x05,
        Digit5 => 0x06,
        Digit6 => 0x07,
        Digit7 => 0x08,
        Digit8 => 0x09,
        Digit9 => 0x0A,
        Digit0 => 0x0B,
        Minus => 0x0C,
        Equal => 0x0D,
        Backspace => 0x0E,
        Tab => 0x0F,
        KeyQ => 0x10,
        KeyW => 0x11,
        KeyE => 0x12,
        KeyR => 0x13,
        KeyT => 0x14,
        KeyY => 0x15,
        KeyU => 0x16,
        KeyI => 0x17,
        KeyO => 0x18,
        KeyP => 0x19,
        BracketLeft => 0x1A,
        BracketRight => 0x1B,
        Enter | NumpadEnter => 0x1C,
        ControlLeft | ControlRight => 0x1D,
        KeyA => 0x1E,
        KeyS => 0x1F,
        KeyD => 0x20,
        KeyF => 0x21,
        KeyG => 0x22,
        KeyH => 0x23,
        KeyJ => 0x24,
        KeyK => 0x25,
        KeyL => 0x26,
        Semicolon => 0x27,
        Quote => 0x28,
        Backquote => 0x29,
        ShiftLeft => 0x2A,
        Backslash => 0x2B,
        KeyZ => 0x2C,
        KeyX => 0x2D,
        KeyC => 0x2E,
        KeyV => 0x2F,
        KeyB => 0x30,
        KeyN => 0x31,
        KeyM => 0x32,
        Comma => 0x33,
        Period => 0x34,
        Slash | NumpadDivide => 0x35,
        ShiftRight => 0x36,
        NumpadMultiply => 0x37,
        AltLeft | AltRight => 0x38,
        Space => 0x39,
        CapsLock => 0x3A,
        F1 => 0x3B,
        F2 => 0x3C,
        F3 => 0x3D,
        F4 => 0x3E,
        F5 => 0x3F,
        F6 => 0x40,
        F7 => 0x41,
        F8 => 0x42,
        F9 => 0x43,
        F10 => 0x44,
        NumLock => 0x45,
        ScrollLock => 0x46,
        Home | Numpad7 => 0x47,
        ArrowUp | Numpad8 => 0x48,
        PageUp | Numpad9 => 0x49,
        NumpadSubtract => 0x4A,
        ArrowLeft | Numpad4 => 0x4B,
        Numpad5 => 0x4C,
        ArrowRight | Numpad6 => 0x4D,
        NumpadAdd => 0x4E,
        End | Numpad1 => 0x4F,
        ArrowDown | Numpad2 => 0x50,
        PageDown | Numpad3 => 0x51,
        Insert | Numpad0 => 0x52,
        Delete | NumpadDecimal => 0x53,
        IntlBackslash => 0x56,
        F11 => 0x57,
        F12 => 0x58,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_and_printable_keys_have_dos_codes() {
        assert_eq!(logical_key(&LogicalKey::Named(NamedKey::ArrowUp)), Key::Up);
        assert_eq!(
            logical_key(&LogicalKey::Named(NamedKey::Space)),
            Key::Character(' ')
        );
        assert_eq!(xt_scan_code(KeyCode::ArrowUp), 0x48);
        assert_eq!(xt_scan_code(KeyCode::KeyA), 0x1e);
    }
}
