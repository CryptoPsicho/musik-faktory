//! Weekly / monthly spending windows (local time of the server).

use crate::store::{Db, Ledger};
use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use musik_shared::*;

fn local_midnight(d: NaiveDate) -> DateTime<Local> {
    let naive = d.and_hms_opt(0, 0, 0).unwrap();
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(t) => t,
        LocalResult::Ambiguous(a, _) => a,
        // Midnight skipped by a DST jump: take the first valid instant after it.
        LocalResult::None => Local.from_local_datetime(&(naive + Duration::hours(1))).earliest().unwrap(),
    }
}

/// [start, end) of the current week.
pub fn week_bounds(now: DateTime<Local>, starts_sunday: bool) -> (DateTime<Utc>, DateTime<Utc>) {
    let today = now.date_naive();
    let back = if starts_sunday {
        today.weekday().num_days_from_sunday()
    } else {
        today.weekday().num_days_from_monday()
    };
    let start = today - Duration::days(back as i64);
    let end = start + Duration::days(7);
    (local_midnight(start).with_timezone(&Utc), local_midnight(end).with_timezone(&Utc))
}

/// [start, end) of the current calendar month.
pub fn month_bounds(now: DateTime<Local>) -> (DateTime<Utc>, DateTime<Utc>) {
    let today = now.date_naive();
    let start = NaiveDate::from_ymd_opt(today.year(), today.month(), 1).unwrap();
    let end = if today.month() == 12 {
        NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(today.year(), today.month() + 1, 1).unwrap()
    };
    (local_midnight(start).with_timezone(&Utc), local_midnight(end).with_timezone(&Utc))
}

fn spent_between(ledger: &[Ledger], start: DateTime<Utc>, end: DateTime<Utc>) -> f64 {
    // `+ 0.0` turns the -0.0 of an empty float sum into a plain 0.
    ledger.iter().filter(|e| !e.pending && e.at >= start && e.at < end).map(|e| e.cost).sum::<f64>() + 0.0
}

/// Money tied up in generations that are still running (their provisional
/// ledger entries). Counted against every window until they are final.
pub fn reserved(db: &Db) -> f64 {
    db.ledger.iter().filter(|e| e.pending).map(|e| e.cost).sum::<f64>() + 0.0
}

fn period(limit: Option<f64>, ledger: &[Ledger], bounds: (DateTime<Utc>, DateTime<Utc>), reserved: f64) -> Option<PeriodStatus> {
    let limit = limit?;
    let spent = spent_between(ledger, bounds.0, bounds.1);
    Some(PeriodStatus {
        limit,
        spent,
        reserved,
        remaining: (limit - spent - reserved).max(0.0),
        resets_at: bounds.1.to_rfc3339(),
    })
}

pub fn status_at(db: &Db, now: DateTime<Local>) -> BudgetStatus {
    let s = &db.settings;
    let reserved = reserved(db);
    let week = period(s.weekly_limit, &db.ledger, week_bounds(now, s.week_starts_sunday), reserved);
    let month = period(s.monthly_limit, &db.ledger, month_bounds(now), reserved);
    let remaining = [week.as_ref(), month.as_ref()]
        .into_iter()
        .flatten()
        .map(|p| p.remaining)
        .reduce(f64::min);
    let remaining_seconds = remaining.map(|r| {
        if s.price_per_minute <= 0.0 {
            u32::MAX
        } else {
            (r / s.price_per_minute * 60.0).floor().min(u32::MAX as f64) as u32
        }
    });
    BudgetStatus { week, month, remaining, remaining_seconds }
}

pub fn status(db: &Db) -> BudgetStatus {
    status_at(db, Local::now())
}

/// Can a generation costing `cost` start now? Err = kid-readable reason.
pub fn check(db: &Db, cost: f64) -> Result<(), (String, &'static str)> {
    let b = status(db);
    let cur = &db.settings.currency;
    let checks = [(b.week.as_ref(), "this week", "budget_week"), (b.month.as_ref(), "this month", "budget_month")];
    for (p, when, code) in checks {
        if let Some(p) = p {
            if cost > p.remaining + 1e-9 {
                let reset = DateTime::parse_from_rfc3339(&p.resets_at)
                    .map(|t| t.with_timezone(&Local).format("%A %-d %B").to_string())
                    .unwrap_or_default();
                return Err((
                    format!(
                        "Not enough budget: this song would cost about {}, but only {} is left {when}. The budget refills on {reset}. Try a shorter song, or ask a parent.",
                        format_money(cost, cur),
                        format_money(p.remaining, cur),
                    ),
                    code,
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Ledger;

    fn entry(at: DateTime<Utc>, cost: f64) -> Ledger {
        Ledger {
            id: "x".into(),
            at,
            kind: LedgerKind::Generation,
            track_id: None,
            version_id: None,
            track_title: String::new(),
            version_label: String::new(),
            seconds: 0.0,
            estimated_cost: cost,
            measured_credits: None,
            cost,
            note: String::new(),
            pending: false,
        }
    }

    #[test]
    fn week_starts_on_monday_by_default() {
        // Wednesday 2026-11-11 12:00 local
        let now = Local.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap();
        let (s, e) = week_bounds(now, false);
        let s = s.with_timezone(&Local);
        assert_eq!(s.date_naive(), NaiveDate::from_ymd_opt(2026, 11, 9).unwrap());
        assert_eq!((e - s.with_timezone(&Utc)).num_days(), 7);
        let (s2, _) = week_bounds(now, true);
        assert_eq!(s2.with_timezone(&Local).date_naive(), NaiveDate::from_ymd_opt(2026, 11, 8).unwrap());
    }

    #[test]
    fn month_wraps_year() {
        let now = Local.with_ymd_and_hms(2026, 12, 31, 23, 0, 0).unwrap();
        let (s, e) = month_bounds(now);
        assert_eq!(s.with_timezone(&Local).date_naive(), NaiveDate::from_ymd_opt(2026, 12, 1).unwrap());
        assert_eq!(e.with_timezone(&Local).date_naive(), NaiveDate::from_ymd_opt(2027, 1, 1).unwrap());
    }

    #[test]
    fn limits_use_the_tightest_window() {
        let now = Local.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap();
        let mut db = Db::default();
        db.settings.weekly_limit = Some(1.0);
        db.settings.monthly_limit = Some(2.0);
        // Spent 0.9 earlier this month (last week) and 0.3 this week.
        db.ledger.push(entry(Local.with_ymd_and_hms(2026, 11, 3, 10, 0, 0).unwrap().with_timezone(&Utc), 0.9));
        db.ledger.push(entry(Local.with_ymd_and_hms(2026, 11, 10, 10, 0, 0).unwrap().with_timezone(&Utc), 0.3));
        // Last month: ignored.
        db.ledger.push(entry(Local.with_ymd_and_hms(2026, 10, 30, 10, 0, 0).unwrap().with_timezone(&Utc), 5.0));
        let b = status_at(&db, now);
        assert!((b.week.as_ref().unwrap().remaining - 0.7).abs() < 1e-9);
        assert!((b.month.as_ref().unwrap().remaining - 0.8).abs() < 1e-9);
        assert!((b.remaining.unwrap() - 0.7).abs() < 1e-9);
        // 0.7 at 0.15/min = 280 s
        assert_eq!(b.remaining_seconds, Some(280));
    }

    #[test]
    fn no_limits_means_unlimited() {
        let mut db = Db::default();
        db.settings.weekly_limit = None;
        db.settings.monthly_limit = None;
        let b = status(&db);
        assert!(b.remaining.is_none());
        assert!(check(&db, 1000.0).is_ok());
    }
}
