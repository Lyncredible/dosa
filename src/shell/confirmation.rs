use std::error::Error;

use super::{Command, Shell, copy::CopyOperation, delete::Deletion};

pub(super) enum Confirmation {
    Delete(Deletion),
    Copy(CopyOperation),
}

impl Shell {
    pub fn confirmation_waiting(&self) -> bool {
        self.confirmation.is_some()
    }

    pub fn resume_confirmation(&mut self, key: u8) -> Result<Option<Command>, Box<dyn Error>> {
        let key = key.to_ascii_uppercase();
        if !matches!(key, b'Y' | b'N' | 3 | 27) {
            return Ok(None);
        }
        let Some(pending) = self.confirmation.take() else {
            return Ok(None);
        };
        let approved = key == b'Y';
        let result = match (pending, key) {
            (_, 3 | 27) => Ok(Command::Prompt),
            (Confirmation::Delete(deletion), _) => deletion.confirm(self, approved),
            (Confirmation::Copy(copy), _) => copy.confirm(self, approved),
        };
        let command = result.map_err(|error| self.guest_failed(error))?;
        let mut output = match key {
            b'Y' => b"Y\r\n".to_vec(),
            b'N' => b"N\r\n".to_vec(),
            _ => b"\r\n".to_vec(),
        };
        Ok(Some(match command {
            Command::Confirm(prompt) => {
                output.extend(prompt);
                Command::Confirm(output)
            }
            Command::Print(text) => {
                output.extend(text);
                Command::Print(output)
            }
            _ => Command::Print(output),
        }))
    }
}
