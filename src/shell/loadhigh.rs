use std::error::Error;

use dosa_dos::{ShellIo, UpperMemoryRequest};

use super::{Command, Shell, split_word};

pub(super) const HELP: &str = "LOADHIGH [/L:region[,size][;region...]] [/S] program [arguments]\r\n\
    LH is an alias. Region 1 is upper memory and region 0 is conventional memory.\r\n\
    Programs use conventional memory when upper memory is unavailable.\r\n";
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
        let mut regions = None;
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
                if regions.is_some() {
                    return Err("invalid LOADHIGH memory region list".into());
                }
                regions = Some(
                    word[3..]
                        .split(';')
                        .map(parse_region)
                        .collect::<Option<Vec<_>>>()
                        .ok_or("invalid LOADHIGH memory region list")?,
                );
            } else if !quoted && word.eq_ignore_ascii_case("/S") {
                if shrink {
                    return Err(USAGE.into());
                }
                shrink = true;
            } else {
                if word.is_empty() || shrink && regions.is_none() {
                    return Err(USAGE.into());
                }
                let upper = match regions.as_deref() {
                    None => Some(UpperMemoryRequest {
                        allow_conventional_allocations: true,
                        ..UpperMemoryRequest::default()
                    }),
                    Some([(1, minimum), rest @ ..]) => Some(UpperMemoryRequest {
                        minimum_bytes: minimum.unwrap_or(0),
                        shrink: shrink && minimum.is_some(),
                        allow_conventional_allocations: rest.iter().any(|&(region, _)| region == 0),
                    }),
                    Some(_) => None,
                };
                return self.launch_program_with_memory(word, tail, io, upper);
            }
            rest = tail;
        }
    }
}

fn parse_region(value: &str) -> Option<(u16, Option<u32>)> {
    let decimal = |text: &str| text.bytes().all(|byte| byte.is_ascii_digit());
    let (region, minimum) = match value.split_once(',') {
        Some((region, size)) if decimal(size) => (region, Some(size.parse().ok()?)),
        Some(_) => return None,
        None => (value, None),
    };
    if !decimal(region) {
        return None;
    }
    Some((region.parse().ok()?, minimum))
}
