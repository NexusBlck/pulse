//! The Focus timer: focus sessions with short breaks, and a long break
//! every few sessions (the Pomodoro technique). Times are wall-clock
//! seconds, so a timer keeps counting through sleep.

use crate::config::FocusConfig;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Focus,
    Break,
    LongBreak,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Focus => "Focus",
            Phase::Break => "Break",
            Phase::LongBreak => "Long break",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Run {
    Idle,
    Running { ends_at: f64 },
    Paused { left: f64 },
}

#[derive(Clone, Copy, Debug)]
pub struct Pomodoro {
    pub phase: Phase,
    pub run: Run,
    /// Focus sessions finished since the timer was last reset.
    pub done: u32,
}

impl Default for Pomodoro {
    fn default() -> Self {
        Self { phase: Phase::Focus, run: Run::Idle, done: 0 }
    }
}

pub fn duration(p: Phase, c: &FocusConfig) -> f64 {
    let min = match p {
        Phase::Focus => c.focus_minutes,
        Phase::Break => c.break_minutes,
        Phase::LongBreak => c.long_break_minutes,
    };
    (min as f64 * 60.0).max(1.0)
}

impl Pomodoro {
    /// Running or paused: the closed island shows the countdown.
    pub fn active(&self) -> bool {
        self.run != Run::Idle
    }

    pub fn running(&self) -> bool {
        matches!(self.run, Run::Running { .. })
    }

    pub fn left(&self, now: f64, c: &FocusConfig) -> f64 {
        match self.run {
            Run::Running { ends_at } => (ends_at - now).max(0.0),
            Run::Paused { left } => left,
            Run::Idle => duration(self.phase, c),
        }
    }

    /// How much of the phase has gone, 0..1.
    pub fn progress(&self, now: f64, c: &FocusConfig) -> f32 {
        (1.0 - self.left(now, c) / duration(self.phase, c)).clamp(0.0, 1.0) as f32
    }

    pub fn start(&mut self, now: f64, c: &FocusConfig) {
        self.run = match self.run {
            Run::Idle => Run::Running { ends_at: now + duration(self.phase, c) },
            Run::Paused { left } => Run::Running { ends_at: now + left },
            r => r,
        };
    }

    pub fn pause(&mut self, now: f64) {
        if let Run::Running { ends_at } = self.run {
            self.run = Run::Paused { left: (ends_at - now).max(0.0) };
        }
    }

    pub fn toggle(&mut self, now: f64, c: &FocusConfig) {
        if self.running() {
            self.pause(now);
        } else {
            self.start(now, c);
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// End the phase now, as if its time were up.
    pub fn skip(&mut self, now: f64, c: &FocusConfig) -> Phase {
        self.advance(now, c)
    }

    /// Call each frame: the phase that just ended, if one did.
    pub fn tick(&mut self, now: f64, c: &FocusConfig) -> Option<Phase> {
        match self.run {
            Run::Running { ends_at } if now >= ends_at => Some(self.advance(ends_at, c)),
            _ => None,
        }
    }

    /// When the running phase ends (for the wake-up).
    pub fn ends_at(&self) -> Option<f64> {
        match self.run {
            Run::Running { ends_at } => Some(ends_at),
            _ => None,
        }
    }

    fn advance(&mut self, at: f64, c: &FocusConfig) -> Phase {
        let ended = self.phase;
        let auto = if ended == Phase::Focus {
            self.done += 1;
            let every = c.long_break_every.max(1);
            self.phase = if self.done % every == 0 { Phase::LongBreak } else { Phase::Break };
            c.auto_start_breaks
        } else {
            self.phase = Phase::Focus;
            c.auto_start_focus
        };
        self.run = if auto { Run::Running { ends_at: at + duration(self.phase, c) } } else { Run::Idle };
        ended
    }
}

/// "24:59", or "1:02:03" past an hour.
pub fn fmt_left(secs: f64) -> String {
    let s = secs.ceil().max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> FocusConfig {
        FocusConfig { focus_minutes: 25.0, break_minutes: 5.0, long_break_minutes: 15.0, long_break_every: 2, auto_start_breaks: true, auto_start_focus: false, sound: true }
    }

    #[test]
    fn runs_pauses_and_resumes() {
        let c = cfg();
        let mut p = Pomodoro::default();
        p.start(0.0, &c);
        assert_eq!(p.left(60.0, &c), 1440.0);
        p.pause(60.0);
        assert_eq!(p.left(500.0, &c), 1440.0);
        p.start(500.0, &c);
        assert_eq!(p.ends_at(), Some(1940.0));
    }

    #[test]
    fn focus_leads_to_breaks_and_a_long_one() {
        let c = cfg();
        let mut p = Pomodoro::default();
        p.start(0.0, &c);
        assert_eq!(p.tick(1500.0, &c), Some(Phase::Focus));
        // The break starts by itself, from when focus ended.
        assert_eq!((p.phase, p.ends_at()), (Phase::Break, Some(1800.0)));
        assert_eq!(p.tick(1800.0, &c), Some(Phase::Break));
        // Focus waits for you.
        assert_eq!((p.phase, p.run), (Phase::Focus, Run::Idle));
        p.start(2000.0, &c);
        p.tick(3500.0, &c);
        assert_eq!(p.phase, Phase::LongBreak);
        assert_eq!(p.done, 2);
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_left(1500.0), "25:00");
        assert_eq!(fmt_left(59.2), "01:00");
        assert_eq!(fmt_left(3723.0), "1:02:03");
    }
}
