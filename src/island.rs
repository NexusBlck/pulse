//! The island's outline: width, height and corner radius, each on a spring,
//! plus the opacity of the content inside it.

use crate::config::Motion;
use crate::spring::Spring;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geom {
    pub w: f32,
    pub h: f32,
    pub r: f32,
}

impl Geom {
    pub const fn new(w: f32, h: f32, r: f32) -> Self {
        Self { w, h, r }
    }
}

/// Delay before new content starts fading in (seconds), so the outline
/// leads and the content follows.
const FADE_DELAY: f64 = 0.04;

pub struct Island {
    w: Spring,
    h: Spring,
    r: Spring,
    content: Spring,
    fade_at: Option<f64>,
    target: Geom,
    motion: Motion,
}

impl Island {
    pub fn new(g: Geom, motion: Motion) -> Self {
        let mut i = Self {
            w: Spring::new(g.w, 1.0, 1.0),
            h: Spring::new(g.h, 1.0, 1.0),
            r: Spring::new(g.r, 1.0, 1.0),
            content: Spring::new(1.0, 700.0, Spring::damping_for(700.0, 1.0)),
            fade_at: None,
            target: g,
            motion,
        };
        i.tune();
        i
    }

    pub fn set_motion(&mut self, m: Motion) {
        self.motion = m;
        self.tune();
    }

    pub fn target(&self) -> Geom {
        self.target
    }

    /// Head for `g`. With `new_content` the content restarts from transparent
    /// and fades in just after the outline starts moving.
    pub fn set_target(&mut self, g: Geom, now: f64, new_content: bool) {
        self.target = g;
        self.tune();
        self.w.target = g.w;
        self.h.target = g.h;
        self.r.target = g.r;
        if new_content {
            self.content.snap(0.0);
            self.fade_at = Some(now + FADE_DELAY);
        }
    }

    /// The same content coming back mid-fade picks up from `alpha`.
    pub fn resume_content(&mut self, alpha: f32) {
        self.content.snap(alpha.clamp(0.0, 1.0));
        self.content.target = 1.0;
        self.fade_at = None;
    }

    /// Growing: the width leads and the height trails, so the pill stretches
    /// wide and then drops open. Shrinking: the height leads, nearly
    /// critically damped so the small pill never squashes. Corners never wobble.
    fn tune(&mut self) {
        let speed = self.motion.speed * self.motion.speed;
        let bounce = self.motion.bounce;
        let growing = self.target.w * self.target.h > self.w.value * self.h.value;
        let (kw, kh, zeta) = if growing {
            (440.0 * speed, 300.0 * speed, 1.0 - bounce)
        } else {
            (340.0 * speed, 440.0 * speed, (1.0 - bounce * 0.35).min(1.0))
        };
        self.w.tune(kw, Spring::damping_for(kw, zeta));
        self.h.tune(kh, Spring::damping_for(kh, zeta));
        let kr = 420.0 * speed;
        self.r.tune(kr, Spring::damping_for(kr, 1.0));
    }

    pub fn snap(&mut self, g: Geom) {
        self.target = g;
        self.w.snap(g.w);
        self.h.snap(g.h);
        self.r.snap(g.r);
        self.content.snap(1.0);
        self.fade_at = None;
    }

    pub fn step(&mut self, dt: f32, now: f64) {
        self.w.step(dt);
        self.h.step(dt);
        self.r.step(dt);
        if self.fade_at.is_some_and(|t| now >= t) {
            self.content.target = 1.0;
            self.fade_at = None;
        }
        self.content.step(dt);
    }

    pub fn settled(&self) -> bool {
        self.fade_at.is_none() && self.w.settled() && self.h.settled() && self.r.settled() && self.content.settled()
    }

    pub fn current(&self) -> Geom {
        let h = self.h.value.max(4.0);
        Geom { w: self.w.value.max(20.0), h, r: self.r.value.clamp(0.0, h) }
    }

    pub fn content_alpha(&self) -> f32 {
        self.content.value.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PILL: Geom = Geom::new(90.0, 30.0, 15.0);
    const CARD: Geom = Geom::new(560.0, 200.0, 30.0);

    fn run(i: &mut Island) -> (f32, f32) {
        let (mut wmax, mut hmin) = (0.0f32, f32::MAX);
        let mut t = 0.0;
        while !i.settled() && t < 3.0 {
            i.step(1.0 / 60.0, t as f64);
            t += 1.0 / 60.0;
            wmax = wmax.max(i.current().w);
            hmin = hmin.min(i.current().h);
        }
        (wmax, hmin)
    }

    #[test]
    fn grows_with_a_little_overshoot_and_settles() {
        let mut i = Island::new(PILL, Motion::default());
        i.set_target(CARD, 0.0, true);
        let (wmax, _) = run(&mut i);
        assert!(i.settled());
        assert_eq!(i.current(), CARD);
        assert!(wmax > CARD.w && wmax < CARD.w * 1.06, "peak {wmax}");
    }

    #[test]
    fn shrinks_without_squashing() {
        let mut i = Island::new(CARD, Motion::default());
        i.set_target(PILL, 0.0, true);
        let (_, hmin) = run(&mut i);
        assert_eq!(i.current(), PILL);
        assert!(hmin > PILL.h - 2.0, "squashed to {hmin}");
    }
}
