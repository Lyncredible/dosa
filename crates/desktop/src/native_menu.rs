//! Windows non-client menu bar. Commands are queued for the winit loop.

use std::{cell::RefCell, collections::BTreeMap, io, ptr::null_mut, sync::Arc};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            AppendMenuW, CreateMenu, CreatePopupMenu, DestroyMenu, DrawMenuBar, HMENU, IsWindow,
            MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, SetMenu, WM_COMMAND,
            WM_NCDESTROY,
        },
    },
};
use winit::{
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
    window::Window,
};

use crate::{
    MenuAction, MenuState,
    menu::{self, CATEGORIES, Command, Item, Tab},
};

const SUBCLASS: usize = 1;
const FIRST_COMMAND: usize = 0x1000;

#[derive(Clone)]
enum Action {
    Run(Command),
    SelectDrive(char),
}

#[derive(Default)]
struct Dispatch {
    actions: Vec<Option<Action>>,
    pending: Vec<Action>,
}

#[derive(PartialEq, Eq)]
struct Model {
    groups: Vec<Vec<Item>>,
    drives: BTreeMap<char, &'static str>,
    drive: char,
}

pub(crate) struct NativeMenu {
    window: Arc<Window>,
    hwnd: HWND,
    dispatch: Box<RefCell<Dispatch>>,
    bar: Option<OwnedMenu>,
    model: Option<Model>,
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
        let dispatch = Box::<RefCell<Dispatch>>::default();
        // The heap allocation is stable and outlives the subclass registration.
        let context = (&*dispatch as *const RefCell<Dispatch>) as usize;
        if unsafe { SetWindowSubclass(hwnd, Some(on_message), SUBCLASS, context) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            window,
            hwnd,
            dispatch,
            bar: None,
            model: None,
            selected_drive: None,
            caption: "Dosa".into(),
        })
    }

    pub fn sync(&mut self, state: &MenuState, fullscreen: bool) -> io::Result<()> {
        let caption = if state.enabled {
            caption(state)
        } else {
            String::from("Dosa")
        };
        if self.caption != caption {
            self.window.set_title(&caption);
            self.caption = caption;
        }
        if !state.enabled || fullscreen {
            if self.bar.is_some() {
                self.attach(null_mut())?;
                self.bar = None;
                self.model = None;
                self.dispatch.borrow_mut().actions.clear();
            }
            return Ok(());
        }
        let drive = self
            .selected_drive
            .or(state.write_blocked_zip)
            .unwrap_or(state.active_drive);
        let model = Model {
            groups: CATEGORIES
                .iter()
                .map(|&(tab, _)| menu::options(tab, state, drive, false))
                .collect(),
            drives: state.drives.clone(),
            drive,
        };
        if self.model.as_ref() == Some(&model) {
            return Ok(());
        }

        let (bar, actions) = build(&model)?;
        self.attach(bar.0)?;
        self.dispatch.borrow_mut().actions = actions;
        self.bar = Some(bar);
        self.model = Some(model);
        Ok(())
    }

    fn attach(&self, menu: HMENU) -> io::Result<()> {
        if unsafe { SetMenu(self.hwnd, menu) } == 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe { DrawMenuBar(self.hwnd) };
        Ok(())
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        let actions = std::mem::take(&mut self.dispatch.borrow_mut().pending);
        actions
            .into_iter()
            .filter_map(|action| match action {
                Action::Run(command) => Some(command),
                Action::SelectDrive(drive) => {
                    self.selected_drive = Some(drive);
                    None
                }
            })
            .collect()
    }
}

impl Drop for NativeMenu {
    fn drop(&mut self) {
        // The window also removes the subclass on destruction. If it's alive,
        // detach before the owned menu and callback context are freed.
        unsafe {
            if IsWindow(self.hwnd) != 0 {
                RemoveWindowSubclass(self.hwnd, Some(on_message), SUBCLASS);
                if self.bar.is_some() && SetMenu(self.hwnd, null_mut()) == 0 {
                    // The window still owns the attached menu.
                    std::mem::forget(self.bar.take());
                }
            } else {
                // Windows destroyed its attached menu with the HWND.
                std::mem::forget(self.bar.take());
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
    } else if message == WM_COMMAND
        && lparam == 0
        && (FIRST_COMMAND..=u16::MAX as usize).contains(&wparam)
    {
        // The menu is attached and used on the event-loop thread only.
        let dispatch = unsafe { &*(context as *const RefCell<Dispatch>) };
        if let Ok(mut dispatch) = dispatch.try_borrow_mut()
            && let Some(Some(action)) = dispatch.actions.get(wparam - FIRST_COMMAND).cloned()
        {
            dispatch.pending.push(action);
            return 0;
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

struct OwnedMenu(HMENU);

impl OwnedMenu {
    fn new(popup: bool) -> io::Result<Self> {
        let menu = unsafe {
            if popup {
                CreatePopupMenu()
            } else {
                CreateMenu()
            }
        };
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

fn build(model: &Model) -> io::Result<(OwnedMenu, Vec<Option<Action>>)> {
    let bar = OwnedMenu::new(false)?;
    let root = OwnedMenu::new(true)?;
    let mut actions = Vec::new();
    let mut add = |menu: &OwnedMenu, label: &str, action, enabled, chosen| {
        let flags =
            MF_STRING | if enabled { 0 } else { MF_GRAYED } | if chosen { MF_CHECKED } else { 0 };
        menu.append(flags, FIRST_COMMAND + actions.len(), label)?;
        actions.push(enabled.then_some(action));
        Ok::<_, io::Error>(())
    };
    for (&(tab, label), items) in CATEGORIES.iter().zip(&model.groups) {
        let category = OwnedMenu::new(true)?;
        if tab == Tab::Drives {
            let drives = OwnedMenu::new(true)?;
            for letter in 'A'..='Z' {
                let info = model.drives.get(&letter).copied().unwrap_or("Not mounted");
                add(
                    &drives,
                    &format!("{letter}:  {info}"),
                    Action::SelectDrive(letter),
                    true,
                    letter == model.drive,
                )?;
            }
            category.popup(&format!("Drive {}:", model.drive), drives)?;
            category.append(MF_SEPARATOR, 0, "")?;
        }
        for item in items {
            add(
                &category,
                item.label,
                Action::Run(item.command.clone()),
                item.enabled,
                item.chosen,
            )?;
        }
        root.popup(label, category)?;
    }
    root.append(MF_SEPARATOR, 0, "")?;
    add(
        &root,
        "Reset guest",
        Action::Run(Command::App(MenuAction::ResetGuest)),
        true,
        false,
    )?;
    bar.popup("Menu", root)?;
    Ok((bar, actions))
}

fn caption(state: &MenuState) -> String {
    let mut title = format!(
        "Dosa  ·  {}  ·  {}  ·  {}:",
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
