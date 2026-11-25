use std::collections::BTreeMap;

use crate::{DosError, sound_blaster};

pub(crate) const CAPACITY: usize = 32 * 1024;
// Allow the executable path and the DOS environment trailer to fit after the variables.
const MAX_VARIABLE_BYTES: usize = CAPACITY - 544;

pub struct Environment(BTreeMap<String, String>);

impl Default for Environment {
    fn default() -> Self {
        let (name, value) = sound_blaster::ENVIRONMENT
            .split_once('=')
            .expect("the default BLASTER variable has a value");
        Self(BTreeMap::from([
            ("PATH".into(), "\\".into()),
            ("COMSPEC".into(), "COMMAND.COM".into()),
            (name.into(), value.into()),
        ]))
    }
}

impl Environment {
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&name.to_ascii_uppercase()).map(String::as_str)
    }

    pub fn variables(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
    }

    /// Set a variable for future child programs. An empty value removes it.
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), DosError> {
        if name.is_empty()
            || !name.is_ascii()
            || name
                .bytes()
                .any(|byte| byte.is_ascii_control() || byte == b'=')
            || !value.is_ascii()
            || value.contains(['\0', '\r', '\n'])
        {
            return Err(DosError::InvalidEnvironment(
                "invalid DOS environment variable",
            ));
        }
        let name = name.to_ascii_uppercase();
        if value.is_empty() {
            self.0.remove(&name);
            return Ok(());
        }
        let other_bytes: usize = self
            .0
            .iter()
            .filter(|&(key, _)| key != &name)
            .map(|(key, value)| key.len() + value.len() + 2)
            .sum();
        if other_bytes + name.len() + value.len() + 2 > MAX_VARIABLE_BYTES {
            return Err(DosError::InvalidEnvironment("out of environment space"));
        }
        self.0.insert(name, value.to_owned());
        Ok(())
    }

    pub(crate) fn encode(&self, program_name: &str) -> Result<Vec<u8>, DosError> {
        let mut bytes = Vec::new();
        for (name, value) in self.variables() {
            bytes.extend(name.as_bytes());
            bytes.push(b'=');
            bytes.extend(value.as_bytes());
            bytes.push(0);
        }
        if bytes.is_empty() {
            bytes.push(0);
        }
        bytes.extend([0, 1, 0]);
        bytes.extend(program_name.as_bytes());
        bytes.push(0);
        if bytes.len() > CAPACITY {
            return Err(DosError::InvalidEnvironment(
                "program name and variables exceed environment space",
            ));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_environment_keeps_the_double_null_terminator() {
        let mut environment = Environment::default();
        for name in ["BLASTER", "COMSPEC", "PATH"] {
            environment.set(name, "").unwrap();
        }
        assert_eq!(
            environment.encode("TEST.COM").unwrap(),
            b"\0\0\x01\0TEST.COM\0"
        );
    }

    #[test]
    fn overflowing_an_environment_variable_keeps_its_old_value() {
        let mut environment = Environment::default();
        environment.set("name", "old").unwrap();
        assert!(environment.set("NAME", &"X".repeat(CAPACITY)).is_err());
        assert_eq!(environment.get("Name"), Some("old"));
    }
}
