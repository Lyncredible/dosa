use std::collections::VecDeque;
use std::error::Error;

use dosa_dos::ShellPipe;

use super::{Command, Shell, redirection, split_word};

pub(super) struct Pipeline {
    stages: VecDeque<String>,
    input: Option<ShellPipe>,
    pub(super) batch_depth: usize,
    batch: bool,
}

pub(super) fn parse(line: &str) -> Result<Option<VecDeque<String>>, &'static str> {
    let mut quoted = false;
    let mut chaining = false;
    let mut stages = VecDeque::new();
    let mut start = 0;
    for (end, character) in line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            '&' if !quoted => chaining = true,
            '|' if !quoted => {
                push_stage(&mut stages, &line[start..end])?;
                start = end + 1;
            }
            _ => {}
        }
    }
    if stages.is_empty() {
        return Ok(None);
    }
    if chaining {
        return Err("command chaining is unsupported");
    }
    if quoted {
        return Err("unterminated quoted pipe command");
    }
    push_stage(&mut stages, &line[start..])?;
    Ok(Some(stages))
}

fn push_stage(stages: &mut VecDeque<String>, stage: &str) -> Result<(), &'static str> {
    let stage = stage.trim();
    if stage.is_empty() {
        return Err("a pipe requires a command on each side");
    }
    stages.push_back(stage.to_owned());
    Ok(())
}

impl Shell {
    pub(super) fn start_pipeline(
        &mut self,
        stages: VecDeque<String>,
        batch: bool,
    ) -> Result<Command, Box<dyn Error>> {
        self.pipelines.push(Pipeline {
            stages,
            input: None,
            batch_depth: self.batch_depth(),
            batch,
        });
        self.advance_pipeline()?
            .ok_or_else(|| "a pipe requires a command on each side".into())
    }

    pub(super) fn advance_pipeline(&mut self) -> Result<Option<Command>, Box<dyn Error>> {
        let Some(pipeline) = self.pipelines.last_mut() else {
            return Ok(None);
        };
        let Some(stage) = pipeline.stages.pop_front() else {
            self.pipelines.pop();
            return Ok(None);
        };
        let input = pipeline.input.take();
        let output = (!pipeline.stages.is_empty()).then(ShellPipe::default);
        let batch = pipeline.batch;
        let mut io = self.machine.prepare_shell_io(
            self.drive,
            &self.directories,
            self.batch_io(),
            None,
            None,
        )?;
        if let Some(input) = &input {
            io.attach_pipe(input, false);
        }
        if let Some(output) = &output {
            io.attach_pipe(output, true);
        }
        self.pipelines.last_mut().unwrap().input = output;

        let (word, _) = split_word(&stage)?;
        if matches!(
            word.to_ascii_uppercase().as_str(),
            "BOOT" | "MOUNT" | "IMGMOUNT"
        ) {
            return Err("this command cannot be used in a DOS pipe".into());
        }
        let mut io = Some(io);
        let command = if batch {
            self.execute_batch_command(&stage, false, &mut io)?
        } else if let Some(request) = redirection::parse(&stage)? {
            self.execute_redirected(request, false, io.take())?
        } else {
            self.execute_command(&stage, &mut io)?
        };
        match io {
            Some(io) => self.apply_redirection(io, command).map(Some),
            None => Ok(Some(command)),
        }
    }
}
