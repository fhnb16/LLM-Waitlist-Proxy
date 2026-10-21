//! Расписание: окна времени, в которые приём или выдача заданий на паузе либо число
//! одновременных заданий модели ограничено.

use chrono::{DateTime, Datelike, Local, Timelike};
use serde::Serialize;

use crate::config::{ScheduleAction, ScheduleRule, parse_hhmm};

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

#[derive(Clone, Debug, Default, Serialize)]
pub struct ActiveRule {
    pub name: String,
    pub action: String,
    pub models: Vec<String>,
    pub slots: u32,
    /// Сколько секунд до конца окна.
    pub ends_in: u64,
}

/// Действующие сейчас правила.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ScheduleNow {
    pub active: Vec<ActiveRule>,
}

impl ScheduleNow {
    fn applies(r: &ActiveRule, model: &str) -> bool {
        r.models.is_empty() || r.models.iter().any(|m| m == model)
    }

    /// Приём на паузе: секунды до конца окна (для `Retry-After`).
    pub fn intake_paused(&self, model: &str) -> Option<u64> {
        self.active.iter().filter(|r| r.action == "pause_intake" && Self::applies(r, model)).map(|r| r.ends_in).max()
    }

    pub fn dispatch_paused(&self, model: &str) -> bool {
        self.active.iter().any(|r| r.action == "pause_dispatch" && Self::applies(r, model))
    }

    pub fn slot_cap(&self, model: &str) -> Option<u32> {
        self.active.iter().filter(|r| r.action == "limit_slots" && Self::applies(r, model)).map(|r| r.slots.max(1)).min()
    }
}

fn action_name(a: ScheduleAction) -> &'static str {
    match a {
        ScheduleAction::PauseIntake => "pause_intake",
        ScheduleAction::PauseDispatch => "pause_dispatch",
        ScheduleAction::LimitSlots => "limit_slots",
    }
}

/// Попадает ли момент `now` в окно правила; если да — секунды до конца окна.
pub fn in_window<Tz: chrono::TimeZone>(r: &ScheduleRule, now: &DateTime<Tz>) -> Option<u64> {
    if !r.enabled {
        return None;
    }
    let from = parse_hhmm(&r.from)?;
    let to = parse_hhmm(&r.to)?;
    let minute = now.hour() * 60 + now.minute();
    let wd = now.weekday().num_days_from_monday() as usize;
    let day_ok = |d: usize| r.days.is_empty() || r.days.iter().any(|x| x.eq_ignore_ascii_case(DAYS[d]));
    let secs_into = now.second() as i64;
    let left = |end_min: i64| -> u64 { ((end_min - minute as i64) * 60 - secs_into).max(1) as u64 };
    if from == to {
        // Окно на весь день.
        return day_ok(wd).then(|| left(1440));
    }
    if from < to {
        (day_ok(wd) && minute >= from && minute < to).then(|| left(to as i64))
    } else {
        // Через полночь: вечер дня `d` или утро следующего дня.
        if minute >= from && day_ok(wd) {
            Some(left(1440 + to as i64))
        } else if minute < to && day_ok((wd + 6) % 7) {
            Some(left(to as i64))
        } else {
            None
        }
    }
}

pub fn evaluate(rules: &[ScheduleRule]) -> ScheduleNow {
    evaluate_at(rules, &Local::now())
}

pub fn evaluate_at<Tz: chrono::TimeZone>(rules: &[ScheduleRule], now: &DateTime<Tz>) -> ScheduleNow {
    let active = rules
        .iter()
        .filter_map(|r| {
            in_window(r, now).map(|ends_in| ActiveRule {
                name: if r.name.is_empty() { format!("{}–{}", r.from, r.to) } else { r.name.clone() },
                action: action_name(r.action).to_string(),
                models: r.models.clone(),
                slots: r.slots,
                ends_in,
            })
        })
        .collect();
    ScheduleNow { active }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(d: u32, h: u32, m: u32) -> DateTime<chrono::Utc> {
        // 2026-10-19 — понедельник.
        chrono::Utc.with_ymd_and_hms(2026, 10, 19 + d, h, m, 0).unwrap()
    }

    fn rule(from: &str, to: &str, days: &[&str], action: ScheduleAction) -> ScheduleRule {
        ScheduleRule { from: from.into(), to: to.into(), days: days.iter().map(|s| s.to_string()).collect(), action, ..Default::default() }
    }

    #[test]
    fn simple_window() {
        let r = rule("01:00", "07:00", &[], ScheduleAction::PauseIntake);
        assert!(in_window(&r, &at(0, 3, 0)).is_some());
        assert_eq!(in_window(&r, &at(0, 6, 59)), Some(60));
        assert!(in_window(&r, &at(0, 7, 0)).is_none());
    }

    #[test]
    fn overnight_and_days() {
        let r = rule("23:00", "02:00", &["mon"], ScheduleAction::PauseDispatch);
        assert!(in_window(&r, &at(0, 23, 30)).is_some()); // пн вечер
        assert!(in_window(&r, &at(1, 1, 0)).is_some()); // вт утро — продолжение пн
        assert!(in_window(&r, &at(1, 23, 30)).is_none()); // вт вечер — не пн
    }

    #[test]
    fn effects() {
        let mut r = rule("00:00", "00:00", &[], ScheduleAction::LimitSlots);
        r.slots = 1;
        r.models = vec!["big".into()];
        let p = rule("00:00", "00:00", &[], ScheduleAction::PauseIntake);
        let now = evaluate_at(&[r, p], &at(0, 12, 0));
        assert_eq!(now.slot_cap("big"), Some(1));
        assert_eq!(now.slot_cap("small"), None);
        assert!(now.intake_paused("small").is_some());
        assert!(!now.dispatch_paused("big"));
    }
}
