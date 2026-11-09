//! 小状态机：现在的状态和进了这个状态多久。

/// 现在的状态 `S` 和进了这个状态多少秒。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fsm<S> {
    state: S,
    time: f32,
}

impl<S: Copy + PartialEq> Fsm<S> {
    pub fn new(state: S) -> Self {
        Self { state, time: 0.0 }
    }

    pub fn state(&self) -> S {
        self.state
    }

    /// 进了这个状态多少秒。
    pub fn time(&self) -> f32 {
        self.time
    }

    pub fn tick(&mut self, dt: f32) {
        self.time += dt;
    }

    /// 换到 `state`：换了才把时间清零，返回换没换。
    pub fn go(&mut self, state: S) -> bool {
        let changed = state != self.state;
        if changed {
            *self = Self::new(state);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_resets_only_when_the_state_changes() {
        let mut fsm = Fsm::new(1);
        fsm.tick(0.5);
        fsm.tick(0.25);
        assert_eq!((fsm.state(), fsm.time()), (1, 0.75));
        assert!(!fsm.go(1));
        assert_eq!(fsm.time(), 0.75, "还是原来的状态，时间接着算");
        assert!(fsm.go(2));
        assert_eq!((fsm.state(), fsm.time()), (2, 0.0));
    }
}
