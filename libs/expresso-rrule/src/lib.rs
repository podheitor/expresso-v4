//! RRULE expander for free/busy + occurrence listing (RFC 5545 subset).
//!
//! Supported: FREQ=DAILY|WEEKLY|MONTHLY|YEARLY, INTERVAL, COUNT, UNTIL,
//! BYDAY (plain weekdays for WEEKLY; ordinal `1MO`/`-1FR` for MONTHLY/YEARLY),
//! BYMONTHDAY, BYMONTH, BYSETPOS, and EXDATE exclusions. Unrecognised tokens
//! are ignored; a rule with no FREQ → parse returns None (single-instance
//! fallback at the call site).
//!
//! Expansion model (RFC 5545 §3.3.10): for each period the BYxxx rules build a
//! candidate set, BYSETPOS selects positions within it, EXDATE removes matches,
//! then COUNT/UNTIL cap the stream. Candidates are generated in date order.
//!
//! Guards: hard cap of 1000 periods to prevent pathological rules from running
//! unbounded. Designed for bounded-window lookup.

use time::{Date, Duration, Month, OffsetDateTime, Weekday};

const MAX_ITER: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// A BYDAY entry: weekday plus optional ordinal (`1MO` = first Monday,
/// `-1FR` = last Friday). Ordinal is None for plain weekday codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByDay {
    pub weekday: Weekday,
    pub ordinal: Option<i8>,
}

#[derive(Debug, Clone)]
pub struct Rrule {
    pub freq: Freq,
    pub interval: u32,
    pub count: Option<u32>,
    pub until: Option<OffsetDateTime>,
    pub byday: Vec<ByDay>,
    /// BYMONTHDAY: 1..=31 or negative (-1 = last day of month).
    pub bymonthday: Vec<i8>,
    /// BYMONTH: 1..=12.
    pub bymonth: Vec<u8>,
    /// BYSETPOS: 1-based position(s) into the period's candidate set; negative
    /// counts from the end. Empty = keep all.
    pub bysetpos: Vec<i32>,
    /// EXDATE: explicit datetimes to exclude from the expansion.
    pub exdate: Vec<OffsetDateTime>,
}

impl Rrule {
    /// Parse RRULE value (without "RRULE:" prefix). Returns None on unsupported
    /// syntax so caller can fall back to single-instance expansion.
    pub fn parse(raw: &str) -> Option<Self> {
        let mut freq: Option<Freq> = None;
        let mut interval: u32 = 1;
        let mut count: Option<u32> = None;
        let mut until: Option<OffsetDateTime> = None;
        let mut byday: Vec<ByDay> = Vec::new();
        let mut bymonthday: Vec<i8> = Vec::new();
        let mut bymonth: Vec<u8> = Vec::new();
        let mut bysetpos: Vec<i32> = Vec::new();
        let mut exdate: Vec<OffsetDateTime> = Vec::new();

        let body = raw.strip_prefix("RRULE:").unwrap_or(raw);
        for part in body.split(';') {
            let (k, v) = part.split_once('=')?;
            match k.trim().to_ascii_uppercase().as_str() {
                "FREQ" => {
                    freq = Some(match v.trim().to_ascii_uppercase().as_str() {
                        "DAILY" => Freq::Daily,
                        "WEEKLY" => Freq::Weekly,
                        "MONTHLY" => Freq::Monthly,
                        "YEARLY" => Freq::Yearly,
                        _ => return None,
                    });
                }
                "INTERVAL" => interval = v.parse().ok()?,
                "COUNT" => count = Some(v.parse().ok()?),
                "UNTIL" => until = parse_until(v),
                "BYDAY" => {
                    for d in v.split(',') {
                        byday.push(byday_from(d.trim())?);
                    }
                }
                "BYMONTHDAY" => {
                    for d in v.split(',') {
                        bymonthday.push(parse_monthday(d.trim())?);
                    }
                }
                "BYMONTH" => {
                    for m in v.split(',') {
                        bymonth.push(parse_month(m.trim())?);
                    }
                }
                "BYSETPOS" => {
                    for p in v.split(',') {
                        let n: i32 = p.trim().parse().ok()?;
                        if n != 0 {
                            bysetpos.push(n);
                        }
                    }
                }
                "EXDATE" => {
                    for d in v.split(',') {
                        if let Some(dt) = parse_until(d.trim()) {
                            exdate.push(dt);
                        }
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
            bymonthday,
            bymonth,
            bysetpos,
            exdate,
        })
    }

    /// Expand occurrences within [win_from, win_to]. `dtstart` is the master
    /// instance start; `duration` = master dtend - dtstart (>= 0).
    pub fn expand(
        &self,
        dtstart: OffsetDateTime,
        duration: Duration,
        win_from: OffsetDateTime,
        win_to: OffsetDateTime,
    ) -> Vec<(OffsetDateTime, OffsetDateTime)> {
        let mut out: Vec<(OffsetDateTime, OffsetDateTime)> = Vec::new();
        let limit_count = self.count.unwrap_or(u32::MAX);

        let mut emitted: u32 = 0;
        let mut current = dtstart;
        let mut iters: u32 = 0;

        while iters < MAX_ITER && emitted < limit_count {
            iters += 1;
            if self.until.is_some_and(|u| current > u) || current >= win_to {
                break;
            }

            // Build this period's candidate set, apply BYSETPOS, then stream.
            let mut candidates = self.period_candidates(current, dtstart);
            candidates = apply_bysetpos(&self.bysetpos, candidates);

            for c in candidates {
                if emitted >= limit_count {
                    break;
                }
                // Outside the series range — never counts toward COUNT.
                if c < dtstart || self.until.is_some_and(|u| c > u) {
                    continue;
                }
                // EXDATE-excluded dates ARE part of the series: they consume a
                // COUNT slot (RFC 5545 / dateutil semantics) but emit nothing.
                emitted += 1;
                if self.is_excluded(c) {
                    continue;
                }
                let end = c + duration;
                if end > win_from && c < win_to {
                    out.push((c, end));
                }
            }

            current = match advance(current, self.freq, self.interval) {
                Some(n) => n,
                None => break,
            };
        }
        out
    }

    /// Generate the candidate datetimes for the period containing `current`,
    /// applying BYMONTH/BYMONTHDAY/BYDAY expansion. Returns date-sorted,
    /// deduplicated datetimes (each keeps `current`'s clock time).
    fn period_candidates(
        &self,
        current: OffsetDateTime,
        dtstart: OffsetDateTime,
    ) -> Vec<OffsetDateTime> {
        let dates: Vec<Date> = match self.freq {
            Freq::Weekly if !self.byday.is_empty() => self.weekly_dates(current),
            Freq::Monthly | Freq::Yearly if self.needs_set_expansion() => {
                self.month_or_year_dates(current)
            }
            _ => {
                // DAILY, or any FREQ with no expanding BY* rule: the period's
                // anchor date, filtered by BYMONTH/BYMONTHDAY if present.
                let d = current.date();
                if self.month_allowed(d) && self.monthday_allowed(d) {
                    vec![d]
                } else {
                    vec![]
                }
            }
        };
        let mut out: Vec<OffsetDateTime> = dates
            .into_iter()
            .filter(|d| self.month_allowed(*d))
            .map(|d| current.replace_date(d))
            .filter(|c| *c >= dtstart)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// True when MONTHLY/YEARLY needs day-set expansion (BYDAY/BYMONTHDAY).
    fn needs_set_expansion(&self) -> bool {
        !self.byday.is_empty() || !self.bymonthday.is_empty()
    }

    /// WEEKLY: the BYDAY weekdays mapped into `current`'s week.
    fn weekly_dates(&self, current: OffsetDateTime) -> Vec<Date> {
        let week_start = start_of_week(current.date());
        self.byday
            .iter()
            .map(|bd| week_start + Duration::days(weekday_index(bd.weekday) as i64))
            .collect()
    }

    /// MONTHLY/YEARLY: union of BYMONTHDAY days and BYDAY (ordinal or plain)
    /// resolved within each candidate month of the period.
    fn month_or_year_dates(&self, current: OffsetDateTime) -> Vec<Date> {
        let mut out: Vec<Date> = Vec::new();
        for (y, m) in self.candidate_months(current) {
            for md in &self.bymonthday {
                if let Some(d) = monthday_to_date(y, m, *md) {
                    out.push(d);
                }
            }
            for bd in &self.byday {
                out.extend(byday_dates_in_month(y, m, *bd));
            }
        }
        out
    }

    /// The (year, month) pairs to expand for this period. YEARLY scans the
    /// BYMONTH list (or just the anchor month); MONTHLY uses the anchor month.
    fn candidate_months(&self, current: OffsetDateTime) -> Vec<(i32, Month)> {
        let d = current.date();
        if self.freq == Freq::Yearly && !self.bymonth.is_empty() {
            self.bymonth
                .iter()
                .filter_map(|m| Month::try_from(*m).ok().map(|mo| (d.year(), mo)))
                .collect()
        } else {
            vec![(d.year(), d.month())]
        }
    }

    fn month_allowed(&self, d: Date) -> bool {
        self.bymonth.is_empty() || self.bymonth.contains(&(d.month() as u8))
    }

    fn monthday_allowed(&self, d: Date) -> bool {
        if self.bymonthday.is_empty() {
            return true;
        }
        let last = d.month().length(d.year()) as i8;
        self.bymonthday
            .iter()
            .any(|md| *md == d.day() as i8 || (*md < 0 && last + md + 1 == d.day() as i8))
    }

    fn is_excluded(&self, c: OffsetDateTime) -> bool {
        self.exdate.contains(&c)
    }
}

/// Select BYSETPOS positions from a period's candidate list (1-based; negative
/// counts from the end). Empty selector keeps everything. Positions are
/// resolved against the sorted candidate set and re-sorted for output.
fn apply_bysetpos(bysetpos: &[i32], candidates: Vec<OffsetDateTime>) -> Vec<OffsetDateTime> {
    if bysetpos.is_empty() || candidates.is_empty() {
        return candidates;
    }
    let len = candidates.len() as i32;
    let mut picked: Vec<OffsetDateTime> = bysetpos
        .iter()
        .filter_map(|p| {
            let idx = if *p > 0 { *p - 1 } else { len + *p };
            (0..len).contains(&idx).then(|| candidates[idx as usize])
        })
        .collect();
    picked.sort();
    picked.dedup();
    picked
}

/// All dates in (year, month) matching a BYDAY entry. Plain weekday → every
/// matching day; ordinal (`1MO`, `-1FR`) → the nth (or nth-from-last) match.
fn byday_dates_in_month(year: i32, month: Month, bd: ByDay) -> Vec<Date> {
    let len = month.length(year);
    let matches: Vec<Date> = (1..=len)
        .filter_map(|day| Date::from_calendar_date(year, month, day).ok())
        .filter(|d| d.weekday() == bd.weekday)
        .collect();
    match bd.ordinal {
        None => matches,
        Some(n) if n > 0 => matches.get((n - 1) as usize).copied().into_iter().collect(),
        Some(n) => {
            let from_end = (-n - 1) as usize;
            matches
                .len()
                .checked_sub(from_end + 1)
                .and_then(|i| matches.get(i))
                .copied()
                .into_iter()
                .collect()
        }
    }
}

/// Resolve a BYMONTHDAY value (positive or negative) to a concrete date.
fn monthday_to_date(year: i32, month: Month, md: i8) -> Option<Date> {
    let last = month.length(year) as i8;
    let day = if md > 0 {
        md
    } else if md < 0 {
        last + md + 1
    } else {
        return None;
    };
    if day < 1 || day > last {
        return None;
    }
    Date::from_calendar_date(year, month, day as u8).ok()
}

/// Single-instance fallback clamped to [win_from, win_to]. Used when there's
/// no rrule or parse fails.
pub fn single_instance(
    dtstart: OffsetDateTime,
    dtend: Option<OffsetDateTime>,
    win_from: OffsetDateTime,
    win_to: OffsetDateTime,
) -> Option<(OffsetDateTime, OffsetDateTime)> {
    let end = dtend.unwrap_or(dtstart);
    // Zero-duration instant (no DTEND): present iff [win_from, win_to) contains it.
    if end == dtstart {
        return (dtstart >= win_from && dtstart < win_to).then_some((dtstart, dtstart));
    }
    if end <= win_from || dtstart >= win_to {
        return None;
    }
    let s = if dtstart < win_from {
        win_from
    } else {
        dtstart
    };
    let e = if end > win_to { win_to } else { end };
    if e <= s {
        return None;
    }
    Some((s, e))
}

fn parse_until(raw: &str) -> Option<OffsetDateTime> {
    // Accept "YYYYMMDDTHHMMSSZ" (common) or "YYYYMMDD".
    let s = raw.trim();
    if s.len() == 16 && s.ends_with('Z') {
        let fmt =
            time::format_description::parse("[year][month][day]T[hour][minute][second]Z").ok()?;
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

/// Parse a BYDAY token: optional signed ordinal prefix + 2-letter weekday
/// (`MO`, `2WE`, `-1FR`). Ordinal 0 is invalid (RFC 5545).
fn byday_from(code: &str) -> Option<ByDay> {
    let code = code.to_ascii_uppercase();
    let split = code.len().saturating_sub(2);
    let (ord_str, day_str) = code.split_at(split);
    let weekday = weekday_from(day_str)?;
    let ordinal = if ord_str.is_empty() {
        None
    } else {
        let n: i8 = ord_str.parse().ok()?;
        if n == 0 {
            return None;
        }
        Some(n)
    };
    Some(ByDay { weekday, ordinal })
}

/// Parse a BYMONTHDAY value: 1..=31 or -1..=-31, never 0.
fn parse_monthday(v: &str) -> Option<i8> {
    let n: i8 = v.parse().ok()?;
    ((1..=31).contains(&n) || (-31..=-1).contains(&n)).then_some(n)
}

/// Parse a BYMONTH value: 1..=12.
fn parse_month(v: &str) -> Option<u8> {
    let n: u8 = v.parse().ok()?;
    (1..=12).contains(&n).then_some(n)
}

fn weekday_index(w: Weekday) -> u8 {
    match w {
        Weekday::Monday => 0,
        Weekday::Tuesday => 1,
        Weekday::Wednesday => 2,
        Weekday::Thursday => 3,
        Weekday::Friday => 4,
        Weekday::Saturday => 5,
        Weekday::Sunday => 6,
    }
}

fn start_of_week(d: Date) -> Date {
    let back = weekday_index(d.weekday()) as i64;
    d - Duration::days(back)
}

fn advance(current: OffsetDateTime, freq: Freq, interval: u32) -> Option<OffsetDateTime> {
    let i = interval as i64;
    match freq {
        Freq::Daily => Some(current + Duration::days(i)),
        Freq::Weekly => Some(current + Duration::weeks(i)),
        Freq::Monthly => add_months(current, i),
        Freq::Yearly => add_months(current, i * 12),
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
    let day = d.day().min(last);
    let nd = Date::from_calendar_date(ny, nm, day).ok()?;
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
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        assert_eq!(
            days,
            vec!["2026-05-11", "2026-05-13", "2026-05-18", "2026-05-20"]
        );
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
        let win_to = start + Duration::days(6);
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

    #[test]
    fn single_instance_outside_window_returns_none() {
        let s = datetime!(2026-05-10 00:00 UTC);
        let e = datetime!(2026-05-10 02:00 UTC);
        let w1 = datetime!(2026-05-11 00:00 UTC);
        let w2 = datetime!(2026-05-12 00:00 UTC);
        assert!(single_instance(s, Some(e), w1, w2).is_none());
    }

    #[test]
    fn single_instance_inside_window_returns_some() {
        let s = datetime!(2026-05-10 09:00 UTC);
        let e = datetime!(2026-05-10 10:00 UTC);
        let w1 = datetime!(2026-05-10 00:00 UTC);
        let w2 = datetime!(2026-05-11 00:00 UTC);
        assert!(single_instance(s, Some(e), w1, w2).is_some());
    }

    #[test]
    fn single_instance_before_window_returns_none() {
        use time::macros::datetime;
        let s = datetime!(2026-05-09 09:00 UTC);
        let e = datetime!(2026-05-09 10:00 UTC);
        let w1 = datetime!(2026-05-10 00:00 UTC);
        let w2 = datetime!(2026-05-11 00:00 UTC);
        assert!(single_instance(s, Some(e), w1, w2).is_none());
    }

    #[test]
    fn single_instance_inside_window_no_dtend_returns_some() {
        use time::macros::datetime;
        let s = datetime!(2026-05-10 09:00 UTC);
        let w1 = datetime!(2026-05-10 00:00 UTC);
        let w2 = datetime!(2026-05-11 00:00 UTC);
        assert!(single_instance(s, None, w1, w2).is_some());
    }

    #[test]
    fn single_instance_after_window_no_dtend_returns_none() {
        use time::macros::datetime;
        let s = datetime!(2026-05-12 09:00 UTC);
        let w1 = datetime!(2026-05-10 00:00 UTC);
        let w2 = datetime!(2026-05-11 00:00 UTC);
        assert!(single_instance(s, None, w1, w2).is_none());
    }

    #[test]
    fn single_instance_at_window_start_returns_some() {
        use time::macros::datetime;
        let s = datetime!(2026-05-10 00:00 UTC);
        let w1 = datetime!(2026-05-10 00:00 UTC);
        let w2 = datetime!(2026-05-11 00:00 UTC);
        assert!(single_instance(s, None, w1, w2).is_some());
    }

    #[test]
    fn single_instance_exactly_at_window_end_returns_none() {
        use time::macros::datetime;
        let s = datetime!(2026-05-11 00:00 UTC);
        let w1 = datetime!(2026-05-10 00:00 UTC);
        let w2 = datetime!(2026-05-11 00:00 UTC);
        // start == w2 means the event begins at the exclusive upper bound
        assert!(single_instance(s, None, w1, w2).is_none());
    }

    #[test]
    fn freq_variants_are_distinct() {
        assert_ne!(Freq::Daily, Freq::Weekly);
        assert_ne!(Freq::Monthly, Freq::Yearly);
    }

    #[test]
    fn yearly_freq_parsed() {
        let r = Rrule::parse("FREQ=YEARLY;COUNT=3").unwrap();
        assert_eq!(r.freq, Freq::Yearly);
        assert_eq!(r.count, Some(3));
    }

    #[test]
    fn interval_zero_normalised_to_one() {
        let r = Rrule::parse("FREQ=DAILY;INTERVAL=0").unwrap();
        assert_eq!(r.interval, 1);
    }

    #[test]
    fn freq_weekly_ne_monthly() {
        assert_ne!(Freq::Weekly, Freq::Monthly);
    }

    #[test]
    fn freq_daily_ne_yearly() {
        assert_ne!(Freq::Daily, Freq::Yearly);
    }

    #[test]
    fn freq_monthly_ne_weekly() {
        assert_ne!(Freq::Monthly, Freq::Weekly);
    }

    #[test]
    fn freq_yearly_ne_daily() {
        assert_ne!(Freq::Yearly, Freq::Daily);
    }

    #[test]
    fn freq_daily_ne_weekly() {
        assert_ne!(Freq::Daily, Freq::Weekly);
    }

    // ---- BYMONTHDAY ----

    #[test]
    fn monthly_bymonthday_positive() {
        // 15th of each month, 3 times.
        let start = datetime!(2026-01-15 08:00 UTC);
        let r = Rrule::parse("FREQ=MONTHLY;BYMONTHDAY=15;COUNT=3").unwrap();
        let occ = r.expand(
            start,
            Duration::hours(1),
            start,
            start + Duration::days(120),
        );
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        assert_eq!(days, vec!["2026-01-15", "2026-02-15", "2026-03-15"]);
    }

    #[test]
    fn monthly_bymonthday_negative_is_last_day() {
        // Last day of each month.
        let start = datetime!(2026-01-31 08:00 UTC);
        let r = Rrule::parse("FREQ=MONTHLY;BYMONTHDAY=-1;COUNT=3").unwrap();
        let occ = r.expand(
            start,
            Duration::hours(1),
            start,
            start + Duration::days(120),
        );
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        // Jan 31, Feb 28 (2026 non-leap), Mar 31.
        assert_eq!(days, vec!["2026-01-31", "2026-02-28", "2026-03-31"]);
    }

    // ---- BYDAY ordinals (monthly) ----

    #[test]
    fn monthly_first_monday() {
        let start = datetime!(2026-01-01 09:00 UTC);
        let r = Rrule::parse("FREQ=MONTHLY;BYDAY=1MO;COUNT=3").unwrap();
        let occ = r.expand(
            start,
            Duration::hours(1),
            start,
            start + Duration::days(120),
        );
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        // First Mondays: Jan 5, Feb 2, Mar 2 2026.
        assert_eq!(days, vec!["2026-01-05", "2026-02-02", "2026-03-02"]);
    }

    #[test]
    fn monthly_last_friday() {
        let start = datetime!(2026-01-01 09:00 UTC);
        let r = Rrule::parse("FREQ=MONTHLY;BYDAY=-1FR;COUNT=2").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::days(90));
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        // Last Fridays: Jan 30, Feb 27 2026.
        assert_eq!(days, vec!["2026-01-30", "2026-02-27"]);
    }

    // ---- BYSETPOS ----

    #[test]
    fn monthly_bysetpos_last_weekday() {
        // Last weekday (MO-FR) of the month via BYDAY + BYSETPOS=-1.
        let start = datetime!(2026-01-01 09:00 UTC);
        let r = Rrule::parse("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYSETPOS=-1;COUNT=2").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::days(90));
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        // Last weekday: Jan 30 (Fri), Feb 27 (Fri) 2026.
        assert_eq!(days, vec!["2026-01-30", "2026-02-27"]);
    }

    // ---- BYMONTH (yearly) ----

    #[test]
    fn yearly_bymonth_filters() {
        // Jun & Dec each year on the anchor day.
        let start = datetime!(2026-06-10 09:00 UTC);
        let r = Rrule::parse("FREQ=YEARLY;BYMONTH=6,12;BYMONTHDAY=10;COUNT=3").unwrap();
        let occ = r.expand(
            start,
            Duration::hours(1),
            start,
            start + Duration::days(800),
        );
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        assert_eq!(days, vec!["2026-06-10", "2026-12-10", "2027-06-10"]);
    }

    // ---- EXDATE ----

    #[test]
    fn exdate_removes_instance() {
        let start = datetime!(2026-05-01 10:00 UTC);
        let r = Rrule::parse("FREQ=DAILY;COUNT=4;EXDATE=20260502T100000Z").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::days(30));
        let days: Vec<_> = occ.iter().map(|(s, _)| s.date().to_string()).collect();
        // May 2 excluded; COUNT still counts it (RFC: EXDATE filters the stream).
        assert_eq!(days, vec!["2026-05-01", "2026-05-03", "2026-05-04"]);
    }

    // ---- parse helpers ----

    #[test]
    fn byday_ordinal_parsed() {
        let bd = byday_from("-1FR").unwrap();
        assert_eq!(bd.weekday, Weekday::Friday);
        assert_eq!(bd.ordinal, Some(-1));
        let plain = byday_from("MO").unwrap();
        assert_eq!(plain.ordinal, None);
    }

    #[test]
    fn byday_ordinal_zero_rejected() {
        assert!(byday_from("0MO").is_none());
    }

    #[test]
    fn bymonthday_range_validated() {
        assert_eq!(parse_monthday("31"), Some(31));
        assert_eq!(parse_monthday("-1"), Some(-1));
        assert_eq!(parse_monthday("0"), None);
        assert_eq!(parse_monthday("32"), None);
    }

    #[test]
    fn bymonth_range_validated() {
        assert_eq!(parse_month("12"), Some(12));
        assert_eq!(parse_month("0"), None);
        assert_eq!(parse_month("13"), None);
    }

    #[test]
    fn rrule_prefix_tolerated() {
        assert!(Rrule::parse("RRULE:FREQ=DAILY;COUNT=2").is_some());
    }

    #[test]
    fn bysetpos_picks_positive_and_negative() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let cands = vec![
            base,
            base + Duration::days(1),
            base + Duration::days(2),
            base + Duration::days(3),
        ];
        let picked = apply_bysetpos(&[1, -1], cands.clone());
        assert_eq!(picked, vec![cands[0], cands[3]]);
    }

    #[test]
    fn bysetpos_out_of_range_dropped() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let cands = vec![base, base + Duration::days(1)];
        let picked = apply_bysetpos(&[5], cands);
        assert!(picked.is_empty());
    }

    #[test]
    fn monthday_to_date_negative_resolves() {
        // Feb 2026 (non-leap) last day = 28.
        let d = monthday_to_date(2026, Month::February, -1).unwrap();
        assert_eq!(d.day(), 28);
    }

    #[test]
    fn byday_dates_in_month_plain_lists_all() {
        // All Mondays in Feb 2026: 2, 9, 16, 23.
        let mondays = byday_dates_in_month(
            2026,
            Month::February,
            ByDay {
                weekday: Weekday::Monday,
                ordinal: None,
            },
        );
        let days: Vec<_> = mondays.iter().map(|d| d.day()).collect();
        assert_eq!(days, vec![2, 9, 16, 23]);
    }

    #[test]
    fn weekly_byday_still_works_after_refactor() {
        // Regression: plain WEEKLY+BYDAY unchanged by ordinal support.
        let start = datetime!(2026-05-11 09:00 UTC);
        let r = Rrule::parse("FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4").unwrap();
        let occ = r.expand(start, Duration::hours(1), start, start + Duration::weeks(3));
        assert_eq!(occ.len(), 4);
    }
}
