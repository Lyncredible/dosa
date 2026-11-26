use std::error::Error;

use dosa_dos::ShellIo;

use super::{Command, Shell, split_word};

pub(super) const HELP: &str = "LOADHIGH [/L:region[,size][;region...]] [/S] program [arguments]\r\n\
    LH is an alias. Programs use conventional memory when upper memory is unavailable.\r\n";
const USAGE: &str = "usage: LH [/L:region[,size][;region...]] [/S] program [arguments]";

impl Shell {
    pub(super) fn load_high(
        &mut self,
        args: &str,
        io: &mut Option<ShellIo>,
    ) -> Result<Command, Box<dyn Error>> {
        if args == "/?" {
            return Ok(Command::Print(HELP.as_bytes().to_vec()));
        }
        let mut regions = false;
        let mut shrink = false;
        let mut rest = args;
        loop {
            let quoted = rest.starts_with('"');
            let (word, tail) = split_word(rest)?;
            if !quoted
                && word
                    .get(..3)
                    .is_some_and(|part| part.eq_ignore_ascii_case("/L:"))
            {
                if regions || !word[3..].split(';').all(valid_region) {
                    return Err("invalid LOADHIGH memory region list".into());
                }
                regions = true;
            } else if !quoted && word.eq_ignore_ascii_case("/S") {
                if shrink {
                    return Err(USAGE.into());
                }
                shrink = true;
            } else {
                if word.is_empty() || shrink && !regions {
                    return Err(USAGE.into());
                }
                // A system without an upper-memory provider runs the program in conventional RAM.
                return self.launch_program(word, tail, io);
            }
            rest = tail;
        }
    }
}

fn valid_region(value: &str) -> bool {
    let decimal = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    let (region, size) = value
        .split_once(',')
        .map_or((value, None), |(region, size)| (region, Some(size)));
    decimal(region)
        && region.parse::<u16>().is_ok()
        && size.is_none_or(|size| decimal(size) && size.parse::<u32>().is_ok())
}
