//! A damped spring. Every moving part of the island is one of these: when a
//! target changes mid-flight the spring keeps its velocity, so motion bends
//! instead of restarting.

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub velocity: f32,
    pub target: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl Spring {
    pub fn new(value: f32, stiffness: f32, damping: f32) -> Self {
        Self { value, velocity: 0.0, target: value, stiffness, damping }
    }

    /// Damping for a damping ratio `zeta` (1 = critical, below 1 overshoots).
    pub fn damping_for(stiffness: f32, zeta: f32) -> f32 {
        2.0 * zeta * stiffness.sqrt()
    }

    pub fn tune(&mut self, stiffness: f32, damping: f32) {
        self.stiffness = stiffness;
        self.damping = damping;
    }

    pub fn snap(&mut self, v: f32) {
        self.value = v;
        self.target = v;
        self.velocity = 0.0;
    }

    pub fn step(&mut self, dt: f32) {
        if self.settled_eps(self.eps()) {
            self.value = self.target;
            self.velocity = 0.0;
            return;
        }
        // Semi-implicit Euler in fixed sub-steps: stable at any frame rate.
        const H: f32 = 1.0 / 240.0;
        let mut left = dt.clamp(0.0, 0.1);
        while left > 0.0 {
            let h = left.min(H);
            let force = -self.stiffness * (self.value - self.target) - self.damping * self.velocity;
            self.velocity += force * h;
            self.value += self.velocity * h;
            left -= h;
        }
        if self.settled_eps(self.eps()) {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }

    /// Opacity-sized springs (0..1) need a finer tolerance than DIP-sized ones.
    fn eps(&self) -> f32 {
        if self.target.abs() <= 1.0 && self.value.abs() <= 1.5 {
            0.002
        } else {
            0.05
        }
    }

    pub fn settled(&self) -> bool {
        self.settled_eps(self.eps())
    }

    fn settled_eps(&self, eps: f32) -> bool {
        (self.value - self.target).abs() < eps && self.velocity.abs() < eps * 4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settles_on_target() {
        let mut s = Spring::new(0.0, 400.0, Spring::damping_for(400.0, 0.8));
        s.target = 100.0;
        for _ in 0..240 {
            s.step(1.0 / 60.0);
        }
        assert!(s.settled());
        assert_eq!(s.value, 100.0);
    }

    #[test]
    fn keeps_velocity_when_retargeted() {
        let mut s = Spring::new(0.0, 400.0, 30.0);
        s.target = 100.0;
        for _ in 0..5 {
            s.step(1.0 / 60.0);
        }
        let v = s.velocity;
        s.target = 0.0;
        assert_eq!(s.velocity, v);
    }
}
