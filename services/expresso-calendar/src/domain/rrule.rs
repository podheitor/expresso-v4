//! Minimal RRULE expander for free/busy (RFC 5545 subset).
//!
//! Supported: FREQ=DAILY|WEEKLY|MONTHLY|YEARLY, INTERVAL, COUNT, UNTIL,
//! BYDAY (WEEKLY only, list of weekdays). No BYMONTHDAY/BYMONTH/BYSETPOS/
//! EXDATE handling. Unsupported tokens → single-instance fallback (master only).
//!
//! Guards: hard cap of 1000 iterations to prevent pathological rules from
//! running unbounded. Designed for free/busy lookup in a bounded window.

use time::{Date, Duration, Month, OffsetDateTime, Weekday};

const MAX_ITER: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone)]
pub struct Rrule {
    pub freq:     Freq,
    pub interval: u32,
    pub count:    Option<u32>,
    pub until:    Option<OffsetDateTime>,
    pub byday:    Vec<Weekday>,
}

impl Rrule {
    /// Parse RRULE value (without "RRULE:" prefix). Returns None on unsupported
    /// syntax so caller can fall back to single-instance expansion.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut freq: Option<Freq> = None;
        let mut interval: u32     = 1;
        let mut count:    Option<u32>               = None;
        let mut until:    Option<OffsetDateTime>    = None;
        let mut byday:    Vec<Weekday>              = Vec::new();

        for part in raw.split(';') {
            let (k, v) = part.split_once('=')?;
            match k.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match v.trim().to_ascii_uppercase().as_str() {
                        "DAILY"   => Freq::Daily,
                        "WEEKLY"  => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY"  => Freq::Yearly,
                        _ => return None,
                    });
                }
                "INTERVAL" => interval = v.parse().ok()?,
                "COUNT"    => count    = Some(v.parse().ok()?),
                "UNTIL"    => until    = parse_until(v),
                "BYDAY"    => {
                    for d in v.split(',') {
                        byday.push(weekday_from(d.trim())?);
                    }
                }
                // Silent ignore for fields we don't support yet — caller still
                // gets approximate expansion on the recognised ones.
                _ => {}
            }
        }

        Some(Rrule {
            freq: freq?,
            interval: if interval == 0 { 1 } else { interval },
            count,
            until,
            byday,
        })
    }

    /// Expand occurrences within [win_from, win_to]. `dtstart` is the master
    /// instance start; `duration` = master dtend - dtstart (>= 0).
    pub fn expand(
        &self,
        dtstart: OffsetDateTime,
        duration: Duration,
        win_from: OffsetDateTime,
        win_to:   OffsetDateTime,
    ) -> Vec<(OffsetDateTime, OffsetDateTime)> {
        let mut out: Vec<(OffsetDateTime, OffsetDateTime)> = Vec::new();
        let limit_count = self.count.unwrap_or(u32::MAX);
        let hard_until  = self.until;

        let mut emitted: u32 = 0;
        let mut current = dtstart;
        let mut iters: u32 = 0;

        while iters < MAX_ITER && emitted < limit_count {
            iters += 1;
            if let Some(u) = hard_until {
                if current > u { break; }
            }
            if current >= win_to { break; }

            // WEEKLY + BYDAY: emit multiple days per iteration (one per weekday).
            let candidates: Vec<OffsetDateTime> = if self.freq == Freq::Weekly && !self.byday.is_empty() {
                let week_start = start_of_week(current.date());
                self.byday.iter()
                    .map(|wd| {
                        let d = week_start + Duration::days(weekday_index(*wd) as i64);
                        current.replace_date(d)
                    })
                    .filter(|c| *c >= dtstart)
                    .collect()
            } else {
                vec![current]
            };

            for c in candidates {
                if emitted >= limit_count { break; }
                if let Some(u) = hard_until { if c > u { continue; } }
                let end = c + duration;
                // Intersect window.
                if end > win_from && c < win_to {
                    out.push((c, end));
                }
                if c >= dtstart { emitted += 1; }
            }

            current = match advance(current, self.freq, self.interval) {
                Some(n) => n,
                None    => break,
            };
        }
        out
    }
}

/// Single-instance fallback clamped to [win_from, win_to]. Used when there's
/// no rrule or parse fails.
pub fn single_instance(
    dtstart:  OffsetDateTime,
    dtend:    Option<OffsetDateTime>,
    win_from: OffsetDateTime,
    win_to:   OffsetDateTime,
) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let end = dtend.unwrap_or(dtstart);
    if end <= win_from || dtstart >= win_to { return None; }
    let s = if dtstart < win_from { win_from } else { dtstart };
    let e = if end    > win_to   { win_to   } else { end };
    if e <= s { return None; }
    Some((s, e))
}

fn parse_until(raw: &str) -> Option<OffsetDateTime> {
    // Accept "YYYYMMDDTHHMMSSZ" (common) or "YYYYMMDD".
    let s = raw.trim();
    if s.len() == 16 && s.ends_with('Z') {
        let fmt = time::format_description::parse(
            "[year][month][day]T[hour][minute][second]Z",
        ).ok()?;
        let dt = time::PrimitiveDateTime::parse(s, &fmt).ok()?;
        return Some(dt.assume_utc());
    }
    if s.len() == 8 {
        let fmt = time::format_description::parse("[year][month][day]").ok()?;
        let d = Date::parse(s, &fmt).ok()?;
        return Some(d.with_hms(23, 59, 59).ok()?.assume_utc());
    }
    None
}

fn weekday_from(code: &str) -> Option<Weekday> {
    Some(match code.to_ascii_uppercase().as_str() {
        "MO" => Weekday::Monday,
        "TU" => Weekday::Tuesday,
        "WE" => Weekday::Wednesday,
        "TH" => Weekday::Thursday,
        "FR" => Weekday::Friday,
        "SA" => Weekday::Saturday,
        "SU" => Weekday::Sunday,
        _ => return None,
    })
}

fn weekday_index(w: Weekday) -> u8 {
    match w {
        Weekday::Monday    => 0,
        Weekday::Tuesday   => 1,
        Weekday::Wednesday => 2,
        Weekday::Thursday  => 3,
        Weekday::Friday    => 4,
        Weekday::Saturday  => 5,
        Weekday::Sunday    => 6,
    }
}

fn start_of_week(d: Date) -> Date {
    let back = weekday_index(d.weekday()) as i64;
    d - Duration::days(back)
}

fn advance(current: OffsetDateTime, freq: Freq, interval: u32) -> Option<OffsetDateTime> {
    let i = interval as i64;
    match freq {
        Freq::Daily   => Some(current + Duration::days(i)),
        Freq::Weekly  => Some(current + Duration::weeks(i)),
        Freq::Monthly => add_months(current, i),
        Freq::Yearly  => add_months(current, i * 12),
    }
}

fn add_months(dt: OffsetDateTime, months: i64) -> Option<OffsetDateTime> {
    let d = dt.date();
    let y = d.year() as i64;
    let m = d.month() as i64;
    let total = (y * 12 + (m - 1)) + months;
    let ny = (total.div_euclid(12)) as i32;
    let nm_idx = total.rem_euclid(12) as u8 + 1;
    let nm: Month = Month::try_from(nm_idx).ok()?;
    // Clamp day to month length.
    let last = days_in_month(ny, nm);
    let day  = d.day().min(last);
    let nd   = Date::from_calendar_date(ny, nm, day).ok()?;
    Some(dt.replace_date(nd))
}

fn days_in_month(year: i32, m: Month) -> u8 {
    // time::util::days_in_year_month exists.
    m.length(year)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn parses_weekly_byday() {
        let r = Rrule::parse("FREQ=WEEKLY;INTERVAL=1;BYDAY=MO,WE;COUNT=10").unwrap();
        assert_eq!(r.freq, Freq::Weekly);
        assert_eq!(r.count, Some(10));
        assert_eq!(r.byday.len(), 2);
    }

    #[test]
    fn unsupported_freq_rejected() {
        assert!(Rrule::parse("FREQ=SECONDLY").is_none());
    }

    #[test]
    fn daily_count() {
        // Mon 2026-05-11 10:00 UTC, 1h, 3 occurrences.
        let start = datetime!(2026-05-11 10:00 UTC);
        let r = Rrule::parse("FREQ=DAILY;COUNT=3").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::days(10));
        assert_eq!(occ.len(), 3);
        assert_eq!(occ[0].0, start);
        assert_eq!(occ[2].0, start + Duration::days(2));
    }

    #[test]
    fn weekly_byday_expands() {
        // Mon 2026-05-11, BYDAY=MO,WE → expect Mon 11, Wed 13, Mon 18, Wed 20 within 2w.
        let start = datetime!(2026-05-11 09:00 UTC);
        let r = Rrule::parse("FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::weeks(3));
        let days: Vec<_> = occ.iter().map(|(s,_)| s.date().to_string()).collect();
        assert_eq!(days, vec!["2026-05-11","2026-05-13","2026-05-18","2026-05-20"]);
    }

    #[test]
    fn monthly_advances_with_clamp() {
        // Start Jan 31 → Feb 28 (2026 non-leap) clamp.
        let start = datetime!(2026-01-31 12:00 UTC);
        let r = Rrule::parse("FREQ=MONTHLY;COUNT=2").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::days(60));
        assert_eq!(occ.len(), 2);
        assert_eq!(occ[1].0.date().to_string(), "2026-02-28");
    }

    #[test]
    fn until_caps_expansion() {
        let start = datetime!(2026-05-01 00:00 UTC);
        let r = Rrule::parse("FREQ=DAILY;UNTIL=20260503T000000Z").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::days(30));
        assert_eq!(occ.len(), 3); // May 1,2,3
    }

    #[test]
    fn window_intersection() {
        let start = datetime!(2026-05-01 00:00 UTC);
        let r = Rrule::parse("FREQ=DAILY;COUNT=10").unwrap();
        let win_from = start + Duration::days(3);
        let win_to   = start + Duration::days(6);
        let occ = r.expand(start, Duration::hours(1), win_from, win_to);
        // Days 3,4,5 → 3 occurrences inside window.
        assert_eq!(occ.len(), 3);
    }

    #[test]
    fn single_fallback_clamps() {
        let s = datetime!(2026-05-10 00:00 UTC);
        let e = datetime!(2026-05-10 02:00 UTC);
        let w1 = datetime!(2026-05-10 01:00 UTC);
        let w2 = datetime!(2026-05-10 03:00 UTC);
        let r = single_instance(s, Some(e), w1, w2).unwrap();
        assert_eq!(r.0, w1);
        assert_eq!(r.1, e);
    }
}
