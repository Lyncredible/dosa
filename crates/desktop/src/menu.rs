use std::path::PathBuf;

use font8x8::{BASIC_FONTS, LATIN_FONTS, UnicodeFonts};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::Window;

use crate::{Frame, Input, Key};

pub(crate) const BAR_HEIGHT: u32 = 36;
const ROW_HEIGHT: u32 = 24;
const POPUP_WIDTH: u32 = 250;
const POPUP_LEFT: u32 = 6;
const POPUP_TOP: u32 = BAR_HEIGHT + 4;
const CATEGORY_HEADER: u32 = 24;
const GLYPH_SIZE: u32 = 11;
const GLYPH_ADVANCE: u32 = 9;
const NAVY: [u8; 4] = [21, 23, 29, 255];
const PANEL: [u8; 4] = [31, 34, 43, 255];
const RULE: [u8; 4] = [48, 53, 67, 255];
const ACTIVE: [u8; 4] = [48, 57, 75, 255];
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
    Fat,
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
    SaveGuestState(PathBuf),
    LoadGuestState(PathBuf),
    SaveStateSlot(StateSlot),
    LoadStateSlot(StateSlot),
    SetAutomaticSave(bool),
    LoadAutomaticSave,
    StartRecording(PathBuf),
    StopRecording,
    Mount {
        drive: char,
        source: PathBuf,
        kind: MountKind,
    },
    SetCpu(CpuMode),
    Unmount(char),
    UseWritableZip(char),
    SetSpeed(Speed),
    SetVolume(Volume),
    SetMuted(bool),
    SetTextMode(TextMode),
    SetScaling(Scaling),
    SetPaused(bool),
    SetInspection(bool),
    StepInstruction,
    ResetGuest,
    Exit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MenuState {
    pub enabled: bool,
    pub active_drive: char,
    pub write_blocked_zip: Option<char>,
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
    pub save_state_blocked: bool,
    pub state_slots_available: bool,
    pub automatic_save: bool,
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
            write_blocked_zip: None,
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
            save_state_blocked: false,
            state_slots_available: false,
            automatic_save: false,
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
pub(crate) enum Tab {
    Drives,
    Cpu,
    Speed,
    View,
    Audio,
    Edit,
    Debug,
    StateSlots,
}

pub(crate) const CATEGORIES: [(Tab, &str); 8] = [
    (Tab::Drives, "Drives"),
    (Tab::Cpu, "CPU"),
    (Tab::Speed, "Speed"),
    (Tab::View, "View"),
    (Tab::StateSlots, "State slots"),
    (Tab::Audio, "Audio"),
    (Tab::Edit, "Edit"),
    (Tab::Debug, "Debug"),
];

/// The three local, named save destinations.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateSlot {
    One = 1,
    Two = 2,
    Three = 3,
}

impl StateSlot {
    pub fn number(self) -> u8 {
        self as u8
    }
}
const ROOT_ROWS: usize = CATEGORIES.len() + 1;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Panel {
    Root,
    Category(Tab),
}

impl Panel {
    fn rows_top(self) -> u32 {
        POPUP_TOP
            + if self == Self::Root {
                4
            } else {
                CATEGORY_HEADER
            }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Command {
    QuickLaunch,
    Screenshot,
    GuestState { save: bool },
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

const MENU_BUTTON: Rect = Rect {
    x: 7,
    y: 5,
    w: 36,
    h: 26,
};

#[derive(Default)]
pub(crate) struct Menu {
    active: Option<Panel>,
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
        if MENU_BUTTON.contains(point) {
            if self.active.is_some() {
                self.active = None;
            } else {
                self.open_root();
            }
            return (true, None);
        }
        let Some(panel) = self.active else {
            return (true, None);
        };
        if panel == Panel::Root {
            if let Some(index) =
                (0..ROOT_ROWS).find(|&index| row_rect(panel, index).contains(point))
            {
                return (true, self.select_root(index, state));
            }
            self.active = None;
            return (true, None);
        }
        let Panel::Category(tab) = panel else {
            unreachable!()
        };
        if category_header().contains(point) {
            self.open_root();
            return (true, None);
        }
        let drive = self.drive(state);
        let items = options(tab, state, drive, fullscreen);
        for (index, item) in items.iter().enumerate() {
            if row_rect(panel, index).contains(point) {
                return (true, self.activate(item));
            }
        }
        if tab == Tab::Drives {
            for index in 0..26 {
                if drive_rect(index).contains(point) {
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
        state: &MenuState,
        fullscreen: bool,
        compact: bool,
    ) -> (bool, Option<Command>) {
        if !input.pressed {
            return (false, None);
        }
        if input.key == Key::Enter
            && input.modifiers.alt
            && !input.modifiers.ctrl
            && !input.modifiers.logo
        {
            self.active = None;
            return (true, Some(Command::Fullscreen));
        }
        let Some(panel) = self.active else {
            if !state.inspection.is_empty() && input.key == Key::Function(10) {
                return (true, Some(Command::App(MenuAction::StepInstruction)));
            }
            if compact && !state.running && input.key == Key::Function(10) {
                self.open_root();
                return (true, None);
            }
            return (false, None);
        };
        if panel == Panel::Root {
            match input.key {
                Key::Escape => self.active = None,
                Key::Down => self.selected = (self.selected + 1) % ROOT_ROWS,
                Key::Up => self.selected = (self.selected + ROOT_ROWS - 1) % ROOT_ROWS,
                Key::Enter | Key::Right => return (true, self.select_root(self.selected, state)),
                _ => {}
            }
            return (true, None);
        }
        let Panel::Category(tab) = panel else {
            unreachable!()
        };
        let items = options(tab, state, self.drive(state), fullscreen);
        match input.key {
            Key::Escape => self.active = None,
            Key::Left | Key::Backspace => self.open_root(),
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

    fn open_root(&mut self) {
        self.active = Some(Panel::Root);
        self.selected = 0;
    }

    fn select_root(&mut self, index: usize, state: &MenuState) -> Option<Command> {
        if let Some(&(tab, _)) = CATEGORIES.get(index) {
            self.open(tab, state);
            None
        } else {
            self.active = None;
            Some(Command::App(MenuAction::ResetGuest))
        }
    }

    fn open(&mut self, tab: Tab, state: &MenuState) {
        let items = options(tab, state, self.drive(state), false);
        self.selected = items.iter().position(|item| item.enabled).unwrap_or(0);
        self.active = Some(Panel::Category(tab));
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
        header: bool,
    ) -> Frame<'_> {
        let width = logical_width(size, scale);
        let drive = self.drive(state);
        let header_height = if header { BAR_HEIGHT } else { 0 };
        let panel = if header { self.active } else { None };
        let items = match panel {
            Some(Panel::Category(tab)) => options(tab, state, drive, fullscreen),
            _ => Vec::new(),
        };
        let menu_height = match panel {
            None => header_height,
            Some(panel) => {
                panel.rows_top()
                    + ROW_HEIGHT
                        * if panel == Panel::Root {
                            ROOT_ROWS as u32
                        } else {
                            items.len() as u32
                        }
                    + if panel == Panel::Category(Tab::Drives) {
                        64
                    } else {
                        5
                    }
            }
        };
        let inspection_height = header_height + 12 + state.inspection.len() as u32 * 18;
        let height = menu_height
            .max(if state.inspection.is_empty() {
                header_height
            } else {
                inspection_height
            })
            .min((f64::from(size.height) / scale).floor() as u32)
            .max(1);
        self.pixels.resize(width as usize * height as usize * 4, 0);
        self.pixels.fill(0);
        let mut canvas = Canvas {
            width,
            height,
            pixels: &mut self.pixels,
        };
        if header {
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
            if panel.is_some() || self.point.is_some_and(|point| MENU_BUTTON.contains(point)) {
                canvas.fill(MENU_BUTTON, ACTIVE);
            }
            for y in [11, 17, 23] {
                canvas.fill(
                    Rect {
                        x: 17,
                        y,
                        w: 16,
                        h: 2,
                    },
                    TEXT,
                );
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
            let (title, color) = if state.status.is_empty() {
                (&settings, MUTED)
            } else {
                (&state.status, ACCENT)
            };
            canvas.text(55, 12, title, color);
        }
        if !state.inspection.is_empty() {
            canvas.fill(
                Rect {
                    x: 0,
                    y: header_height,
                    w: width.min(410),
                    h: inspection_height - header_height,
                },
                PANEL,
            );
            for (row, line) in state.inspection.iter().enumerate() {
                canvas.text(12, header_height + 8 + row as u32 * 18, line, TEXT);
            }
        }
        if let Some(panel) = panel {
            canvas.fill(
                Rect {
                    x: POPUP_LEFT,
                    y: POPUP_TOP,
                    w: POPUP_WIDTH,
                    h: menu_height - POPUP_TOP,
                },
                RULE,
            );
            canvas.fill(
                Rect {
                    x: POPUP_LEFT + 1,
                    y: POPUP_TOP + 1,
                    w: POPUP_WIDTH - 2,
                    h: menu_height - POPUP_TOP - 2,
                },
                PANEL,
            );
            if panel == Panel::Root {
                for (index, label) in CATEGORIES
                    .iter()
                    .map(|&(_, name)| name)
                    .chain(std::iter::once("Reset guest"))
                    .enumerate()
                {
                    let rect = row_rect(panel, index);
                    if index == self.selected
                        || self.point.is_some_and(|point| rect.contains(point))
                    {
                        canvas.fill(rect, ACTIVE);
                    }
                    canvas.text(rect.x + 14, rect.y + 7, label, TEXT);
                    if index < CATEGORIES.len() {
                        canvas.text(rect.x + rect.w - 22, rect.y + 7, ">", MUTED);
                    }
                }
            } else if let Panel::Category(tab) = panel {
                let header = category_header();
                if self.point.is_some_and(|point| header.contains(point)) {
                    canvas.fill(header, ACTIVE);
                }
                let label = CATEGORIES
                    .iter()
                    .find(|&&(category, _)| category == tab)
                    .unwrap()
                    .1;
                canvas.text(header.x + 12, header.y + 6, &format!("<  {label}"), ACCENT);
            }
            for (index, item) in items.iter().enumerate() {
                let rect = row_rect(panel, index);
                if item.enabled
                    && (index == self.selected
                        || self.point.is_some_and(|point| rect.contains(point)))
                {
                    canvas.fill(rect, ACTIVE);
                }
                if item.chosen {
                    canvas.text(rect.x + 9, rect.y + 7, ">", ACCENT);
                }
                let color = if item.enabled { TEXT } else { DISABLED };
                canvas.text(rect.x + 25, rect.y + 7, item.label, color);
            }
            if panel == Panel::Category(Tab::Drives) {
                let info = state.drives.get(&drive).copied().unwrap_or("Not mounted");
                let label = format!("{drive}: {info}");
                canvas.text(
                    POPUP_LEFT + 12,
                    panel.rows_top() + DRIVE_ACTION_ROWS * ROW_HEIGHT + 3,
                    &label,
                    MUTED,
                );
                for index in 0..26 {
                    let rect = drive_rect(index);
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

fn category_header() -> Rect {
    Rect {
        x: POPUP_LEFT + 1,
        y: POPUP_TOP + 1,
        w: POPUP_WIDTH - 2,
        h: CATEGORY_HEADER - 1,
    }
}

fn row_rect(panel: Panel, index: usize) -> Rect {
    Rect {
        x: POPUP_LEFT + 1,
        y: panel.rows_top() + ROW_HEIGHT * index as u32,
        w: POPUP_WIDTH - 2,
        h: ROW_HEIGHT,
    }
}

const DRIVE_ACTION_ROWS: u32 = 9;

fn drive_rect(index: u32) -> Rect {
    Rect {
        x: POPUP_LEFT + 8 + (index % 13) * 18,
        y: Panel::Category(Tab::Drives).rows_top()
            + DRIVE_ACTION_ROWS * ROW_HEIGHT
            + 18
            + (index / 13) * 21,
        w: 17,
        h: 20,
    }
}

#[derive(PartialEq, Eq)]
pub(crate) struct Item {
    pub label: &'static str,
    pub command: Command,
    pub enabled: bool,
    pub chosen: bool,
}

pub(crate) fn options(tab: Tab, state: &MenuState, drive: char, fullscreen: bool) -> Vec<Item> {
    let can_change_mounts = !state.running && !state.batch_active;
    let item = |label, command, enabled, chosen| Item {
        label,
        command,
        enabled,
        chosen,
    };
    match tab {
        Tab::StateSlots => [
            (StateSlot::One, "Save slot 1", "Load slot 1"),
            (StateSlot::Two, "Save slot 2", "Load slot 2"),
            (StateSlot::Three, "Save slot 3", "Load slot 3"),
        ]
        .into_iter()
        .flat_map(|(slot, save, load)| {
            [
                item(
                    save,
                    Command::App(MenuAction::SaveStateSlot(slot)),
                    state.state_slots_available && state.running && !state.save_state_blocked,
                    false,
                ),
                item(
                    load,
                    Command::App(MenuAction::LoadStateSlot(slot)),
                    state.state_slots_available,
                    false,
                ),
            ]
        })
        .chain([
            item(
                "Automatic save",
                Command::App(MenuAction::SetAutomaticSave(!state.automatic_save)),
                state.state_slots_available,
                state.automatic_save,
            ),
            item(
                "Load automatic save",
                Command::App(MenuAction::LoadAutomaticSave),
                state.state_slots_available,
                false,
            ),
        ])
        .collect(),
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
            ("Mount FAT image...", MountKind::Fat),
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
            if state.running {
                "Stop and use ZIP copy"
            } else {
                "Use writable ZIP copy"
            },
            Command::App(MenuAction::UseWritableZip(drive)),
            state.write_blocked_zip == Some(drive),
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
            items.push(item(
                "Save state...",
                Command::GuestState { save: true },
                state.running && !state.save_state_blocked,
                false,
            ));
            items.push(item(
                "Load state...",
                Command::GuestState { save: false },
                true,
                false,
            ));
            items.push(item("Save screenshot...", Command::Screenshot, true, false));
            items.push(item("Exit", Command::App(MenuAction::Exit), true, false));
            items
        }
    }
}

pub(crate) fn cpu_label(mode: CpuMode) -> &'static str {
    match mode {
        CpuMode::I8086 => "8086",
        CpuMode::I286 => "286",
        CpuMode::I386 => "386",
    }
}

pub(crate) fn speed_label(speed: Speed) -> &'static str {
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
        MountKind::Fat => dialog
            .set_title("Mount a FAT disk image")
            .add_filter("Raw disk image", &["img", "ima", "dsk"])
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
    fn alt_letters_pass_to_the_guest_and_reset_is_in_the_root_menu() {
        let mut menu = Menu::default();
        let state = MenuState {
            running: true,
            ..Default::default()
        };
        for key in ['d', 'c', 's', 'v', 'a', 'e', 'g', 'x', 'y', 'z'] {
            assert_eq!(
                menu.key(&input(Key::Character(key), true), &state, false, true),
                (false, None)
            );
        }
        menu.open_root();
        menu.key(&input(Key::Up, false), &state, false, true);
        assert_eq!(
            menu.key(&input(Key::Enter, false), &state, false, true),
            (true, Some(Command::App(MenuAction::ResetGuest)))
        );
        assert!(!menu.blocks_pointer());
    }

    #[test]
    fn keyboard_can_select_mount_target_and_kind() {
        let mut menu = Menu::default();
        let state = MenuState::default();
        assert_eq!(
            menu.key(&input(Key::Function(10), false), &state, false, true),
            (true, None)
        );
        menu.key(&input(Key::Enter, false), &state, false, true);
        menu.key(&input(Key::Character('z'), false), &state, false, true);
        menu.key(&input(Key::Down, false), &state, false, true);
        menu.key(&input(Key::Down, false), &state, false, true);
        assert_eq!(
            menu.key(&input(Key::Enter, false), &state, false, true),
            (true, Some(Command::Mount(MountKind::SessionZip, 'Z')))
        );
        assert!(!menu.blocks_pointer());
    }

    #[test]
    fn save_and_load_commands_follow_the_guest_and_supported_io() {
        for (running, batch_active, save_state_blocked, enabled) in [
            (false, false, false, [false, true]),
            (true, false, false, [true, true]),
            (true, true, false, [true, true]),
            (true, true, true, [false, true]),
        ] {
            let state = MenuState {
                running,
                batch_active,
                save_state_blocked,
                ..Default::default()
            };
            let commands: Vec<_> = options(Tab::View, &state, 'C', false)
                .into_iter()
                .filter_map(|item| match item.command {
                    Command::GuestState { save } => Some((save, item.enabled)),
                    _ => None,
                })
                .collect();
            assert_eq!(commands, [(true, enabled[0]), (false, enabled[1])]);
        }
    }

    #[test]
    fn state_slots_offer_saves_only_for_a_supported_running_guest() {
        for (running, save_state_blocked, save_enabled) in [
            (false, false, false),
            (true, false, true),
            (true, true, false),
        ] {
            let state = MenuState {
                running,
                save_state_blocked,
                state_slots_available: true,
                ..Default::default()
            };
            let items = options(Tab::StateSlots, &state, 'C', false);
            assert_eq!(items.len(), 8);
            for (items, slot) in items[..6].as_chunks::<2>().0.iter().zip([
                StateSlot::One,
                StateSlot::Two,
                StateSlot::Three,
            ]) {
                assert_eq!(
                    items[0].command,
                    Command::App(MenuAction::SaveStateSlot(slot))
                );
                assert_eq!(items[0].enabled, save_enabled);
                assert_eq!(
                    items[1].command,
                    Command::App(MenuAction::LoadStateSlot(slot))
                );
                assert!(items[1].enabled);
            }
        }
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
            menu.key(&input(Key::Enter, false), &state, false, true),
            (true, None)
        );
        menu.open(Tab::Speed, &state);
        assert_eq!(
            menu.key(&input(Key::Enter, false), &state, false, true),
            (true, Some(Command::App(MenuAction::SetSpeed(Speed::Slow))))
        );
    }

    #[test]
    fn unmount_targets_the_selected_mounted_drive_and_respects_batch_lock() {
        let mut menu = Menu::default();
        let mut state = MenuState::default();
        state.drives.insert('D', "Folder (read-only)");
        menu.open(Tab::Drives, &state);
        menu.key(&input(Key::Character('d'), false), &state, false, true);
        menu.key(&input(Key::Up, false), &state, false, true);
        assert_eq!(
            menu.key(&input(Key::Enter, false), &state, false, true),
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
        menu.open(Tab::Audio, &state);
        assert_eq!(
            menu.key(&input(Key::Enter, false), &state, false, true),
            (true, Some(Command::App(MenuAction::SetMuted(true))))
        );
        assert_eq!(state.audio_gain(), 0.5);
        state.muted = true;
        assert_eq!(state.audio_gain(), 0.0);
        menu.open(Tab::Audio, &state);
        menu.key(&input(Key::Down, false), &state, false, true);
        assert_eq!(
            menu.key(&input(Key::Enter, false), &state, false, true),
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
            menu.key(&input(Key::Up, false), &state, false, true);
            assert_eq!(
                menu.key(&input(Key::Enter, false), &state, false, true),
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
            menu.key(&input(Key::Function(10), false), &state, false, true),
            (false, None)
        );
        menu.open(Tab::Debug, &state);
        assert_eq!(
            menu.key(&input(Key::Enter, false), &state, false, true),
            (true, Some(Command::App(MenuAction::SetInspection(true))))
        );
        state.paused = true;
        state.inspection.push("AX 0000".into());
        assert_eq!(
            menu.key(&input(Key::Function(10), false), &state, false, true),
            (true, Some(Command::App(MenuAction::StepInstruction)))
        );
        let frame = menu.paint(PhysicalSize::new(800, 500), 1.0, &state, false, true);
        assert!(frame.height > BAR_HEIGHT);
        assert_eq!(&frame.rgba[(800 * BAR_HEIGHT as usize) * 4..][..4], &PANEL);
        let frame = menu.paint(PhysicalSize::new(800, 500), 1.0, &state, false, false);
        assert_eq!(frame.height, 30);
        assert_eq!(&frame.rgba[..4], &PANEL);
    }

    #[test]
    fn dpi_adjusted_clicks_match_the_overlay_rows() {
        let mut menu = Menu::default();
        let state = MenuState::default();
        menu.move_pointer(PhysicalPosition::new(20.0, 16.0), 2.0);
        assert_eq!(menu.mouse(true, true, &state, false), (true, None));
        assert_eq!(menu.mouse(false, true, &state, false), (true, None));
        assert!(menu.blocks_pointer());
        let frame = menu.paint(PhysicalSize::new(800, 500), 2.0, &state, false, true);
        assert_eq!(frame.width, 400);
        let row = (Panel::Root.rows_top() * frame.width) as usize;
        assert_eq!(frame.rgba[4 * (row + 16) + 3], 255);
        assert_eq!(frame.rgba[4 * (row + 380) + 3], 0);
        menu.move_pointer(PhysicalPosition::new(24.0, 106.0), 2.0);
        assert_eq!(menu.mouse(true, true, &state, false), (true, None));
        assert_eq!(menu.mouse(false, true, &state, false), (true, None));
        menu.move_pointer(PhysicalPosition::new(24.0, 194.0), 2.0);
        assert_eq!(
            menu.mouse(true, true, &state, false),
            (true, Some(Command::Mount(MountKind::Folder, 'C')))
        );
        assert_eq!(menu.mouse(false, true, &state, false), (true, None));
    }
}
