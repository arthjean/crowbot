//! Schedules and their next occurrence (US-030): the one place where time
//! zones and changes of the clock are handled.
//!
//! A `Cron` schedule is read on the wall clock of its zone, through `jiff`
//! (MIT or Unlicense; zone data from the system's `zoneinfo` on Unix, and
//! bundled with the program on Windows). A wall-clock time that a change of
//! the clock skips is due at the first instant after the gap, and one that it
//! repeats is due once, at the first of the two. An `Every` schedule adds its
//! period in UTC, so that a change of offset does not move it.

use crowbot_protocol::{ROUTINE_EVERY_MAX_SECONDS, ROUTINE_EVERY_MIN_SECONDS, Schedule};
use jiff::Timestamp;
use jiff::civil::{Date, DateTime};
use jiff::tz::{AmbiguousOffset, TimeZone};

/// How many days a search for the next occurrence of a cron expression
/// looks ahead: nine years cover the 29th of February of any leap year,
/// around a century that skips one.
const SEARCH_DAYS: u32 = 366 * 9;

const FIELDS: [Field; 5] = [
    Field {
        name: "minute",
        low: 0,
        high: 59,
        names: &[],
    },
    Field {
        name: "hour",
        low: 0,
        high: 23,
        names: &[],
    },
    Field {
        name: "day of the month",
        low: 1,
        high: 31,
        names: &[],
    },
    Field {
        name: "month",
        low: 1,
        high: 12,
        names: &[
            "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
        ],
    },
    Field {
        name: "day of the week",
        low: 0,
        high: 7,
        names: &["sun", "mon", "tue", "wed", "thu", "fri", "sat"],
    },
];

struct Field {
    name: &'static str,
    low: u32,
    high: u32,
    /// The names of the values from `low`, when the field has some.
    names: &'static [&'static str],
}

/// A parsed cron expression: the values each field allows, as bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u32,
    days: u32,
    months: u16,
    weekdays: u8,
    /// The day of the month, or the day of the week, was written `*`: the
    /// other field alone then decides the day, and with neither every day
    /// is one. Two restricted fields allow a day either one allows, as cron
    /// does.
    any_day: bool,
    any_weekday: bool,
}

/// Parses five fields. The error names the first field that is wrong.
pub fn parse_cron(expression: &str) -> Result<Cron, String> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.len() != FIELDS.len() {
        return Err(format!(
            "A cron expression has five fields (minute, hour, day of the month, month, day of the week), not {}.",
            fields.len()
        ));
    }
    let mut bits = [0u64; 5];
    for (index, (text, field)) in fields.iter().zip(FIELDS.iter()).enumerate() {
        bits[index] = parse_field(text, field)
            .map_err(|message| format!("The {} field `{text}` {message}", field.name))?;
    }
    // Sunday is 0 or 7.
    let weekdays = bits[4] | (bits[4] >> 7);
    let cron = Cron {
        minutes: bits[0],
        hours: u32::try_from(bits[1]).unwrap_or(u32::MAX),
        days: u32::try_from(bits[2]).unwrap_or(u32::MAX),
        months: u16::try_from(bits[3]).unwrap_or(u16::MAX),
        weekdays: u8::try_from(weekdays & 0x7f).unwrap_or(u8::MAX),
        any_day: fields[2].starts_with('*'),
        any_weekday: fields[4].starts_with('*'),
    };
    if cron.any_weekday && !cron.any_day && !cron.some_month_has_a_day() {
        return Err(format!(
            "The day of the month field `{}` names no day that its months have, so the routine would never run.",
            fields[2]
        ));
    }
    Ok(cron)
}

fn parse_field(text: &str, field: &Field) -> Result<u64, String> {
    let mut bits = 0u64;
    for item in text.split(',') {
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .map_err(|_| format!("has a step `{step}` that is not a number."))?;
                if step == 0 {
                    return Err("has a step of 0.".to_owned());
                }
                (range, step)
            }
            None => (item, 1),
        };
        let (low, high) = if range == "*" {
            (field.low, field.high)
        } else if let Some((low, high)) = range.split_once('-') {
            (value(low, field)?, value(high, field)?)
        } else {
            let low = value(range, field)?;
            // `5/15` runs from 5 to the end of the field.
            (low, if item.contains('/') { field.high } else { low })
        };
        if low > high {
            return Err(format!("has a range from {low} down to {high}."));
        }
        let mut at = low;
        while at <= high {
            bits |= 1 << at;
            at += step;
        }
    }
    Ok(bits)
}

fn value(text: &str, field: &Field) -> Result<u32, String> {
    let lower = text.to_ascii_lowercase();
    if let Some(index) = field.names.iter().position(|name| *name == lower) {
        return Ok(field.low + u32::try_from(index).unwrap_or(0));
    }
    let value: u32 = text.parse().map_err(|_| {
        format!(
            "holds `{text}`, which is not a value from {} to {}.",
            field.low, field.high
        )
    })?;
    if value < field.low || value > field.high {
        return Err(format!(
            "holds {value}, out of the range from {} to {}.",
            field.low, field.high
        ));
    }
    Ok(value)
}

impl Cron {
    fn some_month_has_a_day(&self) -> bool {
        // The longest each month can be: February has a 29th in leap years.
        const LONGEST: [u32; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        (1..=12u32).any(|month| {
            self.months & (1 << month) != 0
                && (1..=LONGEST[month as usize - 1]).any(|day| self.days & (1 << day) != 0)
        })
    }

    fn allows_day(&self, date: Date) -> bool {
        if self.months & (1 << date.month()) == 0 {
            return false;
        }
        let day = self.days & (1 << date.day()) != 0;
        let weekday = self.weekdays & (1 << date.weekday().to_sunday_zero_offset()) != 0;
        match (self.any_day, self.any_weekday) {
            (true, true) => true,
            (true, false) => weekday,
            (false, true) => day,
            (false, false) => day || weekday,
        }
    }

    /// The first occurrence strictly after `after`, on the wall clock of
    /// `zone`.
    fn next_after(&self, zone: &TimeZone, after: Timestamp) -> Option<Timestamp> {
        let start = after.to_zoned(zone.clone()).datetime();
        let mut date = start.date();
        for _ in 0..SEARCH_DAYS {
            if self.allows_day(date) {
                for hour in (0..24).filter(|hour| self.hours & (1 << hour) != 0) {
                    for minute in (0..60).filter(|minute| self.minutes & (1u64 << minute) != 0) {
                        let at = date.at(hour, minute, 0, 0);
                        // A time the clock repeats shows a later wall clock
                        // than an instant of its second pass: it is due at
                        // the first, which comes before `after`, so it is
                        // left behind.
                        if at <= start {
                            continue;
                        }
                        if let Some(instant) = resolve(zone, at)
                            && instant > after
                        {
                            return Some(instant);
                        }
                    }
                }
            }
            date = date.tomorrow().ok()?;
        }
        None
    }
}

/// The instant a wall-clock time of `zone` stands for: the first instant
/// after a gap for a time the clock skips, and the first of the two for a
/// time it repeats.
fn resolve(zone: &TimeZone, at: DateTime) -> Option<Timestamp> {
    match zone.to_ambiguous_timestamp(at).offset() {
        AmbiguousOffset::Unambiguous { offset } => offset.to_timestamp(at).ok(),
        AmbiguousOffset::Gap { after, .. } => {
            // Read with the offset after the gap, the time lands before the
            // change of the clock; the change is the first instant after.
            let before_change = after.to_timestamp(at).ok()?;
            zone.following(before_change)
                .next()
                .map(|change| change.timestamp())
        }
        AmbiguousOffset::Fold { before, .. } => before.to_timestamp(at).ok(),
    }
}

/// A schedule the daemon can compute with.
#[derive(Debug, Clone)]
pub enum Checked {
    Once(i64),
    Every(i64),
    Cron(Cron, TimeZone),
}

/// Checks a schedule: an `Every` period within its bounds, five valid cron
/// fields and a zone the system knows. The error says what is wrong.
pub fn check(schedule: &Schedule) -> Result<Checked, String> {
    match schedule {
        Schedule::Once { at_ms } => Timestamp::from_millisecond(*at_ms)
            .map(|_| Checked::Once(*at_ms))
            .map_err(|_| "The instant of the routine is out of range.".to_owned()),
        Schedule::Every { seconds } => {
            if !(ROUTINE_EVERY_MIN_SECONDS..=ROUTINE_EVERY_MAX_SECONDS).contains(seconds) {
                return Err(format!(
                    "A routine repeats every {ROUTINE_EVERY_MIN_SECONDS} seconds at least and every {} days at most.",
                    ROUTINE_EVERY_MAX_SECONDS / 86_400
                ));
            }
            let millis = i64::try_from(*seconds).unwrap_or(i64::MAX) * 1000;
            Ok(Checked::Every(millis))
        }
        Schedule::Cron { expression, zone } => {
            let cron = parse_cron(expression)?;
            let zone = TimeZone::get(zone).map_err(|_| {
                format!("The time zone `{zone}` is not one this computer knows, such as Europe/Paris or UTC.")
            })?;
            Ok(Checked::Cron(cron, zone))
        }
    }
}

impl Checked {
    /// The next occurrence after `last`. For `Every`, `last` is the last
    /// occurrence and the next one is the period after it, in UTC; for the
    /// others, the first occurrence strictly after `last`. `None` once a
    /// `Once` schedule has passed.
    pub fn next_after(&self, last: i64) -> Option<i64> {
        match self {
            Self::Once(at) => (*at > last).then_some(*at),
            Self::Every(period) => last.checked_add(*period),
            Self::Cron(cron, zone) => cron
                .next_after(zone, Timestamp::from_millisecond(last).ok()?)
                .map(Timestamp::as_millisecond),
        }
    }

    /// The time between the occurrence `due` and the next one, for a
    /// schedule that repeats.
    pub fn period(&self, due: i64) -> Option<i64> {
        match self {
            Self::Once(_) => None,
            Self::Every(period) => Some(*period),
            Self::Cron(..) => self.next_after(due).map(|next| next - due),
        }
    }

    pub fn repeats(&self) -> bool {
        !matches!(self, Self::Once(_))
    }

    /// The first occurrence of a new routine, from `now`.
    pub fn first(&self, now: i64) -> Option<i64> {
        self.next_after(now)
    }

    /// The occurrences after `after`, `count` at most.
    pub fn upcoming(&self, after: i64, count: usize) -> Vec<i64> {
        let mut found = Vec::with_capacity(count);
        let mut last = after;
        while found.len() < count {
            let Some(next) = self.next_after(last) else {
                break;
            };
            found.push(next);
            last = next;
        }
        found
    }

    /// The zone an occurrence is shown in: the routine's own for `Cron`,
    /// the computer's for the others.
    pub fn zone(&self) -> TimeZone {
        match self {
            Self::Cron(_, zone) => zone.clone(),
            _ => TimeZone::system(),
        }
    }

    /// An instant as a person reads it, on the clock of the schedule's zone.
    pub fn show(&self, at: i64) -> String {
        show_in(&self.zone(), at)
    }
}

/// An instant as a person reads it on the clock of `zone`.
pub fn show_in(zone: &TimeZone, at: i64) -> String {
    match Timestamp::from_millisecond(at) {
        Ok(instant) => instant
            .to_zoned(zone.clone())
            .strftime("%Y-%m-%d %H:%M %Z (%:Q)")
            .to_string(),
        Err(_) => format!("{at} ms"),
    }
}

/// The name of the computer's time zone, when it has one.
pub fn system_zone() -> Option<String> {
    TimeZone::system().iana_name().map(str::to_owned)
}

/// Reads an instant a person or a bot wrote: RFC 3339 with an offset
/// (`2026-11-25T09:00:00+01:00`), or a wall-clock time
/// (`2026-11-25T09:00`) on the clock of `zone`, the computer's by default.
pub fn parse_instant(text: &str, zone: Option<&str>) -> Result<i64, String> {
    if let Ok(instant) = text.parse::<Timestamp>() {
        return Ok(instant.as_millisecond());
    }
    let civil: DateTime = text.parse().map_err(|_| {
        format!("`{text}` is not an instant such as 2026-11-25T09:00 or 2026-11-25T09:00:00+01:00.")
    })?;
    let zone = match zone {
        Some(name) => TimeZone::get(name)
            .map_err(|_| format!("The time zone `{name}` is not one this computer knows."))?,
        None => TimeZone::system(),
    };
    resolve(&zone, civil)
        .map(Timestamp::as_millisecond)
        .ok_or_else(|| format!("`{text}` is out of range."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    fn cron(expression: &str, zone: &str) -> Checked {
        check(&Schedule::Cron {
            expression: expression.to_owned(),
            zone: zone.to_owned(),
        })
        .unwrap()
    }

    fn shown(ms: i64, zone: &str) -> String {
        Timestamp::from_millisecond(ms)
            .unwrap()
            .to_zoned(TimeZone::get(zone).unwrap())
            .strftime("%Y-%m-%dT%H:%M%:z")
            .to_string()
    }

    #[test]
    fn a_schedule_is_once_every_or_cron() {
        assert!(matches!(
            check(&Schedule::Once { at_ms: 5 }),
            Ok(Checked::Once(5))
        ));
        assert!(matches!(
            check(&Schedule::Every { seconds: 3600 }),
            Ok(Checked::Every(3_600_000))
        ));
        assert!(check(&Schedule::Every { seconds: 10 }).is_err());
        assert!(matches!(cron("0 9 * * *", "UTC"), Checked::Cron(..)));
        let zone = check(&Schedule::Cron {
            expression: "0 9 * * *".to_owned(),
            zone: "Mars/Olympus".to_owned(),
        });
        assert!(zone.unwrap_err().contains("Mars/Olympus"));
    }

    #[test]
    fn a_time_the_clock_skips_is_due_at_the_first_instant_after_the_gap() {
        // On 2026-03-29 Paris goes from 02:00 to 03:00.
        let daily = cron("30 2 * * *", "Europe/Paris");
        let next = daily.next_after(at("2026-03-28T12:00:00Z")).unwrap();
        assert_eq!(shown(next, "Europe/Paris"), "2026-03-29T03:00+02:00");
        // The day after, 02:30 exists again.
        let after = daily.next_after(next).unwrap();
        assert_eq!(shown(after, "Europe/Paris"), "2026-03-30T02:30+02:00");
        // In New York on 2026-03-08, 02:00 to 03:00 is skipped.
        let quarter = cron("*/15 * * * *", "America/New_York");
        let next = quarter.next_after(at("2026-03-08T06:50:00Z")).unwrap();
        assert_eq!(shown(next, "America/New_York"), "2026-03-08T03:00-04:00");
        let then = quarter.next_after(next).unwrap();
        assert_eq!(shown(then, "America/New_York"), "2026-03-08T03:15-04:00");
    }

    #[test]
    fn a_time_the_clock_repeats_is_due_once_at_the_first_of_the_two() {
        // On 2026-10-25 Paris goes back from 03:00 to 02:00: 02:30 happens
        // twice, at +02:00 then at +01:00.
        let daily = cron("30 2 * * *", "Europe/Paris");
        let first = daily.next_after(at("2026-10-24T12:00:00Z")).unwrap();
        assert_eq!(shown(first, "Europe/Paris"), "2026-10-25T02:30+02:00");
        let next = daily.next_after(first).unwrap();
        assert_eq!(shown(next, "Europe/Paris"), "2026-10-26T02:30+01:00");
        // From inside the second pass, the repeated times are left behind.
        let quarter = cron("*/15 * * * *", "Europe/Paris");
        let second_pass = at("2026-10-25T01:20:00Z"); // 02:20 +01:00
        let next = quarter.next_after(second_pass).unwrap();
        assert_eq!(shown(next, "Europe/Paris"), "2026-10-25T03:00+01:00");
    }

    #[test]
    fn an_every_schedule_adds_its_period_in_utc() {
        let every = check(&Schedule::Every { seconds: 86_400 }).unwrap();
        // Across the change of the clock in Paris, a day is 24 hours.
        let last = at("2026-03-28T09:00:00Z");
        assert_eq!(every.next_after(last), Some(last + 86_400_000));
        assert_eq!(every.period(last), Some(86_400_000));
    }

    #[test]
    fn a_field_out_of_range_is_refused_and_named() {
        for (expression, field) in [
            ("60 * * * *", "minute"),
            ("* 24 * * *", "hour"),
            ("* * 0 * *", "day of the month"),
            ("* * * 13 *", "month"),
            ("* * * * 8", "day of the week"),
            ("*/0 * * * *", "minute"),
            ("* 5-2 * * *", "hour"),
            ("* * * xyz *", "month"),
        ] {
            let error = parse_cron(expression).unwrap_err();
            assert!(
                error.contains(&format!("The {field} field")),
                "{expression}: {error}"
            );
        }
        assert!(parse_cron("* * * *").unwrap_err().contains("five fields"));
        let never = parse_cron("0 0 30 2 *").unwrap_err();
        assert!(never.contains("never run"), "{never}");
    }

    #[test]
    fn names_lists_ranges_and_steps_are_read_as_cron_does() {
        let weekdays = cron("0 9 * * mon-fri", "UTC");
        // 2026-11-28 is a Saturday: the next is Monday the 30th.
        let next = weekdays.next_after(at("2026-11-27T10:00:00Z")).unwrap();
        assert_eq!(shown(next, "UTC"), "2026-11-30T09:00+00:00");
        // Sunday is 0 or 7.
        assert_eq!(
            parse_cron("0 0 * * 7").unwrap(),
            parse_cron("0 0 * * 0").unwrap()
        );
        // A day of the month and a day of the week both restricted: either.
        let either = cron("0 0 1 * fri", "UTC");
        let next = either.next_after(at("2026-11-24T00:00:00Z")).unwrap();
        assert_eq!(shown(next, "UTC"), "2026-11-27T00:00+00:00");
        let after = either.next_after(next).unwrap();
        assert_eq!(shown(after, "UTC"), "2026-12-01T00:00+00:00");
        let stepped = cron("5/20 * * * *", "UTC");
        let three = stepped.upcoming(at("2026-11-24T00:00:00Z"), 3);
        let minutes: Vec<String> = three.iter().map(|ms| shown(*ms, "UTC")).collect();
        assert_eq!(
            minutes,
            [
                "2026-11-24T00:05+00:00",
                "2026-11-24T00:25+00:00",
                "2026-11-24T00:45+00:00"
            ]
        );
        // The 29th of February is found in the next leap year.
        let leap = cron("0 0 29 2 *", "UTC");
        let next = leap.next_after(at("2026-11-24T00:00:00Z")).unwrap();
        assert_eq!(shown(next, "UTC"), "2028-02-29T00:00+00:00");
    }

    #[test]
    fn a_once_schedule_is_due_until_its_instant_passes() {
        let once = check(&Schedule::Once { at_ms: 1_000 }).unwrap();
        assert_eq!(once.next_after(999), Some(1_000));
        assert_eq!(once.next_after(1_000), None);
        assert!(!once.repeats());
    }

    #[test]
    fn an_instant_is_read_with_an_offset_or_on_a_zone_s_clock() {
        assert_eq!(
            parse_instant("2026-11-25T09:00:00+01:00", None),
            Ok(at("2026-11-25T08:00:00Z"))
        );
        assert_eq!(
            parse_instant("2026-11-25T09:00", Some("Europe/Paris")),
            Ok(at("2026-11-25T08:00:00Z"))
        );
        assert!(parse_instant("tomorrow", None).is_err());
    }

    /// A small generator, so that the property runs the same cases each time.
    struct Cases(u64);

    impl Cases {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, bound: u64) -> u64 {
            self.next() % bound
        }

        fn field(&mut self, low: u64, high: u64) -> String {
            match self.below(5) {
                0 => "*".to_owned(),
                1 => format!("*/{}", 1 + self.below(high - low + 1)),
                2 => {
                    let a = low + self.below(high - low + 1);
                    let b = a + self.below(high - a + 1);
                    format!("{a}-{b}")
                }
                3 => {
                    let a = low + self.below(high - low + 1);
                    let b = low + self.below(high - low + 1);
                    format!("{a},{b}")
                }
                _ => (low + self.below(high - low + 1)).to_string(),
            }
        }
    }

    #[test]
    fn the_next_occurrence_of_any_valid_schedule_is_later_than_the_instant() {
        let zones = [
            "UTC",
            "Europe/Paris",
            "America/New_York",
            "Australia/Lord_Howe",
            "Asia/Kolkata",
            "Pacific/Apia",
        ];
        let mut cases = Cases(0x9e37_79b9_7f4a_7c15);
        let mut checked = 0;
        for _ in 0..2_000 {
            let instant = i64::try_from(cases.below(4_102_444_800_000)).unwrap(); // to 2100
            let schedule = match cases.below(3) {
                0 => Schedule::Once {
                    at_ms: i64::try_from(cases.below(4_102_444_800_000)).unwrap(),
                },
                1 => Schedule::Every {
                    seconds: ROUTINE_EVERY_MIN_SECONDS
                        + cases.below(ROUTINE_EVERY_MAX_SECONDS - ROUTINE_EVERY_MIN_SECONDS),
                },
                _ => Schedule::Cron {
                    expression: format!(
                        "{} {} {} {} {}",
                        cases.field(0, 59),
                        cases.field(0, 23),
                        cases.field(1, 28),
                        cases.field(1, 12),
                        cases.field(0, 6)
                    ),
                    zone: zones[usize::try_from(cases.below(6)).unwrap()].to_owned(),
                },
            };
            let Ok(valid) = check(&schedule) else {
                continue;
            };
            if let Some(next) = valid.next_after(instant) {
                assert!(next > instant, "{schedule:?} after {instant}: {next}");
                checked += 1;
            } else {
                assert!(matches!(valid, Checked::Once(_)), "{schedule:?}");
            }
        }
        assert!(checked > 1_000, "{checked}");
    }
}
