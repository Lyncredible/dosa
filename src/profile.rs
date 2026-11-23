use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};

use dosa::display::Settings;
use dosa_desktop::{CpuMode, Speed, TextMode};
use serde::Deserialize;

#[derive(Default)]
pub struct LaunchProfile {
    pub mounts: Vec<(char, PathBuf)>,
    pub command: Option<String>,
    pub settings: Settings,
}

#[derive(Default)]
pub struct SettingsOverrides {
    pub cpu: Option<CpuMode>,
    pub speed: Option<Speed>,
    pub text_mode: Option<TextMode>,
}

impl SettingsOverrides {
    pub fn apply(self, base: Settings) -> Settings {
        Settings {
            cpu: self.cpu.unwrap_or(base.cpu),
            speed: self.speed.unwrap_or(base.speed),
            text_mode: self.text_mode.unwrap_or(base.text_mode),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileFile {
    exec: Option<String>,
    cpu: Option<CpuValue>,
    speed: Option<String>,
    text_rows: Option<u8>,
    #[serde(default)]
    mounts: Vec<MountEntry>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CpuValue {
    Number(u16),
    Text(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MountEntry {
    drive: String,
    path: PathBuf,
}

impl LaunchProfile {
    pub fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let source = fs::read_to_string(path)
            .map_err(|error| format!("cannot read launch profile {}: {error}", path.display()))?;
        Self::parse(&source, path)
            .map_err(|error| format!("invalid launch profile {}: {error}", path.display()).into())
    }

    fn parse(source: &str, path: &Path) -> Result<Self, Box<dyn Error>> {
        let profile: ProfileFile = toml::from_str(source)?;
        let cpu = profile
            .cpu
            .map(|value| match value {
                CpuValue::Number(value) => parse_cpu(&value.to_string()),
                CpuValue::Text(value) => parse_cpu(&value),
            })
            .transpose()?;
        let speed = profile.speed.as_deref().map(parse_speed).transpose()?;
        let text_mode = profile
            .text_rows
            .map(|value| parse_text_rows(&value.to_string()))
            .transpose()?;
        let settings = SettingsOverrides {
            cpu,
            speed,
            text_mode,
        }
        .apply(Settings::default());

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut mounts = Vec::new();
        for entry in profile.mounts {
            let [drive] = entry.drive.as_bytes() else {
                return Err(
                    format!("mount drive must be one letter A-Z, got {:?}", entry.drive).into(),
                );
            };
            if !drive.is_ascii_alphabetic() {
                return Err(
                    format!("mount drive must be one letter A-Z, got {:?}", entry.drive).into(),
                );
            }
            let drive = char::from(drive.to_ascii_uppercase());
            if mounts.iter().any(|(existing, _)| *existing == drive) {
                return Err(format!("mount drive {drive} appears more than once").into());
            }
            if entry.path.as_os_str().is_empty() {
                return Err(format!("mount drive {drive} needs a host path").into());
            }
            let path = if entry.path.is_absolute() {
                entry.path
            } else {
                if entry.path.has_root()
                    || matches!(entry.path.components().next(), Some(Component::Prefix(_)))
                {
                    return Err(format!(
                        "mount drive {drive} path {} must be absolute or relative to the profile directory",
                        entry.path.display()
                    )
                    .into());
                }
                base.join(entry.path)
            };
            mounts.push((drive, path));
        }
        Ok(Self {
            mounts,
            command: profile.exec,
            settings,
        })
    }

    pub fn apply_mount_overrides(&mut self, cli_mounts: Vec<(char, PathBuf)>) {
        let profile_count = self.mounts.len();
        for mount in cli_mounts {
            // Preserve the first drive selected by the profile and the native
            // ordering of CLI mounts that add new drives.
            if let Some(existing) = self.mounts[..profile_count]
                .iter_mut()
                .find(|(drive, _)| *drive == mount.0)
            {
                *existing = mount;
            } else {
                self.mounts.push(mount);
            }
        }
    }
}

pub fn parse_cpu(value: &str) -> Result<CpuMode, String> {
    match value {
        "8086" => Ok(CpuMode::I8086),
        "286" => Ok(CpuMode::I286),
        "386" => Ok(CpuMode::I386),
        _ => Err(format!("cpu must be 8086, 286, or 386, got {value:?}")),
    }
}

pub fn parse_speed(value: &str) -> Result<Speed, String> {
    match value {
        "slow" => Ok(Speed::Slow),
        "normal" => Ok(Speed::Normal),
        "fast" => Ok(Speed::Fast),
        "max" | "maximum" => Ok(Speed::Maximum),
        _ => Err(format!(
            "speed must be slow, normal, fast, or maximum, got {value:?}"
        )),
    }
}

pub fn parse_text_rows(value: &str) -> Result<TextMode, String> {
    match value {
        "25" => Ok(TextMode::Rows25),
        "43" => Ok(TextMode::Rows43),
        "50" => Ok(TextMode::Rows50),
        _ => Err(format!("text rows must be 25, 43, or 50, got {value:?}")),
    }
}
