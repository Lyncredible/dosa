use iced_x86::{CpuidFeature, Instruction};

/// Selects the supported instruction generation and real-mode FLAGS behavior.
/// Every profile shares Dosa's partial interpreter and device model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CpuProfile {
    I8086,
    I286,
    #[default]
    I386,
}

impl std::fmt::Display for CpuProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::I8086 => "8086",
            Self::I286 => "286",
            Self::I386 => "386",
        })
    }
}

impl CpuProfile {
    pub(super) fn allows(self, instruction: &Instruction, bytes: &[u8]) -> bool {
        // The decoder can discard redundant prefixes, so check the encoding
        // itself before relying on its instruction-generation metadata.
        if self != Self::I386
            && bytes
                .iter()
                .copied()
                .take_while(|&byte| {
                    matches!(
                        byte,
                        0x26 | 0x2e | 0x36 | 0x3e | 0x64..=0x67 | 0xf0 | 0xf2 | 0xf3
                    )
                })
                .any(|byte| matches!(byte, 0x64..=0x67))
        {
            return false;
        }

        instruction.cpuid_features().iter().all(|&feature| {
            use CpuidFeature::*;
            match feature {
                INTEL8086 => true,
                INTEL8086_ONLY => self == Self::I8086,
                INTEL186 | INTEL286 => self != Self::I8086,
                INTEL286_ONLY => self == Self::I286,
                INTEL386 | INTEL386_ONLY => self == Self::I386,
                _ => false,
            }
        })
    }

    pub(super) fn normalize_flags(self, flags: u16) -> u16 {
        // FLAGS bits 12–15 distinguish these generations in real mode.
        let status = (flags & 0x0fd5) | 2;
        status
            | match self {
                Self::I8086 => 0xf000,
                Self::I286 => 0,
                Self::I386 => flags & 0x7000,
            }
    }
}
