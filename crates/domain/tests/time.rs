//! The UTC calendar routine against known instants.

use mintswarm_domain::time::{civil_from_days, format_utc_seconds};

/// Seconds in a day.
const DAY: u64 = 86_400;

#[test]
fn civil_from_days_gives_known_dates() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    assert_eq!(civil_from_days(59), (1970, 3, 1));
    // 2024-02-29 is day 19_782.
    assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    assert_eq!(civil_from_days(19_783), (2024, 3, 1));
}

#[test]
fn format_utc_seconds_gives_known_instants() {
    assert_eq!(format_utc_seconds(0), "1970-01-01T00:00:00Z");
    assert_eq!(format_utc_seconds(1_700_000_000), "2023-11-14T22:13:20Z");
    assert_eq!(format_utc_seconds(1_709_210_096), "2024-02-29T12:34:56Z");
    // 2100 is not a leap year: 2100-02-28 is followed by 2100-03-01.
    assert_eq!(
        format_utc_seconds(4_107_542_400 - 1),
        "2100-02-28T23:59:59Z"
    );
    assert_eq!(format_utc_seconds(4_107_542_400), "2100-03-01T00:00:00Z");
    // 2400 is a leap year.
    assert_eq!(format_utc_seconds(13_574_563_200), "2400-02-29T00:00:00Z");
    assert_eq!(
        format_utc_seconds(13_574_563_200 + DAY),
        "2400-03-01T00:00:00Z"
    );
}
