//! Helpers for the `過去から` section.
//!
//! This module is intentionally side-effect free — it only formats the
//! elapsed-time label shown next to the resurfaced entry. The actual SQL
//! selection (7+ days old, not due-today, not recently resurfaced) lives
//! in `store::pick_resurface`.

use chrono::{DateTime, Duration, Utc};

/// Human-readable elapsed label in Japanese, e.g. `3日前`, `2週間前`,
/// `4ヶ月前`, `1年前`. Falls back to `たった今` for very recent timestamps
/// and for clock skew (a future `created_at`).
pub fn format_elapsed(created_at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let delta = now.signed_duration_since(created_at);
    if delta < Duration::zero() {
        return "たった今".to_string();
    }

    let days = delta.num_days();
    if days >= 365 {
        let years = days / 365;
        return format!("{}年前", years);
    }
    if days >= 30 {
        let months = days / 30;
        return format!("{}ヶ月前", months);
    }
    if days >= 7 {
        let weeks = days / 7;
        return format!("{}週間前", weeks);
    }
    if days >= 1 {
        return format!("{}日前", days);
    }
    let hours = delta.num_hours();
    if hours >= 1 {
        return format!("{}時間前", hours);
    }
    let minutes = delta.num_minutes();
    if minutes >= 1 {
        return format!("{}分前", minutes);
    }
    "たった今".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, 0, 0, 0).unwrap()
    }

    #[test]
    fn months_and_years() {
        let now = t(2026, 5, 11);
        assert_eq!(format_elapsed(t(2026, 1, 11), now), "4ヶ月前");
        assert_eq!(format_elapsed(t(2025, 5, 11), now), "1年前");
    }

    #[test]
    fn weeks_and_days() {
        let now = t(2026, 5, 11);
        assert_eq!(format_elapsed(t(2026, 4, 27), now), "2週間前");
        assert_eq!(format_elapsed(t(2026, 5, 8), now), "3日前");
    }

    #[test]
    fn freshly_made() {
        let now = t(2026, 5, 11);
        assert_eq!(format_elapsed(now, now), "たった今");
    }
}
