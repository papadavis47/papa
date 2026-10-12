//! How note dates read in the UI, in the local time zone.

use jiff::Timestamp;
use jiff::Zoned;
use jiff::tz::TimeZone;

/// For the note list, relative to `now`: the time for today ("14:05"), the
/// day for this year ("Mar 3"), the full date before that ("Mar 3 2025").
pub fn short(at: Timestamp, now: &Zoned) -> String {
    let at = at.to_zoned(now.time_zone().clone());
    if at.date() == now.date() {
        at.strftime("%H:%M").to_string()
    } else if at.year() == now.year() {
        at.strftime("%b %-d").to_string()
    } else {
        at.strftime("%b %-d %Y").to_string()
    }
}

/// For the editor: "Mar 3 2026".
pub fn long(at: Timestamp, tz: &TimeZone) -> String {
    at.to_zoned(tz.clone()).strftime("%b %-d %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Zoned {
        "2026-10-12T15:30:00+02:00[Europe/Paris]".parse().unwrap()
    }

    fn at(time: &str) -> Timestamp {
        time.parse().unwrap()
    }

    #[test]
    fn today_shows_the_local_time() {
        // 08:05 UTC is 10:05 in Paris.
        assert_eq!(short(at("2026-10-12T08:05:00Z"), &now()), "10:05");
    }

    #[test]
    fn days_are_local_too() {
        // 23:30 UTC on the 11th is already 01:30 on the 12th in Paris
        // (summer time, UTC+2).
        assert_eq!(short(at("2026-10-11T23:30:00Z"), &now()), "01:30");
        assert_eq!(short(at("2026-10-11T21:30:00Z"), &now()), "Oct 11");
    }

    #[test]
    fn earlier_years_show_the_year() {
        assert_eq!(short(at("2025-03-03T12:00:00Z"), &now()), "Mar 3 2025");
        assert_eq!(
            long(at("2026-03-03T12:00:00Z"), &TimeZone::UTC),
            "Mar 3 2026"
        );
    }
}
