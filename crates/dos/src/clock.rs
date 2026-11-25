use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Snapshot of the UTC clock used by the DOS date and time services.
#[derive(Clone, Copy)]
pub struct DosClock(Duration);

impl DosClock {
    pub fn now() -> Self {
        Self(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default(),
        )
    }

    /// Year, month, day, and weekday, with Sunday numbered zero.
    pub fn date(self) -> (u16, u8, u8, u8) {
        let days = self.0.as_secs() / 86_400;
        let (year, month, day) = civil_date(days);
        (year, month, day, ((days + 4) % 7) as u8)
    }

    /// Hours, minutes, seconds, and hundredths of a second.
    pub fn time(self) -> (u8, u8, u8, u8) {
        let seconds = self.0.as_secs() % 86_400;
        (
            (seconds / 3600) as u8,
            (seconds / 60 % 60) as u8,
            (seconds % 60) as u8,
            (self.0.subsec_millis() / 10) as u8,
        )
    }
}

fn civil_date(mut days: u64) -> (u16, u8, u8) {
    let leap = |year| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let mut year = 1970 + (days / 146_097 * 400) as u16;
    days %= 146_097;
    loop {
        let count = if leap(year) { 366 } else { 365 };
        if days < count {
            break;
        }
        days -= count;
        year += 1;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for (month, count) in months.into_iter().enumerate() {
        if days < count {
            return (year, month as u8 + 1, days as u8 + 1);
        }
        days -= count;
    }
    unreachable!()
}
