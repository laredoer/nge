//! 弹球的纯逻辑：小球、触摸、碰撞计分。不碰 GPU，便于单测。

use std::collections::HashMap;

use serde_json::{Value as JsonValue, json};

pub const MAX_BALLS: usize = 32;
const INITIAL_BALLS: usize = 6;
/// 移动不超过这个距离（point）算点击。
const TAP_SLOP: f32 = 8.0;
/// 甩出速度上限（point/s）。
const MAX_FLING: f32 = 2400.0;

#[derive(Clone, Copy)]
pub struct Ball {
    pub pos: [f32; 2],
    vel: [f32; 2],
    pub radius: f32,
    pub hue: f32,
    /// 正被手指按住：跟手，不参与物理。
    held: bool,
}

/// 一根手指的拖动状态。
struct Drag {
    /// 按住的球（下标），按在空白处为 None。
    ball: Option<usize>,
    start: [f32; 2],
    last: [f32; 2],
    last_ms: f64,
    /// 平滑后的手指速度（point/s）。
    vel: [f32; 2],
}

pub struct Sim {
    size: [f32; 2],
    pub balls: Vec<Ball>,
    drags: HashMap<u32, Drag>,
    paused: bool,
    speed: f32,
    score: u32,
    pub time: f32,
    seed: u32,
    /// 待发回 NSH 的事件，由调用方取走。
    events: Vec<(&'static str, JsonValue)>,
}

impl Default for Sim {
    fn default() -> Self {
        Self {
            size: [0.0; 2],
            balls: Vec::new(),
            drags: HashMap::new(),
            paused: false,
            speed: 1.0,
            score: 0,
            time: 0.0,
            seed: 0x9e37_79b9,
            events: Vec::new(),
        }
    }
}

impl Sim {
    fn emit(&mut self, name: &'static str, data: JsonValue) {
        self.events.push((name, data));
    }

    pub fn drain_events(&mut self) -> std::vec::Drain<'_, (&'static str, JsonValue)> {
        self.events.drain(..)
    }

    /// 推进一帧；视口尺寸首次有效时生成初始小球。
    pub fn update(&mut self, dt: f32, size: [f32; 2]) {
        self.size = size;
        if self.balls.is_empty() && self.score == 0 {
            self.reset(size);
        }
        if !self.paused {
            self.time += dt;
            self.step(dt, size);
        }
    }

    fn random(&mut self) -> f32 {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.seed >> 8) as f32 / (1 << 24) as f32
    }

    fn spawn(&mut self, size: [f32; 2]) {
        let radius = 12.0 + self.random() * 18.0;
        let angle = self.random() * std::f32::consts::TAU;
        let speed = 120.0 + self.random() * 160.0;
        let pos = [
            radius + self.random() * (size[0] - 2.0 * radius).max(1.0),
            radius + self.random() * (size[1] - 2.0 * radius).max(1.0),
        ];
        self.spawn_at(pos, [angle.cos() * speed, angle.sin() * speed], radius);
    }

    fn spawn_at(&mut self, pos: [f32; 2], vel: [f32; 2], radius: f32) {
        if self.balls.len() >= MAX_BALLS {
            return;
        }
        let hue = self.random();
        self.balls.push(Ball { pos, vel, radius, hue, held: false });
        self.emit_score();
    }

    /// 手指下最上层（最后画的）那个球。
    fn ball_at(&self, p: [f32; 2]) -> Option<usize> {
        self.balls.iter().rposition(|b| {
            let (dx, dy) = (p[0] - b.pos[0], p[1] - b.pos[1]);
            dx * dx + dy * dy <= (b.radius + 6.0).powi(2)
        })
    }

    /// NSH 转发过来的触摸：{ phase: start|move|end|cancel, id, x, y, t(ms) }。
    fn touch(&mut self, args: &JsonValue) {
        let num = |k: &str| args[k].as_f64().unwrap_or(0.0);
        let id = num("id") as u32;
        let p = [num("x") as f32, num("y") as f32];
        let t = num("t");
        let phase = args["phase"].as_str().unwrap_or("");
        if phase != "move" {
            log::debug!("touch {phase} id={id} at ({:.0}, {:.0})", p[0], p[1]);
        }
        match phase {
            "start" => {
                let ball = self.ball_at(p);
                if let Some(i) = ball {
                    self.balls[i].held = true;
                    self.balls[i].vel = [0.0; 2];
                }
                self.drags.insert(id, Drag { ball, start: p, last: p, last_ms: t, vel: [0.0; 2] });
            }
            "move" => {
                let Some(drag) = self.drags.get_mut(&id) else { return };
                let dt = ((t - drag.last_ms) / 1000.0) as f32;
                if dt > 0.0 {
                    for axis in 0..2 {
                        let v = (p[axis] - drag.last[axis]) / dt;
                        drag.vel[axis] = drag.vel[axis] * 0.4 + v * 0.6;
                    }
                }
                drag.last = p;
                drag.last_ms = t;
                if let Some(ball) = drag.ball.and_then(|i| self.balls.get_mut(i)) {
                    ball.pos = p;
                }
            }
            phase @ ("end" | "cancel") => {
                let Some(drag) = self.drags.remove(&id) else { return };
                let vel = clamp_len(drag.vel, MAX_FLING);
                match drag.ball.and_then(|i| self.balls.get_mut(i)) {
                    Some(ball) => {
                        ball.held = false;
                        ball.vel = vel;
                    }
                    None if phase == "end" => {
                        let moved = ((p[0] - drag.start[0]).powi(2) + (p[1] - drag.start[1]).powi(2)).sqrt();
                        let radius = 12.0 + self.random() * 18.0;
                        if moved <= TAP_SLOP {
                            // 点击：原地生成，随机方向
                            let angle = self.random() * std::f32::consts::TAU;
                            self.spawn_at(p, [angle.cos() * 160.0, angle.sin() * 160.0], radius);
                        } else {
                            // 空白处拖动：从起点按拖动方向发射
                            let dir = [p[0] - drag.start[0], p[1] - drag.start[1]];
                            let speed = (moved * 4.0).min(MAX_FLING);
                            self.spawn_at(drag.start, [dir[0] / moved * speed, dir[1] / moved * speed], radius);
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    fn reset(&mut self, size: [f32; 2]) {
        self.balls.clear();
        self.drags.clear();
        self.score = 0;
        for _ in 0..INITIAL_BALLS {
            self.spawn(size);
        }
        self.emit_score();
    }

    fn emit_score(&mut self) {
        self.emit("score", json!({ "score": self.score, "balls": self.balls.len() }));
    }

    fn step(&mut self, dt: f32, size: [f32; 2]) {
        let mut hits = 0;
        for ball in self.balls.iter_mut().filter(|b| !b.held) {
            for axis in 0..2 {
                ball.pos[axis] += ball.vel[axis] * dt * self.speed;
                let (lo, hi) = (ball.radius, size[axis] - ball.radius);
                if ball.pos[axis] < lo && ball.vel[axis] < 0.0 {
                    ball.pos[axis] = lo;
                    ball.vel[axis] = -ball.vel[axis];
                    hits += 1;
                } else if ball.pos[axis] > hi && ball.vel[axis] > 0.0 {
                    ball.pos[axis] = hi.max(lo);
                    ball.vel[axis] = -ball.vel[axis];
                    hits += 1;
                }
            }
        }
        if hits > 0 {
            self.score += hits;
            self.emit_score();
        }
    }

    pub fn set_speed(&mut self, value: Option<&JsonValue>) {
        let speed = value.and_then(JsonValue::as_f64).unwrap_or(1.0);
        self.speed = speed.clamp(0.1, 5.0) as f32;
    }

    pub fn command(&mut self, name: &str, args: &JsonValue) {
        let size = self.size;
        match name {
            "pause" => self.paused = true,
            "resume" => self.paused = false,
            "spawn" => self.spawn(size),
            "reset" => self.reset(size),
            "touch" => return self.touch(args),
            other => log::warn!("game.view 未知命令: {other}"),
        }
        self.emit("state", json!({ "paused": self.paused }));
    }
}

fn clamp_len(v: [f32; 2], max: f32) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len > max { [v[0] / len * max, v[1] / len * max] } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game() -> Sim {
        Sim { size: [400.0, 600.0], ..Sim::default() }
    }

    fn touch(game: &mut Sim, phase: &str, x: f32, y: f32, t: f64) {
        game.command("touch", &json!({ "phase": phase, "id": 1, "x": x, "y": y, "t": t }));
    }

    #[test]
    fn first_update_spawns_initial_balls_and_reports_score() {
        let mut g = Sim::default();
        g.update(0.016, [400.0, 600.0]);
        assert_eq!(g.balls.len(), INITIAL_BALLS);
        let events: Vec<_> = g.drain_events().collect();
        assert!(events.iter().any(|(n, d)| *n == "score" && d["balls"] == INITIAL_BALLS), "{events:?}");
        assert_eq!(g.drain_events().count(), 0, "取走后应清空");
    }

    #[test]
    fn paused_sim_does_not_move() {
        let mut g = game();
        g.spawn_at([200.0, 200.0], [100.0, 0.0], 20.0);
        g.command("pause", &JsonValue::Null);
        g.update(0.1, [400.0, 600.0]);
        assert_eq!(g.balls[0].pos, [200.0, 200.0]);
    }

    #[test]
    fn tap_on_empty_space_spawns_ball_there() {
        let mut g = game();
        touch(&mut g, "start", 100.0, 200.0, 0.0);
        touch(&mut g, "end", 102.0, 201.0, 80.0);
        assert_eq!(g.balls.len(), 1);
        assert_eq!(g.balls[0].pos, [102.0, 201.0]);
    }

    #[test]
    fn drag_on_empty_space_launches_ball_along_drag() {
        let mut g = game();
        touch(&mut g, "start", 100.0, 300.0, 0.0);
        touch(&mut g, "move", 200.0, 300.0, 50.0);
        touch(&mut g, "end", 200.0, 300.0, 60.0);
        assert_eq!(g.balls.len(), 1);
        let b = g.balls[0];
        assert_eq!(b.pos, [100.0, 300.0]);
        assert!(b.vel[0] > 0.0 && b.vel[1].abs() < 1e-3, "向右发射: {:?}", b.vel);
    }

    #[test]
    fn grabbed_ball_follows_finger_and_is_flung() {
        let mut g = game();
        g.spawn_at([200.0, 200.0], [50.0, 50.0], 20.0);
        touch(&mut g, "start", 205.0, 200.0, 0.0);
        assert!(g.balls[0].held);
        assert_eq!(g.balls[0].vel, [0.0, 0.0]);

        touch(&mut g, "move", 215.0, 200.0, 16.0);
        touch(&mut g, "move", 225.0, 200.0, 32.0);
        assert_eq!(g.balls[0].pos, [225.0, 200.0]);

        g.step(0.1, g.size);
        assert_eq!(g.balls[0].pos, [225.0, 200.0], "按住时不参与物理");

        touch(&mut g, "end", 225.0, 200.0, 40.0);
        let b = g.balls[0];
        assert!(!b.held);
        assert!(b.vel[0] > 300.0, "按手势速度甩出: {:?}", b.vel);
        assert_eq!(g.balls.len(), 1, "甩球不生成新球");
    }

    #[test]
    fn fling_speed_is_capped() {
        let mut g = game();
        g.spawn_at([200.0, 200.0], [0.0, 0.0], 20.0);
        touch(&mut g, "start", 200.0, 200.0, 0.0);
        touch(&mut g, "move", 390.0, 200.0, 1.0);
        touch(&mut g, "end", 390.0, 200.0, 2.0);
        let v = g.balls[0].vel;
        assert!((v[0] * v[0] + v[1] * v[1]).sqrt() <= MAX_FLING + 1e-3);
    }

    #[test]
    fn cancel_releases_ball_without_spawning() {
        let mut g = game();
        touch(&mut g, "start", 50.0, 50.0, 0.0);
        touch(&mut g, "cancel", 50.0, 50.0, 10.0);
        assert!(g.balls.is_empty());
    }
}
