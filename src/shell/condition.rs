use std::error::Error;

use dosa_dos::matches_dos_pattern;
use dosa_mount::MountError;

use super::{Shell, split_word};

pub(super) const HELP: &str = "Batch conditions\r\n\
    IF [NOT] ERRORLEVEL number command\r\n\
    IF [NOT] string1==string2 command\r\n\
    IF [NOT] EXIST filename command\r\n";

impl Shell {
    pub(super) fn test_condition<'a>(
        &self,
        args: &'a str,
    ) -> Result<(bool, &'a str), Box<dyn Error>> {
        let (word, rest) = split_word(args)?;
        let negate = !args.starts_with('"') && word.eq_ignore_ascii_case("NOT");
        let condition = if negate { rest } else { args };
        let (predicate, rest) = split_word(condition)?;
        let (matched, command) = match predicate.to_ascii_uppercase().as_str() {
            "ERRORLEVEL" if !condition.starts_with('"') && !rest.starts_with("==") => {
                let (number, command) = split_word(rest)?;
                let threshold = number
                    .parse::<u8>()
                    .map_err(|_| "IF ERRORLEVEL requires a number from 0 to 255")?;
                (self.errorlevel >= threshold, command)
            }
            "EXIST" if !condition.starts_with('"') && !rest.starts_with("==") => {
                let (filename, command) = split_word(rest)?;
                if filename.is_empty() || command.is_empty() {
                    return Err("usage: IF [NOT] EXIST filename command".into());
                }
                (self.file_exists(filename)?, command)
            }
            _ => compare_strings(condition)?,
        };
        if command.is_empty() {
            return Err("IF requires a command after the condition".into());
        }
        Ok((matched != negate, command))
    }

    fn file_exists(&self, filename: &str) -> Result<bool, Box<dyn Error>> {
        let (drive, path) = self.resolve(filename)?;
        let Some(mount) = self.machine.drive(drive) else {
            return Ok(false);
        };
        let (parent, name) = path.rsplit_once('\\').unwrap_or(("", &path));
        let result = if name.contains(['*', '?']) {
            mount.list(parent).map(|entries| {
                entries
                    .iter()
                    .any(|entry| !entry.is_dir && matches_dos_pattern(&entry.name, name))
            })
        } else if name.eq_ignore_ascii_case("NUL") {
            mount.metadata(parent).map(|entry| entry.is_dir)
        } else {
            mount.metadata(&path).map(|entry| !entry.is_dir)
        };
        match result {
            Ok(matched) => Ok(matched),
            Err(error) if error.is_not_found() || matches!(error, MountError::NotDirectory(_)) => {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn compare_strings(condition: &str) -> Result<(bool, &str), &'static str> {
    const USAGE: &str = "usage: IF [NOT] string1==string2 command";
    let (left, rest) = if condition.starts_with('"') {
        quoted_operand(condition)?
    } else {
        let (left, _) = condition.split_once("==").ok_or(USAGE)?;
        let operand = left.trim_end();
        if operand.contains(char::is_whitespace) {
            return Err(USAGE);
        }
        (operand, &condition[left.len()..])
    };
    let rest = rest
        .trim_start()
        .strip_prefix("==")
        .ok_or(USAGE)?
        .trim_start();
    let (right, command) = if rest.starts_with('"') {
        quoted_operand(rest)?
    } else {
        split_word(rest)?
    };
    if right.is_empty() {
        return Err(USAGE);
    }
    Ok((left == right, command.trim_start()))
}

// Quotes are part of the strings being compared. They also allow whitespace
// and the comparison operator itself inside an operand.
fn quoted_operand(input: &str) -> Result<(&str, &str), &'static str> {
    let end = input[1..].find('"').ok_or("unterminated IF string")? + 2;
    Ok(input.split_at(end))
}
