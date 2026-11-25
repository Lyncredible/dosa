use std::error::Error;

use super::{
    Command, Shell, clock::ClockInput, copy::CopyOperation, delete::Deletion, directory::Pager,
};

pub(super) enum PendingInput {
    Delete(Deletion),
    Copy(CopyOperation),
    Page(Pager),
    Clock(ClockInput),
}

impl Shell {
    pub fn input_waiting(&self) -> bool {
        self.pending_input.is_some()
    }

    pub fn line_input_waiting(&self) -> bool {
        matches!(self.pending_input, Some(PendingInput::Clock(_)))
    }

    pub fn pending_input_requirement(&self) -> &'static str {
        match self.pending_input {
            Some(PendingInput::Page(_)) => "DIR /P needs a key",
            Some(PendingInput::Clock(_)) => "DATE or TIME needs a line",
            _ => "confirmation needs Y or N",
        }
    }

    pub fn resume_input(&mut self, key: u8) -> Result<Option<Command>, Box<dyn Error>> {
        let Some(pending) = self.pending_input.take() else {
            return Ok(None);
        };
        if let PendingInput::Clock(clock) = pending {
            return Ok(clock.key(self, key));
        }
        let key = key.to_ascii_uppercase();
        let approved = key == b'Y';
        let result = match (pending, key) {
            (_, 3 | 27) => Ok(Command::Prompt),
            (PendingInput::Page(pager), _) => {
                return Ok(Some(pager.next(self, b"\r\n".to_vec())));
            }
            (PendingInput::Delete(deletion), b'Y' | b'N') => deletion.confirm(self, approved),
            (PendingInput::Copy(copy), b'Y' | b'N') => copy.confirm(self, approved),
            (pending, _) => {
                self.pending_input = Some(pending);
                return Ok(None);
            }
        };
        let command = result.map_err(|error| self.guest_failed(error))?;
        let mut output = match key {
            b'Y' => b"Y\r\n".to_vec(),
            b'N' => b"N\r\n".to_vec(),
            _ => b"\r\n".to_vec(),
        };
        Ok(Some(match command {
            Command::WaitForInput(prompt) => {
                output.extend(prompt);
                Command::WaitForInput(output)
            }
            Command::Print(text) => {
                output.extend(text);
                Command::Print(output)
            }
            _ => Command::Print(output),
        }))
    }
}
