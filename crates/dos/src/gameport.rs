use std::time::Duration;

pub(crate) const PORT: u16 = 0x201;

/// An analog joystick with two axes from 0 to 255 and two trigger buttons.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JoystickState {
    pub x: u8,
    pub y: u8,
    pub buttons: [bool; 2],
}

impl Default for JoystickState {
    fn default() -> Self {
        Self {
            x: 128,
            y: 128,
            buttons: [false; 2],
        }
    }
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct GamePort {
    sticks: [Option<JoystickState>; 2],
    deadlines: [[Duration; 2]; 2],
}

impl GamePort {
    pub(crate) fn set(&mut self, connector: usize, state: Option<JoystickState>) {
        if let Some(stick) = self.sticks.get_mut(connector) {
            if stick.is_some() != state.is_some() {
                self.deadlines[connector] = [Duration::ZERO; 2];
            }
            *stick = state;
        }
    }

    pub(crate) fn reset_timers(&mut self) {
        self.deadlines = [[Duration::ZERO; 2]; 2];
    }

    pub(crate) fn connected(&self) -> bool {
        self.sticks.iter().any(Option::is_some)
    }

    pub(crate) fn positions(&self) -> [u16; 4] {
        let [a, b] = self.sticks.map(|state| {
            state.map_or([0, 0], |stick| {
                [u16::from(stick.x.max(1)), u16::from(stick.y.max(1))]
            })
        });
        [a[0], a[1], b[0], b[1]]
    }

    pub(crate) fn buttons(&self) -> u8 {
        let mut switches = 0xf0;
        for (connector, stick) in self.sticks.iter().enumerate() {
            if let Some(stick) = stick {
                for (button, pressed) in stick.buttons.into_iter().enumerate() {
                    switches &= !(u8::from(pressed) << (4 + connector * 2 + button));
                }
            }
        }
        switches
    }

    pub(crate) fn read(&self, now: Duration) -> u8 {
        let mut value = self.buttons() | 0x0f;
        for (connector, stick) in self.sticks.iter().enumerate() {
            if stick.is_some() {
                for (axis, deadline) in self.deadlines[connector].iter().enumerate() {
                    value &= !(u8::from(now >= *deadline) << (connector * 2 + axis));
                }
            }
        }
        value
    }

    pub(crate) fn strobe(&mut self, now: Duration) {
        for (stick, deadlines) in self.sticks.iter().zip(&mut self.deadlines) {
            if let Some(stick) = stick {
                *deadlines = [stick.x, stick.y].map(|axis| now + pulse(axis));
            }
        }
    }
}

fn pulse(axis: u8) -> Duration {
    // IBM's 0–100 kΩ potentiometer produces a 24.2–1124.2 µs pulse.
    Duration::from_nanos(24_200 + u64::from(axis) * 1_100_000 / 255)
}

#[cfg(test)]
#[test]
fn strobe_latches_independent_axes_but_buttons_update_immediately() {
    let mut port = GamePort::default();
    let start = Duration::from_secs(1);
    assert_eq!(port.read(start), 0xff);
    port.set(
        0,
        Some(JoystickState {
            x: 0,
            y: 255,
            buttons: [true, false],
        }),
    );
    port.set(1, Some(JoystickState::default()));
    assert_eq!(port.read(start), 0xe0);
    port.strobe(start);
    assert_eq!(port.read(start), 0xef);
    port.set(0, Some(JoystickState::default()));
    assert_eq!(port.read(start + pulse(0)), 0xfe);
    assert_eq!(port.read(start + pulse(128)), 0xf2);
    assert_eq!(port.read(start + pulse(255)), 0xf0);

    port.strobe(start);
    port.set(1, None);
    assert_eq!(port.read(start + pulse(128)), 0xfc);
}
