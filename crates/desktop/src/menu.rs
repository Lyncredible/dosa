use std::path::PathBuf;

use font8x8::{BASIC_FONTS, LATIN_FONTS, UnicodeFonts};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::Window;

use crate::{Frame, Input, Key};

pub(crate) const BAR_HEIGHT: u32 = 52;
const TAB_HEIGHT: u32 = 34;
const ROW_HEIGHT: u32 = 27;
const POPUP_WIDTH: u32 = 250;
const GLYPH_SIZE: u32 = 11;
const GLYPH_ADVANCE: u32 = 9;
const NAVY: [u8; 4] = [25, 30, 43, 255];
const PANEL: [u8; 4] = [33, 40, 55, 255];
const RULE: [u8; 4] = [60, 73, 95, 255];
const ACTIVE: [u8; 4] = [65, 79, 108, 255];
const TEXT: [u8; 4] = [239, 242, 250, 255];
const MUTED: [u8; 4] = [174, 186, 208, 255];
const DISABLED: [u8; 4] = [133, 144, 164, 255];
const ACCENT: [u8; 4] = [161, 200, 255, 255];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CpuMode {
    I8086,
    I286,
    #[default]
    I386,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Speed {
    Slow,
    #[default]
    Normal,
    Fast,
    Maximum,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Volume {
    Quiet = 25,
    Half = 50,
    Loud = 75,
    #[default]
    Full = 100,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextMode {
    #[default]
    Rows25,
    Rows43,
    Rows50,
    WideRows25,
    WideRows43,
    WideRows50,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Scaling {
    #[default]
    Fit,
    Integer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MountKind {
    File,
    Folder,
    Zip,
    Iso,
    SessionZip,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MenuAction {
    CopyText,
    PasteText,
    StopPaste,
    QuickLaunch(PathBuf),
    SaveScreenshot(PathBuf),
    StartRecording(PathBuf),
    StopRecording,
    Mount {
        drive: char,
        source: PathBuf,
        kind: MountKind,
    },
    SetCpu(CpuMode),
    Unmount(char),
    SetSpeed(Speed),
    SetVolume(Volume),
    SetMuted(bool),
    SetTextMode(TextMode),
    SetScaling(Scaling),
    SetPaused(bool),
    SetInspection(bool),
    StepInstruction,
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuState {
    pub enabled: bool,
    pub active_drive: char,
    pub drives: std::collections::BTreeMap<char, &'static str>,
    pub cpu: CpuMode,
    pub speed: Speed,
    pub volume: Volume,
    pub muted: bool,
    pub recording: bool,
    pub text_mode: TextMode,
    pub scaling: Scaling,
    pub running: bool,
    pub batch_active: bool,
    pub text_visible: bool,
    pub paste_pending: bool,
    pub paused: bool,
    pub inspection: Vec<String>,
    pub status: String,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            enabled: false,
            active_drive: 'C',
            drives: Default::default(),
            cpu: CpuMode::default(),
            speed: Speed::default(),
            volume: Volume::default(),
            muted: false,
            recording: false,
            text_mode: TextMode::default(),
            scaling: Scaling::default(),
            running: false,
            batch_active: false,
            text_visible: false,
            paste_pending: false,
            paused: false,
            inspection: Vec::new(),
            status: String::new(),
        }
    }
}

impl MenuState {
    pub(crate) fn audio_gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            f32::from(self.volume as u8) / 100.0
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tab {
    Drives,
    Cpu,
    Speed,
    View,
    Audio,
    Edit,
    Debug,
}

const TABS: [(Tab, &str, u32, u32); 7] = [
    (Tab::Drives, "Drives", 7, 68),
    (Tab::Cpu, "CPU", 78, 50),
    (Tab::Speed, "Speed", 131, 62),
    (Tab::View, "View", 196, 53),
    (Tab::Audio, "Audio", 252, 62),
    (Tab::Edit, "Edit", 317, 54),
    (Tab::Debug, "Debug", 374, 65),
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    QuickLaunch,
    Screenshot,
    RecordAudio,
    Mount(MountKind, char),
    Fullscreen,
    App(MenuAction),
}

#[derive(Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Rect {
    fn contains(self, point: (u32, u32)) -> bool {
        (self.x..self.x + self.w).contains(&point.0) && (self.y..self.y + self.h).contains(&point.1)
    }
}

fn tab_rect(tab: Tab) -> Rect {
    let &(_, _, x, w) = TABS.iter().find(|(entry, ..)| *entry == tab).unwrap();
    Rect {
        x,
        y: 3,
        w,
        h: TAB_HEIGHT - 6,
    }
}

#[derive(Default)]
pub(crate) struct Menu {
    active: Option<Tab>,
    selected: usize,
    drive: Option<char>,
    point: Option<(u32, u32)>,
    mouse_down: bool,
    pixels: Vec<u8>,
}

impl Menu {
    pub(crate) fn close(&mut self) {
        self.active = None;
        self.mouse_down = false;
    }

    pub(crate) fn leave(&mut self) {
        self.point = None;
    }

    pub(crate) fn move_pointer(&mut self, position: PhysicalPosition<f64>, scale: f64) {
        self.point = (position.x >= 0.0 && position.y >= 0.0)
            .then(|| ((position.x / scale) as u32, (position.y / scale) as u32));
    }

    pub(crate) fn blocks_pointer(&self) -> bool {
        self.active.is_some() || self.point.is_some_and(|(_, y)| y < BAR_HEIGHT)
    }

    fn drive(&self, state: &MenuState) -> char {
        self.drive.unwrap_or(state.active_drive)
    }

    pub(crate) fn mouse(
        &mut self,
        pressed: bool,
        left: bool,
        width: u32,
        state: &MenuState,
        fullscreen: bool,
    ) -> (bool, Option<Command>) {
        if !pressed && left && std::mem::take(&mut self.mouse_down) {
            return (true, None);
        }
        if !self.blocks_pointer() {
            return (false, None);
        }
        if !pressed || !left {
            return (true, None);
        }
        self.mouse_down = true;
        let Some(point) = self.point else {
            self.active = None;
            return (true, None);
        };
        if let Some((tab, ..)) = TABS.iter().find(|(tab, ..)| tab_rect(*tab).contains(point)) {
            if self.active == Some(*tab) {
                self.active = None;
            } else {
                self.open(*tab, state);
            }
            return (true, None);
        }
        let Some(tab) = self.active else {
            return (true, None);
        };
        let left = popup_left(tab, width);
        let drive = self.drive(state);
        let items = options(tab, state, drive, fullscreen);
        for (index, item) in items.iter().enumerate() {
            if row_rect(left, index).contains(point) {
                return (true, self.activate(item));
            }
        }
        if tab == Tab::Drives {
            for index in 0..26 {
                if drive_rect(left, index).contains(point) {
                    self.drive = Some(char::from(b'A' + index as u8));
                    return (true, None);
                }
            }
        }
        self.active = None;
        (true, None)
    }

    pub(crate) fn key(
        &mut self,
        input: &Input,
        shortcut_key: Option<char>,
        state: &MenuState,
        fullscreen: bool,
    ) -> (bool, Option<Command>) {
        if !input.pressed {
            return (false, None);
        }
        if input.modifiers.alt && !input.modifiers.ctrl && !input.modifiers.logo {
            let tab = match shortcut_key.map(|key| key.to_ascii_lowercase()) {
                Some('d') => Some(Tab::Drives),
                Some('c') => Some(Tab::Cpu),
                Some('s') => Some(Tab::Speed),
                Some('v') => Some(Tab::View),
                Some('a') => Some(Tab::Audio),
                Some('e') => Some(Tab::Edit),
                Some('g') => Some(Tab::Debug),
                _ => None,
            };
            if let Some(tab) = tab {
                self.open(tab, state);
                return (true, None);
            }
            if input.key == Key::Enter {
                self.active = None;
                return (true, Some(Command::Fullscreen));
            }
        }
        let Some(tab) = self.active else {
            if !state.inspection.is_empty() && input.key == Key::Function(10) {
                return (true, Some(Command::App(MenuAction::StepInstruction)));
            }
            return (false, None);
        };
        let items = options(tab, state, self.drive(state), fullscreen);
        match input.key {
            Key::Escape => self.active = None,
            Key::Down | Key::Up => {
                let count = items.len();
                let step = if input.key == Key::Down { 1 } else { count - 1 };
                for _ in 0..count {
                    self.selected = (self.selected + step) % count;
                    if items[self.selected].enabled {
                        break;
                    }
                }
            }
            Key::Enter => {
                if let Some(item) = items.get(self.selected) {
                    return (true, self.activate(item));
                }
            }
            Key::Character(ch) if tab == Tab::Drives && ch.is_ascii_alphabetic() => {
                self.drive = Some(ch.to_ascii_uppercase());
            }
            _ => {}
        }
        (true, None)
    }

    fn open(&mut self, tab: Tab, state: &MenuState) {
        let items = options(tab, state, self.drive(state), false);
        self.selected = items.iter().position(|item| item.enabled).unwrap_or(0);
        self.active = Some(tab);
    }

    fn activate(&mut self, option: &Item) -> Option<Command> {
        if option.enabled {
            self.active = None;
            Some(option.command.clone())
        } else {
            None
        }
    }

    pub(crate) fn paint(
        &mut self,
        size: PhysicalSize<u32>,
        scale: f64,
        state: &MenuState,
        fullscreen: bool,
    ) -> Frame<'_> {
        let width = logical_width(size, scale);
        let menu_height = self.active.map_or(BAR_HEIGHT, |tab| {
            let count = options(tab, state, self.drive(state), fullscreen).len() as u32;
            BAR_HEIGHT + ROW_HEIGHT * count + if tab == Tab::Drives { 75 } else { 5 }
        });
        let inspection_height = BAR_HEIGHT + 12 + state.inspection.len() as u32 * 18;
        let height = menu_height
            .max(if state.inspection.is_empty() {
                BAR_HEIGHT
            } else {
                inspection_height
            })
            .min((f64::from(size.height) / scale).floor() as u32)
            .max(1);
        let left = self.active.map_or(0, |tab| popup_left(tab, width));
        let drive = self.drive(state);
        self.pixels.resize(width as usize * height as usize * 4, 0);
        self.pixels.fill(0);
        let mut canvas = Canvas {
            width,
            height,
            pixels: &mut self.pixels,
        };
        canvas.fill(
            Rect {
                x: 0,
                y: 0,
                w: width,
                h: BAR_HEIGHT,
            },
            NAVY,
        );
        canvas.fill(
            Rect {
                x: 0,
                y: BAR_HEIGHT - 1,
                w: width,
                h: 1,
            },
            RULE,
        );
        for (tab, label, ..) in TABS {
            let rect = tab_rect(tab);
            if self.active == Some(tab) || self.point.is_some_and(|point| rect.contains(point)) {
                canvas.fill(rect, ACTIVE);
            }
            canvas.text(rect.x + 7, 11, label, TEXT);
        }
        if width >= 610 {
            canvas.text(width - 166, 11, "Alt+D/C/S/V/A/E/G", MUTED);
        }
        let settings = format!(
            "{}: | {} | {}{}{}{}",
            state.active_drive,
            cpu_label(state.cpu),
            speed_label(state.speed),
            if state.paused { " | Paused" } else { "" },
            if state.muted { " | Muted" } else { "" },
            if state.recording { " | REC" } else { "" },
        );
        canvas.text(12, 36, &settings, MUTED);
        if !state.status.is_empty() {
            let position = 30 + settings.chars().count() as u32 * GLYPH_ADVANCE;
            canvas.text(position, 36, &state.status, ACCENT);
        }
        if !state.inspection.is_empty() {
            canvas.fill(
                Rect {
                    x: 0,
                    y: BAR_HEIGHT,
                    w: width.min(410),
                    h: inspection_height - BAR_HEIGHT,
                },
                PANEL,
            );
            for (row, line) in state.inspection.iter().enumerate() {
                canvas.text(12, BAR_HEIGHT + 8 + row as u32 * 18, line, TEXT);
            }
        }
        if let Some(tab) = self.active {
            let items = options(tab, state, drive, fullscreen);
            canvas.fill(
                Rect {
                    x: left,
                    y: BAR_HEIGHT,
                    w: POPUP_WIDTH,
                    h: menu_height - BAR_HEIGHT,
                },
                RULE,
            );
            canvas.fill(
                Rect {
                    x: left + 1,
                    y: BAR_HEIGHT,
                    w: POPUP_WIDTH - 2,
                    h: menu_height - BAR_HEIGHT - 1,
                },
                PANEL,
            );
            for (index, item) in items.iter().enumerate() {
                let rect = row_rect(left, index);
                if item.enabled
                    && (index == self.selected
                        || self.point.is_some_and(|point| rect.contains(point)))
                {
                    canvas.fill(rect, ACTIVE);
                }
                if item.chosen {
                    canvas.text(rect.x + 9, rect.y + 8, ">", ACCENT);
                }
                let color = if item.enabled { TEXT } else { DISABLED };
                canvas.text(rect.x + 25, rect.y + 8, item.label, color);
            }
            if tab == Tab::Drives {
                let info = state.drives.get(&drive).copied().unwrap_or("Not mounted");
                let label = format!("{drive}: {info}");
                canvas.text(
                    left + 12,
                    BAR_HEIGHT + DRIVE_ACTION_ROWS * ROW_HEIGHT + 6,
                    &label,
                    MUTED,
                );
                for index in 0..26 {
                    let rect = drive_rect(left, index);
                    let letter = char::from(b'A' + index as u8);
                    if letter == drive || self.point.is_some_and(|point| rect.contains(point)) {
                        canvas.fill(rect, ACTIVE);
                    }
                    let color = if state.drives.contains_key(&letter) || letter == drive {
                        TEXT
                    } else {
                        MUTED
                    };
                    canvas.text(rect.x + 4, rect.y + 4, &letter.to_string(), color);
                }
            }
        }
        Frame {
            width,
            height,
            rgba: &self.pixels,
        }
    }
}

pub(crate) fn logical_width(size: PhysicalSize<u32>, scale: f64) -> u32 {
    ((f64::from(size.width) / scale).floor() as u32).max(1)
}

fn popup_left(tab: Tab, width: u32) -> u32 {
    tab_rect(tab).x.min(width.saturating_sub(POPUP_WIDTH))
}

fn row_rect(left: u32, index: usize) -> Rect {
    Rect {
        x: left + 1,
        y: BAR_HEIGHT + ROW_HEIGHT * index as u32,
        w: POPUP_WIDTH - 2,
        h: ROW_HEIGHT,
    }
}

const DRIVE_ACTION_ROWS: u32 = 7;

fn drive_rect(left: u32, index: u32) -> Rect {
    Rect {
        x: left + 8 + (index % 13) * 18,
        y: BAR_HEIGHT + DRIVE_ACTION_ROWS * ROW_HEIGHT + 22 + (index / 13) * 21,
        w: 17,
        h: 20,
    }
}

struct Item {
    label: &'static str,
    command: Command,
    enabled: bool,
    chosen: bool,
}

fn options(tab: Tab, state: &MenuState, drive: char, fullscreen: bool) -> Vec<Item> {
    let can_change_mounts = !state.running && !state.batch_active;
    let item = |label, command, enabled, chosen| Item {
        label,
        command,
        enabled,
        chosen,
    };
    match tab {
        Tab::Edit => vec![
            item(
                "Copy screen text",
                Command::App(MenuAction::CopyText),
                state.text_visible,
                false,
            ),
            item(
                "Paste text",
                Command::App(MenuAction::PasteText),
                state.running || !state.batch_active,
                false,
            ),
            item(
                "Stop pasting",
                Command::App(MenuAction::StopPaste),
                state.paste_pending,
                false,
            ),
        ],
        Tab::Drives => [
            ("Mount file...", MountKind::File),
            ("Mount folder...", MountKind::Folder),
            ("Mount ZIP...", MountKind::SessionZip),
            ("Mount read-only ZIP...", MountKind::Zip),
            ("Mount ISO image...", MountKind::Iso),
        ]
        .into_iter()
        .map(|(label, kind)| item(label, Command::Mount(kind, drive), can_change_mounts, false))
        .chain(std::iter::once(item(
            "Quick launch...",
            Command::QuickLaunch,
            can_change_mounts,
            false,
        )))
        .chain(std::iter::once(item(
            "Unmount",
            Command::App(MenuAction::Unmount(drive)),
            can_change_mounts && state.drives.contains_key(&drive),
            false,
        )))
        .collect(),
        Tab::Cpu => [CpuMode::I8086, CpuMode::I286, CpuMode::I386]
            .into_iter()
            .map(|mode| {
                item(
                    cpu_label(mode),
                    Command::App(MenuAction::SetCpu(mode)),
                    !state.running,
                    mode == state.cpu,
                )
            })
            .collect(),
        Tab::Speed => [Speed::Slow, Speed::Normal, Speed::Fast, Speed::Maximum]
            .into_iter()
            .map(|speed| {
                item(
                    speed_label(speed),
                    Command::App(MenuAction::SetSpeed(speed)),
                    true,
                    speed == state.speed,
                )
            })
            .collect(),
        Tab::Audio => std::iter::once(item(
            if state.muted { "Unmute" } else { "Mute" },
            Command::App(MenuAction::SetMuted(!state.muted)),
            true,
            state.muted,
        ))
        .chain(
            [
                ("Volume 25%", Volume::Quiet),
                ("Volume 50%", Volume::Half),
                ("Volume 75%", Volume::Loud),
                ("Volume 100%", Volume::Full),
            ]
            .map(|(label, volume)| {
                item(
                    label,
                    Command::App(MenuAction::SetVolume(volume)),
                    true,
                    volume == state.volume,
                )
            }),
        )
        .chain(std::iter::once(if state.recording {
            item(
                "Stop WAV recording",
                Command::App(MenuAction::StopRecording),
                true,
                false,
            )
        } else {
            item("Record audio to WAV...", Command::RecordAudio, true, false)
        }))
        .collect(),
        Tab::Debug => {
            let inspecting = !state.inspection.is_empty();
            vec![
                item(
                    if inspecting {
                        "Hide inspection"
                    } else {
                        "Inspect guest"
                    },
                    Command::App(MenuAction::SetInspection(!inspecting)),
                    state.running,
                    inspecting,
                ),
                item(
                    "Step instruction (F10)",
                    Command::App(MenuAction::StepInstruction),
                    state.running && state.paused,
                    false,
                ),
                item(
                    "Resume guest",
                    Command::App(MenuAction::SetPaused(false)),
                    state.running && state.paused,
                    false,
                ),
            ]
        }
        Tab::View => {
            let mut items = Vec::from(
                [
                    ("25 text rows", TextMode::Rows25),
                    ("43 text rows", TextMode::Rows43),
                    ("50 text rows", TextMode::Rows50),
                    ("132 x 25", TextMode::WideRows25),
                    ("132 x 43", TextMode::WideRows43),
                    ("132 x 50", TextMode::WideRows50),
                ]
                .map(|(label, mode)| {
                    item(
                        label,
                        Command::App(MenuAction::SetTextMode(mode)),
                        !state.running,
                        mode == state.text_mode,
                    )
                }),
            );
            items.extend(
                [
                    ("Fit to window", Scaling::Fit),
                    ("Integer scaling", Scaling::Integer),
                ]
                .map(|(label, scaling)| {
                    item(
                        label,
                        Command::App(MenuAction::SetScaling(scaling)),
                        true,
                        state.scaling == scaling,
                    )
                }),
            );
            items.push(item(
                if fullscreen {
                    "Leave fullscreen"
                } else {
                    "Fullscreen"
                },
                Command::Fullscreen,
                true,
                fullscreen,
            ));
            items.push(item(
                if state.paused { "Resume" } else { "Pause" },
                Command::App(MenuAction::SetPaused(!state.paused)),
                state.running,
                false,
            ));
            items.push(item("Save screenshot...", Command::Screenshot, true, false));
            items.push(item("Exit", Command::App(MenuAction::Exit), true, false));
            items
        }
    }
}

fn cpu_label(mode: CpuMode) -> &'static str {
    match mode {
        CpuMode::I8086 => "8086",
        CpuMode::I286 => "286",
        CpuMode::I386 => "386",
    }
}

fn speed_label(speed: Speed) -> &'static str {
    match speed {
        Speed::Slow => "Slow",
        Speed::Normal => "Normal",
        Speed::Fast => "Fast",
        Speed::Maximum => "Maximum",
    }
}

pub(crate) fn pick_path(window: &Window, kind: MountKind) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new().set_parent(window);
    match kind {
        MountKind::File => dialog.set_title("Mount a file").pick_file(),
        MountKind::Folder => dialog.set_title("Mount a folder").pick_folder(),
        MountKind::Zip => dialog
            .set_title("Mount a read-only ZIP archive")
            .add_filter("ZIP archive", &["zip"])
            .pick_file(),
        MountKind::Iso => dialog
            .set_title("Mount an ISO 9660 image")
            .add_filter("ISO image", &["iso"])
            .pick_file(),
        MountKind::SessionZip => dialog
            .set_title("Mount a temporary writable ZIP copy")
            .add_filter("ZIP archive", &["zip"])
            .pick_file(),
    }
}

struct Canvas<'a> {
    width: u32,
    height: u32,
    pixels: &'a mut [u8],
}

impl Canvas<'_> {
    fn fill(&mut self, rect: Rect, color: [u8; 4]) {
        if rect.x >= self.width {
            return;
        }
        let right = (rect.x + rect.w).min(self.width);
        for y in rect.y..(rect.y + rect.h).min(self.height) {
            let row = y as usize * self.width as usize;
            let start = 4 * (row + rect.x as usize);
            let end = 4 * (row + right as usize);
            for pixel in self.pixels[start..end].as_chunks_mut::<4>().0 {
                *pixel = color;
            }
        }
    }

    fn text(&mut self, mut x: u32, y: u32, contents: &str, color: [u8; 4]) {
        for ch in contents.chars() {
            if x >= self.width {
                break;
            }
            let glyph = BASIC_FONTS
                .get(ch)
                .or_else(|| LATIN_FONTS.get(ch))
                .unwrap_or_else(|| BASIC_FONTS.get('?').unwrap());
            for row in 0..GLYPH_SIZE {
                let bits = glyph[(row * 8 / GLYPH_SIZE) as usize];
                for col in 0..GLYPH_ADVANCE {
                    if bits & (1 << (col * 8 / GLYPH_ADVANCE)) != 0 {
                        self.fill(
                            Rect {
                                x: x + col,
                                y: y + row,
                                w: 1,
                                h: 1,
                            },
                            color,
                        );
                    }
                }
            }
            x += GLYPH_ADVANCE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modifiers;

    fn input(key: Key, alt: bool) -> Input {
        Input {
            key,
            pressed: true,
            text: None,
            modifiers: Modifiers {
                alt,
                ..Default::default()
            },
            scan_code: 0,
        }
    }

    #[test]
    fn keyboard_can_select_mount_target_and_kind() {
        let mut menu = Menu::default();
        let state = MenuState::default();
        assert_eq!(
            menu.key(&input(Key::Character('x'), true), Some('d'), &state, false),
            (true, None)
        );
        menu.key(&input(Key::Character('z'), false), None, &state, false);
        menu.key(&input(Key::Down, false), None, &state, false);
        menu.key(&input(Key::Down, false), None, &state, false);
        assert_eq!(
            menu.key(&input(Key::Enter, false), None, &state, false),
            (true, Some(Command::Mount(MountKind::SessionZip, 'Z')))
        );
        assert!(!menu.blocks_pointer());
    }

    #[test]
    fn running_guest_can_change_speed_but_cannot_mount() {
        let mut menu = Menu::default();
        let state = MenuState {
            running: true,
            ..Default::default()
        };
        menu.open(Tab::Drives, &state);
        assert_eq!(
            menu.key(&input(Key::Enter, false), None, &state, false),
            (true, None)
        );
        menu.open(Tab::Speed, &state);
        assert_eq!(
            menu.key(&input(Key::Enter, false), None, &state, false),
            (true, Some(Command::App(MenuAction::SetSpeed(Speed::Slow))))
        );
    }

    #[test]
    fn unmount_targets_the_selected_mounted_drive_and_respects_batch_lock() {
        let mut menu = Menu::default();
        let mut state = MenuState::default();
        state.drives.insert('D', "Folder (read-only)");
        menu.open(Tab::Drives, &state);
        menu.key(&input(Key::Character('d'), false), None, &state, false);
        menu.key(&input(Key::Up, false), None, &state, false);
        assert_eq!(
            menu.key(&input(Key::Enter, false), None, &state, false),
            (true, Some(Command::App(MenuAction::Unmount('D'))))
        );
        state.batch_active = true;
        assert!(
            options(Tab::Drives, &state, 'D', false)
                .iter()
                .all(|item| !item.enabled)
        );
    }

    #[test]
    fn audio_menu_remains_available_to_a_running_guest() {
        let mut menu = Menu::default();
        let mut state = MenuState {
            running: true,
            volume: Volume::Half,
            ..Default::default()
        };
        menu.key(&input(Key::Character('a'), true), Some('a'), &state, false);
        assert_eq!(
            menu.key(&input(Key::Enter, false), None, &state, false),
            (true, Some(Command::App(MenuAction::SetMuted(true))))
        );
        assert_eq!(state.audio_gain(), 0.5);
        state.muted = true;
        assert_eq!(state.audio_gain(), 0.0);
        menu.open(Tab::Audio, &state);
        menu.key(&input(Key::Down, false), None, &state, false);
        assert_eq!(
            menu.key(&input(Key::Enter, false), None, &state, false),
            (
                true,
                Some(Command::App(MenuAction::SetVolume(Volume::Quiet)))
            )
        );
    }

    #[test]
    fn audio_menu_switches_between_start_and_stop_recording() {
        let mut menu = Menu::default();
        let mut state = MenuState::default();
        for expected in [
            Command::RecordAudio,
            Command::App(MenuAction::StopRecording),
        ] {
            menu.open(Tab::Audio, &state);
            menu.key(&input(Key::Up, false), None, &state, false);
            assert_eq!(
                menu.key(&input(Key::Enter, false), None, &state, false),
                (true, Some(expected))
            );
            state.recording = true;
        }
    }

    #[test]
    fn debug_shortcut_steps_only_while_inspection_is_visible() {
        let mut menu = Menu::default();
        let mut state = MenuState {
            running: true,
            ..Default::default()
        };
        assert_eq!(
            menu.key(&input(Key::Function(10), false), None, &state, false),
            (false, None)
        );
        menu.key(&input(Key::Character('g'), true), Some('g'), &state, false);
        assert_eq!(
            menu.key(&input(Key::Enter, false), None, &state, false),
            (true, Some(Command::App(MenuAction::SetInspection(true))))
        );
        state.paused = true;
        state.inspection.push("AX 0000".into());
        assert_eq!(
            menu.key(&input(Key::Function(10), false), None, &state, false),
            (true, Some(Command::App(MenuAction::StepInstruction)))
        );
        let frame = menu.paint(PhysicalSize::new(800, 500), 1.0, &state, false);
        assert!(frame.height > BAR_HEIGHT);
        assert_eq!(&frame.rgba[(800 * BAR_HEIGHT as usize) * 4..][..4], &PANEL);
    }

    #[test]
    fn dpi_adjusted_clicks_match_the_overlay_rows() {
        let mut menu = Menu::default();
        let state = MenuState::default();
        menu.move_pointer(PhysicalPosition::new(20.0, 16.0), 2.0);
        assert_eq!(menu.mouse(true, true, 400, &state, false), (true, None));
        assert_eq!(menu.mouse(false, true, 400, &state, false), (true, None));
        assert!(menu.blocks_pointer());
        let frame = menu.paint(PhysicalSize::new(800, 500), 2.0, &state, false);
        assert_eq!(frame.width, 400);
        let row = (BAR_HEIGHT * frame.width) as usize;
        assert_eq!(frame.rgba[4 * (row + 16) + 3], 255);
        assert_eq!(frame.rgba[4 * (row + 380) + 3], 0);
        menu.move_pointer(PhysicalPosition::new(24.0, 170.0), 2.0);
        assert_eq!(
            menu.mouse(true, true, 400, &state, false),
            (true, Some(Command::Mount(MountKind::Folder, 'C')))
        );
        assert_eq!(menu.mouse(false, true, 400, &state, false), (true, None));
    }
}
