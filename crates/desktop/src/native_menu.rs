//! Windows caption popup. Open requests are handled by the winit loop.

use std::{cell::Cell, io, ptr::null, sync::Arc};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWINDOWATTRIBUTE, DwmSetWindowAttribute},
        Gdi::ClientToScreen,
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            AppendMenuW, CreatePopupMenu, DestroyMenu, GetWindowRect, HMENU, HTCAPTION, HTSYSMENU,
            IsWindow, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, TPM_NONOTIFY,
            TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_NCDESTROY, WM_NCLBUTTONDBLCLK,
            WM_NCLBUTTONDOWN, WM_NCRBUTTONUP,
        },
    },
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

use crate::{
    MenuAction, MenuState,
    menu::{self, CATEGORIES, Command, Tab},
};

const SUBCLASS: usize = 1;
const BRAND_WIDTH: i32 = 95;

enum Action {
    Run(Command),
    SelectDrive(char),
}

#[derive(Clone, Copy)]
pub(crate) enum OpenAt {
    Title,
    Pointer(POINT),
}

#[derive(Default)]
struct Dispatch {
    enabled: Cell<bool>,
    pending: Cell<Option<OpenAt>>,
}

pub(crate) struct NativeMenu {
    window: Arc<Window>,
    hwnd: HWND,
    dispatch: Box<Dispatch>,
    selected_drive: Option<char>,
    caption: String,
}

impl NativeMenu {
    pub fn new(window: Arc<Window>) -> io::Result<Self> {
        let handle = window.window_handle().map_err(io::Error::other)?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("expected a Win32 window"));
        };
        let hwnd = handle.hwnd.get() as HWND;
        let dispatch = Box::<Dispatch>::default();
        // The heap allocation is stable and outlives the subclass registration.
        let context = (&*dispatch as *const Dispatch) as usize;
        if unsafe { SetWindowSubclass(hwnd, Some(on_message), SUBCLASS, context) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // Older Windows releases use their standard theme when these attributes are unavailable.
        set_caption_color(hwnd, DWMWA_CAPTION_COLOR, [29, 34, 44]);
        set_caption_color(hwnd, DWMWA_TEXT_COLOR, [222, 229, 240]);
        Ok(Self {
            window,
            hwnd,
            dispatch,
            selected_drive: None,
            caption: "Dosa".into(),
        })
    }

    pub fn sync(&mut self, state: &MenuState, fullscreen: bool) {
        self.dispatch.enabled.set(state.enabled && !fullscreen);
        let caption = if state.enabled {
            caption(state)
        } else {
            String::from("Dosa")
        };
        if self.caption != caption {
            self.window.set_title(&caption);
            self.caption = caption;
        }
        if !self.dispatch.enabled.get() {
            self.dispatch.pending.set(None);
        }
    }

    pub fn request_keyboard_open(&self) -> bool {
        if !self.dispatch.enabled.get() {
            return false;
        }
        self.dispatch.pending.set(Some(OpenAt::Title));
        true
    }

    pub fn take_request(&self) -> Option<OpenAt> {
        self.dispatch.pending.take()
    }

    pub fn show(&mut self, state: &MenuState, at: OpenAt) -> io::Result<Option<Command>> {
        let point = match at {
            OpenAt::Pointer(point) => point,
            OpenAt::Title => {
                let mut point = POINT {
                    x: scaled(self.hwnd, 8),
                    y: 0,
                };
                if unsafe { ClientToScreen(self.hwnd, &mut point) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                point
            }
        };
        let drive = self
            .selected_drive
            .or(state.write_blocked_zip)
            .unwrap_or(state.active_drive);
        let (root, mut actions) = build(state, drive)?;
        let command_id = unsafe {
            TrackPopupMenuEx(
                root.0,
                TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                self.hwnd,
                null(),
            )
        };
        let action = usize::try_from(command_id)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| actions.get_mut(index))
            .and_then(Option::take);
        Ok(match action {
            Some(Action::Run(command)) => Some(command),
            Some(Action::SelectDrive(drive)) => {
                self.selected_drive = Some(drive);
                None
            }
            None => None,
        })
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        // The callback context must outlive its registration.
        unsafe {
            if IsWindow(self.hwnd) != 0 {
                RemoveWindowSubclass(self.hwnd, Some(on_message), SUBCLASS);
            }
        }
    }
}

unsafe extern "system" fn on_message(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    context: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(hwnd, Some(on_message), id) };
    } else if matches!(
        message,
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK | WM_NCRBUTTONUP
    ) {
        let dispatch = unsafe { &*(context as *const Dispatch) };
        let point = POINT {
            x: i32::from(lparam as i16),
            y: i32::from((lparam >> 16) as i16),
        };
        let hit = wparam as u32;
        if dispatch.enabled.get() && (hit == HTCAPTION || hit == HTSYSMENU) {
            if message == WM_NCRBUTTONUP {
                dispatch.pending.set(Some(OpenAt::Pointer(point)));
                return 0;
            }
            if in_brand(hwnd, point.x) {
                if message == WM_NCLBUTTONDOWN {
                    dispatch.pending.set(Some(OpenAt::Title));
                }
                return 0;
            }
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

fn in_brand(hwnd: HWND, screen_x: i32) -> bool {
    let mut bounds = RECT::default();
    (unsafe { GetWindowRect(hwnd, &mut bounds) != 0 })
        && screen_x >= bounds.left
        && screen_x < bounds.left + scaled(hwnd, BRAND_WIDTH)
}

fn scaled(hwnd: HWND, logical: i32) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    logical * i32::try_from(dpi).unwrap_or(96).max(96) / 96
}

fn set_caption_color(hwnd: HWND, attribute: DWMWINDOWATTRIBUTE, [r, g, b]: [u8; 3]) {
    let color = u32::from_le_bytes([r, g, b, 0]);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute as u32,
            (&raw const color).cast(),
            size_of::<u32>() as u32,
        );
    }
}

struct OwnedMenu(HMENU);

impl OwnedMenu {
    fn new() -> io::Result<Self> {
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(menu))
        }
    }

    fn append(&self, flags: u32, id: usize, label: &str) -> io::Result<()> {
        let label: Vec<u16> = label.encode_utf16().chain([0]).collect();
        if unsafe { AppendMenuW(self.0, flags, id, label.as_ptr()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn popup(&self, label: &str, menu: Self) -> io::Result<()> {
        self.append(MF_POPUP | MF_STRING, menu.0 as usize, label)?;
        std::mem::forget(menu); // The parent menu now owns it.
        Ok(())
    }
}

impl Drop for OwnedMenu {
    fn drop(&mut self) {
        unsafe { DestroyMenu(self.0) };
    }
}

fn build(state: &MenuState, drive: char) -> io::Result<(OwnedMenu, Vec<Option<Action>>)> {
    let root = OwnedMenu::new()?;
    let mut actions = Vec::new();
    let mut add = |menu: &OwnedMenu, label: &str, action, enabled, chosen| {
        let flags =
            MF_STRING | if enabled { 0 } else { MF_GRAYED } | if chosen { MF_CHECKED } else { 0 };
        menu.append(flags, actions.len() + 1, label)?;
        actions.push(enabled.then_some(action));
        Ok::<_, io::Error>(())
    };
    for (tab, label) in CATEGORIES {
        let category = OwnedMenu::new()?;
        if tab == Tab::Drives {
            let drives = OwnedMenu::new()?;
            for letter in 'A'..='Z' {
                let info = state.drives.get(&letter).copied().unwrap_or("Not mounted");
                add(
                    &drives,
                    &format!("{letter}:  {info}"),
                    Action::SelectDrive(letter),
                    true,
                    letter == drive,
                )?;
            }
            category.popup(&format!("Drive {drive}:"), drives)?;
            category.append(MF_SEPARATOR, 0, "")?;
        }
        for item in menu::options(tab, state, drive, false) {
            let label = if matches!(tab, Tab::Cpu | Tab::Speed)
                || matches!(item.command, Command::Fullscreen)
            {
                format!("&{}", item.label)
            } else {
                item.label.to_owned()
            };
            add(
                &category,
                &label,
                Action::Run(item.command),
                item.enabled,
                item.chosen,
            )?;
        }
        let label = match tab {
            Tab::Debug => String::from("De&bug"),
            _ => format!("&{label}"),
        };
        root.popup(&label, category)?;
    }
    root.append(MF_SEPARATOR, 0, "")?;
    add(
        &root,
        "&Reset guest",
        Action::Run(Command::App(MenuAction::ResetGuest)),
        true,
        false,
    )?;
    Ok((root, actions))
}

fn caption(state: &MenuState) -> String {
    let mut title = format!(
        "Dosa ▾    {}  ·  {}  ·  {}:",
        menu::cpu_label(state.cpu),
        menu::speed_label(state.speed),
        state.active_drive
    );
    if state.paused {
        title.push_str("  ·  Paused");
    }
    if state.recording {
        title.push_str("  ·  Recording");
    }
    if !state.status.is_empty() {
        title.push_str("  —  ");
        title.push_str(&state.status);
    }
    title
}
