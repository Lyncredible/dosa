use std::error::Error;
use std::fs;
use std::path::{Component, Path, PathBuf};

use dosa::display::Settings;
use dosa_desktop::{CpuMode, Speed, TextMode};
use serde::Deserialize;

#[derive(Default)]
pub struct LaunchProfile {
    pub mounts: Vec<StartupMount>,
    pub command: Option<String>,
    pub settings: Settings,
}

pub struct StartupMount {
    pub drive: char,
    pub path: PathBuf,
    pub access: MountAccess,
}

pub enum MountAccess {
    ReadOnly,
    WritableDirectory,
    SessionZip,
    FatImage,
    IsoImage,
}

#[derive(Default)]
pub struct SettingsOverrides {
    pub cpu: Option<CpuMode>,
    pub speed: Option<Speed>,
    pub text_mode: Option<TextMode>,
    pub text_columns: Option<u16>,
}

impl SettingsOverrides {
    pub fn apply(self, base: Settings) -> Settings {
        let columns = self.text_columns.unwrap_or(match base.text_mode {
            TextMode::WideRows25 | TextMode::WideRows43 | TextMode::WideRows50 => 132,
            _ => 80,
        });
        let rows = self.text_mode.unwrap_or(base.text_mode);
        Settings {
            cpu: self.cpu.unwrap_or(base.cpu),
            speed: self.speed.unwrap_or(base.speed),
            text_mode: match (columns, rows) {
                (132, TextMode::Rows25 | TextMode::WideRows25) => TextMode::WideRows25,
                (132, TextMode::Rows43 | TextMode::WideRows43) => TextMode::WideRows43,
                (132, _) => TextMode::WideRows50,
                (_, TextMode::WideRows25) => TextMode::Rows25,
                (_, TextMode::WideRows43) => TextMode::Rows43,
                (_, TextMode::WideRows50) => TextMode::Rows50,
                (_, rows) => rows,
            },
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
    text_columns: Option<u16>,
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
    #[serde(default)]
    writable: bool,
    #[serde(default)]
    fat_image: bool,
    #[serde(default)]
    iso_image: bool,
    #[serde(default)]
    session_zip: bool,
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
            text_columns: profile
                .text_columns
                .map(|columns| parse_text_columns(&columns.to_string()))
                .transpose()?,
        }
        .apply(Settings::default());

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut mounts: Vec<StartupMount> = Vec::new();
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
            if mounts.iter().any(|existing| existing.drive == drive) {
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
            if entry.writable && entry.fat_image {
                return Err(format!("FAT image on drive {drive} must be read-only").into());
            }
            if entry.session_zip && (entry.writable || entry.fat_image) {
                return Err(format!(
                    "session_zip on drive {drive} cannot be combined with writable or fat_image"
                )
                .into());
            }
            if entry.iso_image && (entry.writable || entry.fat_image || entry.session_zip) {
                return Err(format!("iso_image on drive {drive} cannot be combined with writable, fat_image, or session_zip").into());
            }
            let access = if entry.iso_image {
                MountAccess::IsoImage
            } else if entry.session_zip {
                MountAccess::SessionZip
            } else if entry.fat_image {
                MountAccess::FatImage
            } else if entry.writable {
                MountAccess::WritableDirectory
            } else {
                MountAccess::ReadOnly
            };
            mounts.push(StartupMount {
                drive,
                path,
                access,
            });
        }
        Ok(Self {
            mounts,
            command: profile.exec,
            settings,
        })
    }

    pub fn apply_mount_overrides(&mut self, cli_mounts: Vec<StartupMount>) {
        let profile_count = self.mounts.len();
        for mount in cli_mounts {
            // Preserve the first drive selected by the profile and the native
            // ordering of CLI mounts that add new drives.
            if let Some(existing) = self.mounts[..profile_count]
                .iter_mut()
                .find(|existing| existing.drive == mount.drive)
            {
                *existing = mount;
            } else {
                self.mounts.push(mount);
            }
        }
    }
}

pub fn parse_text_columns(value: &str) -> Result<u16, String> {
    match value {
        "80" => Ok(80),
        "132" => Ok(132),
        _ => Err("text columns must be 80 or 132".into()),
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

#[cfg(test)]
mod wide_tests {
    use super::*;

    #[test]
    fn row_and_column_overrides_preserve_the_other_dimension() {
        let profile =
            LaunchProfile::parse("text_columns = 132\ntext_rows = 43", Path::new("demo.toml"))
                .unwrap();
        assert_eq!(profile.settings.text_mode, TextMode::WideRows43);
        let settings = SettingsOverrides {
            text_mode: Some(TextMode::Rows50),
            ..Default::default()
        }
        .apply(profile.settings);
        assert_eq!(settings.text_mode, TextMode::WideRows50);
        let settings = SettingsOverrides {
            text_columns: Some(80),
            ..Default::default()
        }
        .apply(settings);
        assert_eq!(settings.text_mode, TextMode::Rows50);
        assert!(parse_text_columns("256").is_err());
    }
}
