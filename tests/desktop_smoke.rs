//! Optional native-window smoke test with a caller-provided DOS archive.

use std::cell::Cell;
use std::collections::HashSet;
use std::error::Error;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dosa::display::Desktop;
use dosa::shell::Shell;
use dosa_desktop::{
    App, Frame, Input, Key, MenuAction, MenuState, MountKind, PointerEvent, PointerMode,
};

fn main() -> Result<(), Box<dyn Error>> {
    let Some(archive) = std::env::var_os("DOSA_SMOKE_ARCHIVE") else {
        println!("desktop smoke skipped; set DOSA_SMOKE_ARCHIVE to enable it");
        return Ok(());
    };
    let program = std::env::var("DOSA_SMOKE_PROGRAM")?;
    let key_count: u32 = std::env::var("DOSA_SMOKE_ENTER_COUNT")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    let min_colors: usize = std::env::var("DOSA_SMOKE_MIN_COLORS")
        .unwrap_or_else(|_| "3".into())
        .parse()?;
    let mut mount_arg = std::ffi::OsString::from("C=");
    mount_arg.push(&archive);
    let listing = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg("--mount")
        .arg(mount_arg)
        .args(["--headless", "--exec", "DIR"])
        .output()?;
    assert!(
        listing.status.success(),
        "DOS DIR failed: {:?}",
        listing.stderr
    );
    let listing = String::from_utf8(listing.stdout)?;
    assert!(
        listing.lines().any(|line| {
            line.split_ascii_whitespace()
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(&program))
        }),
        "archive listing does not contain the selected program"
    );

    let mut desktop = Desktop::new(Shell::new(), None);
    desktop.menu_action(MenuAction::Mount {
        drive: 'C',
        source: archive.into(),
        kind: MountKind::Zip,
    });
    let status = desktop.menu().status;
    assert_eq!(status, "Mounted C:", "desktop mount failed");
    desktop.input(key_event(Key::Character(' '), Some(program), true));
    desktop.input(key_event(Key::Enter, None, true));
    let result = Rc::new(Cell::new(None));
    let now = Instant::now();
    dosa_desktop::run(Smoke {
        desktop,
        started: now,
        changed_at: now,
        sample_at: now,
        key_frame: None,
        keys_remaining: key_count,
        result: Rc::clone(&result),
        fingerprint: None,
        min_colors,
    })?;
    result
        .get()
        .ok_or("desktop closed before the smoke test finished")??;
    println!(
        "desktop smoke passed: archive mounted, graphics rendered, {key_count} input transitions"
    );
    Ok(())
}

struct Smoke {
    desktop: Desktop,
    started: Instant,
    changed_at: Instant,
    sample_at: Instant,
    key_frame: Option<u64>,
    keys_remaining: u32,
    result: Rc<Cell<Option<Result<(), &'static str>>>>,
    fingerprint: Option<u64>,
    min_colors: usize,
}

impl App for Smoke {
    fn tick(&mut self) {
        self.desktop.tick();
        let now = Instant::now();
        if now.duration_since(self.started) >= Duration::from_secs(120) {
            self.result.set(Some(Err(
                "timed out waiting for guest graphics or input response",
            )));
            return;
        }
        if now < self.sample_at || !self.desktop.guest_graphics_active() {
            return;
        }
        self.sample_at = now + Duration::from_millis(125);
        let frame = self.desktop.frame();
        let colors: HashSet<_> = frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .take(640 * 480)
            .collect();
        if colors.len() < self.min_colors {
            self.changed_at = now;
            self.fingerprint = None;
            return;
        }
        let mut hasher = DefaultHasher::new();
        (frame.width, frame.height, frame.rgba).hash(&mut hasher);
        let hash = hasher.finish();
        if self.fingerprint.replace(hash) != Some(hash) {
            self.changed_at = now;
        }
        if now.duration_since(self.changed_at) < Duration::from_millis(400)
            || self.key_frame == Some(hash)
        {
            return;
        }

        if self.keys_remaining == 0 {
            self.result.set(Some(Ok(())));
            return;
        }
        self.keys_remaining -= 1;
        self.key_frame = Some(hash);
        self.changed_at = now;
        for pressed in [true, false] {
            self.desktop.input(key_event(Key::Enter, None, pressed));
        }
    }

    fn frame(&self) -> Frame<'_> {
        self.desktop.frame()
    }

    fn input(&mut self, event: Input) {
        self.desktop.input(event);
    }

    fn should_exit(&self) -> bool {
        self.result.get().is_some() || self.desktop.should_exit()
    }

    fn display_aspect(&self) -> Option<f32> {
        self.desktop.display_aspect()
    }

    fn menu(&self) -> MenuState {
        self.desktop.menu()
    }

    fn menu_action(&mut self, action: MenuAction) {
        self.desktop.menu_action(action);
    }

    fn pointer(&mut self, event: PointerEvent) {
        self.desktop.pointer(event);
    }

    fn pointer_mode(&self) -> PointerMode {
        self.desktop.pointer_mode()
    }
}

fn key_event(key: Key, text: Option<String>, pressed: bool) -> Input {
    Input {
        scan_code: if key == Key::Enter { 0x1c } else { 0 },
        key,
        pressed,
        text,
        modifiers: Default::default(),
    }
}
