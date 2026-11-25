use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SECONDS_PER_DAY: u64 = 86_400;

/// A DOS clock that follows the host until it is set by the guest or shell.
#[derive(Clone, Copy, Default)]
pub(crate) struct SessionClock(Option<(DosClock, Instant)>);

impl SessionClock {
    pub(crate) fn now(self) -> DosClock {
        match self.0 {
            Some((base, anchor)) => DosClock(base.0 + anchor.elapsed()),
            None => DosClock::now(),
        }
    }

    pub(crate) fn adjusted_file_time(self) -> Option<SystemTime> {
        self.0.map(|_| UNIX_EPOCH + self.now().0)
    }

    pub(crate) fn set_date(&mut self, year: u16, month: u8, day: u8) -> bool {
        if !(1980..=2099).contains(&year)
            || !(1..=12).contains(&month)
            || !(1..=month_days(year, month)).contains(&day)
        {
            return false;
        }
        let days = (1970..year).map(year_days).sum::<u64>()
            + (1..month)
                .map(|month| u64::from(month_days(year, month)))
                .sum::<u64>()
            + u64::from(day - 1);
        let now = self.now().0;
        self.set(Duration::new(
            days * SECONDS_PER_DAY + now.as_secs() % SECONDS_PER_DAY,
            now.subsec_nanos(),
        ));
        true
    }

    pub(crate) fn set_time(&mut self, hour: u8, minute: u8, second: u8, hundredths: u8) -> bool {
        if hour >= 24 || minute >= 60 || second >= 60 || hundredths >= 100 {
            return false;
        }
        let days = self.now().0.as_secs() / SECONDS_PER_DAY;
        let seconds = days * SECONDS_PER_DAY
            + u64::from(hour) * 3600
            + u64::from(minute) * 60
            + u64::from(second);
        self.set(Duration::new(seconds, u32::from(hundredths) * 10_000_000));
        true
    }

    fn set(&mut self, value: Duration) {
        self.0 = Some((DosClock(value), Instant::now()));
    }
}

/// Snapshot of the DOS date and time.
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
        let days = self.0.as_secs() / SECONDS_PER_DAY;
        let (year, month, day) = civil_date(days);
        (year, month, day, ((days + 4) % 7) as u8)
    }

    /// Hours, minutes, seconds, and hundredths of a second.
    pub fn time(self) -> (u8, u8, u8, u8) {
        let seconds = self.0.as_secs() % SECONDS_PER_DAY;
        (
            (seconds / 3600) as u8,
            (seconds / 60 % 60) as u8,
            (seconds % 60) as u8,
            (self.0.subsec_millis() / 10) as u8,
        )
    }
}

fn civil_date(mut days: u64) -> (u16, u8, u8) {
    let mut year = 1970 + (days / 146_097 * 400) as u16;
    days %= 146_097;
    while days >= year_days(year) {
        days -= year_days(year);
        year += 1;
    }
    let mut month = 1;
    while days >= u64::from(month_days(year, month)) {
        days -= u64::from(month_days(year, month));
        month += 1;
    }
    (year, month, days as u8 + 1)
}

fn leap(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn year_days(year: u16) -> u64 {
    if leap(year) { 366 } else { 365 }
}

fn month_days(year: u16, month: u8) -> u8 {
    match month {
        2 if leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
