use std::time::{Duration, Instant};

use windows_sys::Win32::UI::Input::XboxController::{
    XINPUT_GAMEPAD, XINPUT_GAMEPAD_A, XINPUT_GAMEPAD_B, XINPUT_GAMEPAD_DPAD_DOWN,
    XINPUT_GAMEPAD_DPAD_LEFT, XINPUT_GAMEPAD_DPAD_RIGHT, XINPUT_GAMEPAD_DPAD_UP,
    XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE, XINPUT_STATE, XInputGetState, XUSER_MAX_COUNT,
};

use crate::GamepadState;

// XInput recommends polling empty slots at a lower rate than connected controllers.
const DISCOVERY_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Default)]
pub(crate) struct Gamepads {
    slots: [Option<u32>; 2],
    next_discovery: Option<Instant>,
}

impl Gamepads {
    pub(crate) fn read(&mut self, now: Instant, focused: bool) -> [Option<GamepadState>; 2] {
        self.poll(now, query).map(|state| {
            state.map(|state| {
                if focused {
                    state
                } else {
                    GamepadState::default()
                }
            })
        })
    }

    fn poll(
        &mut self,
        now: Instant,
        mut query: impl FnMut(u32) -> Option<GamepadState>,
    ) -> [Option<GamepadState>; 2] {
        let previous = self.slots;
        let mut states = std::array::from_fn(|connector| {
            let user = self.slots[connector]?;
            let state = query(user);
            if state.is_none() {
                self.slots[connector] = None;
            }
            state
        });
        if states.iter().all(Option::is_some) || self.next_discovery.is_some_and(|next| now < next)
        {
            return states;
        }
        self.next_discovery = Some(now + DISCOVERY_INTERVAL);
        for user in 0..XUSER_MAX_COUNT {
            let Some(connector) = self.slots.iter().position(Option::is_none) else {
                break;
            };
            if !previous.contains(&Some(user))
                && let Some(state) = query(user)
            {
                self.slots[connector] = Some(user);
                states[connector] = Some(state);
            }
        }
        states
    }
}

fn query(user: u32) -> Option<GamepadState> {
    let mut state = XINPUT_STATE::default();
    // XInput writes one XINPUT_STATE into the supplied writable structure.
    (unsafe { XInputGetState(user, &mut state) } == 0).then(|| translate(state.Gamepad))
}

fn translate(pad: XINPUT_GAMEPAD) -> GamepadState {
    let held = |button| pad.wButtons & button != 0;
    GamepadState {
        x: axis(
            i32::from(pad.sThumbLX),
            held(XINPUT_GAMEPAD_DPAD_LEFT),
            held(XINPUT_GAMEPAD_DPAD_RIGHT),
        ),
        y: axis(
            -i32::from(pad.sThumbLY),
            held(XINPUT_GAMEPAD_DPAD_UP),
            held(XINPUT_GAMEPAD_DPAD_DOWN),
        ),
        buttons: [held(XINPUT_GAMEPAD_A), held(XINPUT_GAMEPAD_B)],
    }
}

fn axis(raw: i32, negative: bool, positive: bool) -> u8 {
    let deadzone = i32::from(XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE);
    match (negative, positive) {
        (true, false) => 0,
        (false, true) => 255,
        (true, true) => 128,
        _ if raw < -deadzone => (128 - 128 * (-raw - deadzone) / (32767 - deadzone)).max(0) as u8,
        _ if raw > deadzone => (128 + 127 * (raw - deadzone) / (32767 - deadzone)).min(255) as u8,
        _ => 128,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamepads_filter_drift_invert_y_and_map_buttons() {
        let mut pad = XINPUT_GAMEPAD {
            sThumbLX: 4500,
            sThumbLY: i16::MAX,
            wButtons: XINPUT_GAMEPAD_A,
            ..Default::default()
        };
        assert_eq!(
            translate(pad),
            GamepadState {
                x: 128,
                y: 0,
                buttons: [true, false],
            },
        );
        pad.sThumbLX = i16::MIN;
        pad.sThumbLY = i16::MIN;
        pad.wButtons = XINPUT_GAMEPAD_B;
        assert_eq!(
            translate(pad),
            GamepadState {
                x: 0,
                y: 255,
                buttons: [false, true],
            },
        );
        pad.wButtons |= XINPUT_GAMEPAD_DPAD_RIGHT | XINPUT_GAMEPAD_DPAD_UP;
        assert_eq!(translate(pad).x, 255);
        assert_eq!(translate(pad).y, 0);
    }

    #[test]
    fn gamepad_slots_survive_another_controller_disconnect() {
        let start = Instant::now();
        let mut pads = Gamepads::default();
        let first = GamepadState {
            x: 50,
            ..Default::default()
        };
        let second = GamepadState {
            x: 150,
            ..Default::default()
        };
        let third = GamepadState {
            x: 250,
            ..Default::default()
        };
        assert_eq!(
            pads.poll(start, |user| [None, Some(first), None, Some(second)]
                [user as usize]),
            [Some(first), Some(second)],
        );
        assert_eq!(
            pads.poll(start + Duration::from_millis(10), |user| {
                [Some(third), None, None, Some(second)][user as usize]
            }),
            [None, Some(second)],
        );
        assert_eq!(
            pads.poll(start + DISCOVERY_INTERVAL, |user| {
                [Some(third), None, None, Some(second)][user as usize]
            }),
            [Some(third), Some(second)],
        );
    }
}
