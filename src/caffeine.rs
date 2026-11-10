//! The Caffeine add-on: the drinks you log, how much caffeine is still in
//! you (it halves every `half_life_h` hours), what will be left at bedtime,
//! and the last call: the latest time your usual drink can go in and still
//! leave no more than your limit at bedtime. Drinks of the last two days are
//! kept in `%APPDATA%\Synapse\caffeine.json`; nothing leaves the PC.

use crate::config::CaffeineConfig;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Drink {
    pub name: String,
    pub mg: f32,
    /// Local time as minutes since 2000-01-01 (simple, sortable, no time zones).
    pub at: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Log {
    pub drinks: Vec<Drink>,
}

/// Minutes in two days: older drinks no longer matter.
const KEEP_MIN: i64 = 2 * 24 * 60;

fn file() -> std::path::PathBuf {
    crate::config::dir().join("caffeine.json")
}

impl Log {
    pub fn load() -> Self {
        std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::config::dir());
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(file(), t);
        }
    }

    pub fn add(&mut self, name: &str, mg: f32, now: i64) {
        self.drinks.retain(|d| now - d.at < KEEP_MIN);
        self.drinks.push(Drink { name: name.to_string(), mg, at: now });
    }

    pub fn undo(&mut self) -> Option<Drink> {
        self.drinks.pop()
    }

    /// Caffeine in the body at `t` (mg), from every drink before it.
    pub fn in_body(&self, t: i64, half_life_h: f32) -> f32 {
        // `+ 0.0`: an empty sum is −0.0, which would read "-0 mg".
        self.drinks.iter().filter(|d| d.at <= t).map(|d| d.mg * decay(t - d.at, half_life_h)).sum::<f32>() + 0.0
    }

    /// Caffeine drunk since `from` (today's total).
    pub fn total_since(&self, from: i64) -> f32 {
        self.drinks.iter().filter(|d| d.at >= from).map(|d| d.mg).sum::<f32>() + 0.0
    }
}

/// How much of a dose is left after `minutes`.
pub fn decay(minutes: i64, half_life_h: f32) -> f32 {
    0.5f32.powf(minutes.max(0) as f32 / (half_life_h * 60.0))
}

/// Minutes since 2000-01-01 for a local date and time.
pub fn minutes_at(year: u16, month: u16, day: u16, hour: u16, minute: u16) -> i64 {
    // Days from civil (Howard Hinnant's algorithm), from 2000-03-01 based epoch.
    let (y, m) = if month <= 2 { (year as i64 - 1, month as i64 + 9) } else { (year as i64, month as i64 - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 730_425; // 2000-01-01
    days * 1440 + hour as i64 * 60 + minute as i64
}

pub fn now_min() -> i64 {
    let t = crate::sys::local_time();
    minutes_at(t.year, t.month, t.day, t.hour, t.minute)
}

/// The day's start: 06:00 (a night owl's 01:00 coffee still counts as yesterday).
pub fn day_start(now: i64) -> i64 {
    let six = now.div_euclid(1440) * 1440 + 6 * 60;
    if now >= six {
        six
    } else {
        six - 1440
    }
}

/// Tonight's bedtime (after the day's start).
pub fn bedtime(now: i64, c: &CaffeineConfig) -> i64 {
    day_start(now) - 6 * 60 + c.bedtime_min as i64
}

/// The latest time today a `usual_mg` drink keeps bedtime at or under the
/// limit. None when even nothing more already goes over.
pub fn last_call(log: &Log, now: i64, usual_mg: f32, c: &CaffeineConfig) -> Option<i64> {
    let bed = bedtime(now, c);
    let left = log.in_body(bed, c.half_life_h);
    let room = c.bedtime_limit_mg - left;
    if room <= 0.0 {
        return None;
    }
    if usual_mg <= room {
        return Some(bed);
    }
    // usual · ½^((bed − t)/hl) ≤ room  ⇒  t ≤ bed − hl · log2(usual / room)
    let minutes = (c.half_life_h * 60.0) * (usual_mg / room).log2();
    Some(bed - minutes.ceil() as i64)
}

/// "15:40" for minutes since 2000-01-01.
pub fn fmt_hm(t: i64) -> String {
    let m = t.rem_euclid(1440);
    format!("{:02}:{:02}", m / 60, m % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CaffeineConfig {
        CaffeineConfig::default()
    }

    #[test]
    fn halves_every_half_life() {
        assert!((decay(300, 5.0) - 0.5).abs() < 1e-6);
        assert!((decay(600, 5.0) - 0.25).abs() < 1e-6);
        let mut l = Log::default();
        l.add("Coffee", 100.0, 0);
        assert!((l.in_body(300, 5.0) - 50.0).abs() < 1e-3);
        assert_eq!(l.in_body(-1, 5.0), 0.0);
        // Nothing drunk reads as 0, not -0.
        assert_eq!(format!("{:.0}", Log::default().in_body(0, 5.0)), "0");
    }

    #[test]
    fn dates() {
        assert_eq!(minutes_at(2000, 1, 1, 0, 0), 0);
        assert_eq!(minutes_at(2000, 1, 2, 1, 30), 1440 + 90);
        assert_eq!(minutes_at(2001, 1, 1, 0, 0), 366 * 1440);
        let t = minutes_at(2026, 11, 10, 2, 0);
        assert_eq!(day_start(t), minutes_at(2026, 11, 9, 6, 0));
        assert_eq!(fmt_hm(minutes_at(2026, 11, 10, 15, 40)), "15:40");
    }

    #[test]
    fn last_call_keeps_bedtime_under_the_limit() {
        let c = cfg();
        let morning = minutes_at(2026, 11, 10, 9, 0);
        let mut l = Log::default();
        l.add("Coffee", 95.0, morning);
        let t = last_call(&l, morning, 95.0, &c).unwrap();
        let mut after = l.clone();
        after.add("Coffee", 95.0, t);
        let bed = bedtime(morning, &c);
        assert!(after.in_body(bed, c.half_life_h) <= c.bedtime_limit_mg + 0.5);
        // A minute earlier is fine too; it's a cut-off, not a target.
        assert!(t < bed && t > morning);
        // Way too much already: no last call.
        l.add("Energy", 900.0, morning + 600);
        assert_eq!(last_call(&l, morning, 95.0, &c), None);
    }
}
