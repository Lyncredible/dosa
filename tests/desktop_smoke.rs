//! Optional native-window smoke test with a generated or caller-provided DOS archive.

use std::cell::Cell;
use std::collections::HashSet;
use std::error::Error;
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dosa::display::Desktop;
use dosa::shell::Shell;
use dosa_desktop::{
    App, Frame, Input, Key, MenuAction, MenuState, MountKind, PointerEvent, PointerMode, Scaling,
};

fn main() -> Result<(), Box<dyn Error>> {
    let generated;
    let mut wav_path = None;
    let (archive, program, key_count) =
        if let Some(archive) = std::env::var_os("DOSA_SMOKE_ARCHIVE") {
            let program = std::env::var("DOSA_SMOKE_PROGRAM")?;
            let key_count: u32 = std::env::var("DOSA_SMOKE_ENTER_COUNT")
                .unwrap_or_else(|_| "0".into())
                .parse()?;
            (PathBuf::from(archive), program, key_count)
        } else if std::env::var_os("DOSA_SMOKE_GENERATED").is_some() {
            generated = tempfile::tempdir()?;
            let archive = generated.path().join("desktop fixture.zip");
            wav_path = Some(generated.path().join("desktop capture.wav"));
            generate_archive(&archive)?;
            (archive, "SMOKE.COM".to_owned(), 1)
        } else {
            println!(
                "desktop smoke skipped; set DOSA_SMOKE_GENERATED or DOSA_SMOKE_ARCHIVE to enable it"
            );
            return Ok(());
        };
    let min_colors: usize = std::env::var("DOSA_SMOKE_MIN_COLORS")
        .unwrap_or_else(|_| "3".into())
        .parse()?;
    let session_zip = std::env::var_os("DOSA_SMOKE_SESSION").is_some();
    let final_enter_delay = std::env::var("DOSA_SMOKE_FINAL_ENTER_DELAY_MS")
        .ok()
        .map(|value| value.parse().map(Duration::from_millis))
        .transpose()?;
    let final_min_colors = std::env::var("DOSA_SMOKE_FINAL_MIN_COLORS")
        .ok()
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(min_colors);
    let mut mount_arg = std::ffi::OsString::from("C=");
    mount_arg.push(archive.as_os_str());
    let listing = Command::new(env!("CARGO_BIN_EXE_dosa"))
        .arg(if session_zip {
            "--mount-session"
        } else {
            "--mount"
        })
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
        source: archive,
        kind: if session_zip {
            MountKind::SessionZip
        } else {
            MountKind::Zip
        },
    });
    let status = desktop.menu().status;
    assert_eq!(status, "Mounted C:", "desktop mount failed");
    if std::env::var_os("DOSA_SMOKE_INTEGER_SCALING").is_some() {
        desktop.menu_action(MenuAction::SetScaling(Scaling::Integer));
        assert_eq!(desktop.menu().scaling, Scaling::Integer);
    }
    if let Some(path) = &wav_path {
        desktop.menu_action(MenuAction::SetMuted(true));
        desktop.menu_action(MenuAction::StartRecording(path.clone()));
        assert!(desktop.menu().recording);
    }
    desktop.input(key_event(Key::Character(' '), Some(program), true));
    desktop.input(key_event(Key::Enter, None, true));
    if wav_path.is_some() {
        desktop.menu_action(MenuAction::SetInspection(true));
        assert!(desktop.menu().paused);
    }
    let result = Rc::new(Cell::new(None));
    let now = Instant::now();
    dosa_desktop::run(Smoke {
        desktop,
        started: now,
        changed_at: now,
        sample_at: now,
        last_enter_at: now,
        key_frame: None,
        keys_remaining: key_count,
        final_enter_delay,
        result: Rc::clone(&result),
        fingerprint: None,
        min_colors,
        final_min_colors,
    })?;
    result
        .get()
        .ok_or("desktop closed before the smoke test finished")??;
    if let Some(path) = wav_path {
        let wav = std::fs::read(path)?;
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[24..28], &48_000u32.to_le_bytes());
        assert!(
            wav[44..]
                .as_chunks::<2>()
                .0
                .contains(&(-8192i16).to_le_bytes())
        );
    }
    println!(
        "desktop smoke passed: archive mounted, graphics rendered, {key_count} input transitions"
    );
    Ok(())
}

fn generate_archive(path: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let guest = [
        0xba, 0x2c, 0x02, 0xb0, 0xd1, 0xee, // Enable Sound Blaster DAC
        0xb0, 0x10, 0xee, 0xb0, 64, 0xee, // Set a known DAC level
        0xb8, 0x13, 0x00, 0xcd, 0x10, // INT 10h, set VGA mode 13h
        0xb8, 0x00, 0xa0, 0x8e, 0xc0, // MOV ES,A000h
        0x26, 0xc7, 0x06, 0, 0, 1, 2, // MOV WORD [ES:0],0201h
        0xb4, 0x00, 0xcd, 0x16, // INT 16h, wait for input
        0x26, 0xc6, 0x06, 0, 0, 3, // MOV BYTE [ES:0],3
        0xeb, 0xfe, // JMP $ to retain the graphics frame
    ];
    let mut archive = zip::ZipWriter::new(File::create(path)?);
    archive.start_file("SMOKE.COM", zip::write::SimpleFileOptions::default())?;
    archive.write_all(&guest)?;
    archive.finish()?;
    Ok(())
}

struct Smoke {
    desktop: Desktop,
    started: Instant,
    changed_at: Instant,
    sample_at: Instant,
    last_enter_at: Instant,
    key_frame: Option<u64>,
    keys_remaining: u32,
    final_enter_delay: Option<Duration>,
    result: Rc<Cell<Option<Result<(), &'static str>>>>,
    fingerprint: Option<u64>,
    min_colors: usize,
    final_min_colors: usize,
}

impl App for Smoke {
    fn audio(&mut self) -> Vec<f32> {
        if self.desktop.menu().recording {
            self.desktop.audio()
        } else {
            Vec::new()
        }
    }

    fn tick(&mut self) {
        self.desktop.tick();
        let now = Instant::now();
        let menu = self.desktop.menu();
        if menu.paused && !menu.inspection.is_empty() {
            let elapsed = now.duration_since(self.started);
            if elapsed >= Duration::from_millis(125)
                && menu
                    .inspection
                    .iter()
                    .any(|line| line.starts_with("IP 0100"))
            {
                self.desktop.menu_action(MenuAction::StepInstruction);
                assert!(
                    self.desktop
                        .menu()
                        .inspection
                        .iter()
                        .any(|line| line.starts_with("IP 0103"))
                );
            } else if elapsed >= Duration::from_millis(250) {
                self.desktop.menu_action(MenuAction::SetPaused(false));
                assert!(self.desktop.menu().inspection.is_empty());
            }
            return;
        }
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
        let min_colors = if self.keys_remaining == 0 {
            self.final_min_colors
        } else {
            self.min_colors
        };
        if colors.len() < min_colors {
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
        if self.keys_remaining == 0 && self.key_frame.is_some_and(|previous| previous != hash) {
            self.result.set(Some(Ok(())));
            return;
        }
        let settled = match self.final_enter_delay.filter(|_| self.keys_remaining == 1) {
            Some(delay) => now.duration_since(self.last_enter_at) >= delay,
            None => now.duration_since(self.changed_at) >= Duration::from_millis(400),
        };
        if !settled || self.key_frame == Some(hash) {
            return;
        }

        if self.keys_remaining == 0 {
            self.result.set(Some(Ok(())));
            return;
        }
        self.keys_remaining -= 1;
        self.last_enter_at = now;
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
