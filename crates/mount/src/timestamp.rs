use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A DOS-compatible modification date and time, with two-second precision.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DosTimestamp {
    date: u16,
    time: u16,
}

impl DosTimestamp {
    /// The fallback used by DOS file services when a mount has no timestamp.
    pub const EPOCH: Self = Self {
        date: 0x0021,
        time: 0,
    };

    pub(crate) fn from_fields(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Option<Self> {
        if !(1980..=2107).contains(&year)
            || !(1..=12).contains(&month)
            || !(1..=month_days(year, month)).contains(&day)
            || hour >= 24
            || minute >= 60
            || second >= 60
        {
            return None;
        }
        Some(Self {
            date: ((year - 1980) << 9) | (u16::from(month) << 5) | u16::from(day),
            time: (u16::from(hour) << 11) | (u16::from(minute) << 5) | u16::from(second / 2),
        })
    }

    pub(crate) fn from_host(value: SystemTime) -> Option<Self> {
        let seconds = value.duration_since(UNIX_EPOCH).ok()?.as_secs();
        let mut days = seconds / 86_400;
        let mut year = 1970;
        while year <= 2107 {
            let length = if leap(year) { 366 } else { 365 };
            if days < length {
                break;
            }
            days -= length;
            year += 1;
        }
        if !(1980..=2107).contains(&year) {
            return None;
        }
        let mut month = 1;
        while days >= u64::from(month_days(year, month)) {
            days -= u64::from(month_days(year, month));
            month += 1;
        }
        let time = seconds % 86_400;
        Self::from_fields(
            year,
            month,
            days as u8 + 1,
            (time / 3600) as u8,
            (time / 60 % 60) as u8,
            (time % 60) as u8,
        )
    }

    /// Year, month, and day in the recorded calendar.
    pub fn date(self) -> (u16, u8, u8) {
        (
            1980 + (self.date >> 9),
            (self.date >> 5 & 15) as u8,
            (self.date & 31) as u8,
        )
    }

    /// Hour, minute, and second in the recorded calendar.
    pub fn time(self) -> (u8, u8, u8) {
        (
            (self.time >> 11) as u8,
            (self.time >> 5 & 63) as u8,
            ((self.time & 31) * 2) as u8,
        )
    }

    /// Convert the DOS calendar to the UTC representation used for host file times.
    pub fn to_system_time(self) -> SystemTime {
        let (year, month, day) = self.date();
        let (hour, minute, second) = self.time();
        let days = (1970..year)
            .map(|year| if leap(year) { 366 } else { 365 })
            .sum::<u64>()
            + (1..month)
                .map(|month| u64::from(month_days(year, month)))
                .sum::<u64>()
            + u64::from(day - 1);
        UNIX_EPOCH
            + Duration::from_secs(
                days * 86_400 + u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second),
            )
    }

    /// The packed DOS date and time words.
    pub fn packed(self) -> (u16, u16) {
        (self.date, self.time)
    }
}

fn month_days(year: u16, month: u8) -> u8 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn leap(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}
