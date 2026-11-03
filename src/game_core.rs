//! 平台无关的游戏核心：wgpu 渲染 + 弹球逻辑。各平台只负责提供 View 句柄、尺寸和帧驱动。

use std::collections::HashMap;
use std::time::Instant;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use serde_json::{Value as JsonValue, json};

const MAX_BALLS: usize = 32;
const INITIAL_BALLS: usize = 6;
/// 移动不超过这个距离（point）算点击。
const TAP_SLOP: f32 = 8.0;
/// 甩出速度上限（point/s）。
const MAX_FLING: f32 = 2400.0;

const SHADER: &str = r#"
struct Uniforms {
    size: vec2f,
    scale: f32,
    time: f32,
    count: u32,
    _p0: u32, _p1: u32, _p2: u32,
    balls: array<vec4f, 32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let p = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(p * 2.0 - 1.0, 0.0, 1.0);
}

fn hue(h: f32) -> vec3f {
    let k = vec3f(0.0, 2.0 / 3.0, 1.0 / 3.0);
    return clamp(abs(fract(h + k) * 6.0 - 3.0) - 1.0, vec3f(0.0), vec3f(1.0));
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = pos.xy / u.scale;
    let uv = p / max(u.size, vec2f(1.0));
    var col = mix(vec3f(0.03, 0.04, 0.09), vec3f(0.09, 0.03, 0.14), uv.y);
    let g = abs(fract(p / 32.0) - 0.5);
    col += vec3f(0.035) * step(0.47, max(g.x, g.y));
    for (var i = 0u; i < u.count; i++) {
        let b = u.balls[i];
        let d = length(p - b.xy);
        let c = hue(b.w + u.time * 0.05);
        let glow = exp(-max(d - b.z, 0.0) / 12.0) * 0.45;
        let body = 1.0 - smoothstep(b.z - 1.0, b.z + 1.0, d);
        let shade = 1.15 - 0.5 * length(p - b.xy + vec2f(b.z * 0.35)) / b.z;
        col = mix(col + c * glow, c * shade, body);
    }
    return vec4f(col, 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    size: [f32; 2],
    scale: f32,
    time: f32,
    count: u32,
    _pad: [u32; 3],
    balls: [[f32; 4]; MAX_BALLS],
}

#[derive(Clone, Copy)]
struct Ball {
    pos: [f32; 2],
    vel: [f32; 2],
    radius: f32,
    hue: f32,
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

struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl Gpu {
    /// 在平台 View 上创建 surface；View 必须比 Game 活得久（由各平台组件持有保证）。
    fn new(target: Target, width: u32, height: u32) -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: Some(target.display),
            raw_window_handle: target.window,
        };
        let surface = unsafe { instance.create_surface_unsafe(target) }.map_err(|e| e.to_string())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))
            .map_err(|e| e.to_string())?;
        let config = surface
            .get_default_config(&adapter, width, height)
            .ok_or("surface 不被该 adapter 支持")?;
        surface.configure(&device, &config);

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("game"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("game"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(config.format.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("uniforms"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        Ok(Self { surface, device, queue, config, pipeline, uniforms, bind_group })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn draw(&mut self, uniforms: &Uniforms) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        self.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(uniforms));
        let target = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("game"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }
}

/// 游戏逻辑：弹球撞墙得分。坐标单位是 point。
/// 平台 View 的原始句柄。
#[derive(Clone, Copy)]
pub struct Target {
    pub window: RawWindowHandle,
    pub display: RawDisplayHandle,
}

/// 一帧的视口：point 尺寸 + 像素尺寸。
#[derive(Clone, Copy)]
pub struct Viewport {
    pub size: [f32; 2],
    pub pixels: [u32; 2],
}

/// 把事件发回 NSH（各平台的 ComponentContext::emit）。
pub type Emit = Box<dyn Fn(&str, JsonValue)>;

pub struct Game {
    emit: Emit,
    target: Target,
    size: [f32; 2],
    gpu: Option<Gpu>,
    gpu_failed: bool,
    balls: Vec<Ball>,
    drags: HashMap<u32, Drag>,
    paused: bool,
    speed: f32,
    score: u32,
    time: f32,
    seed: u32,
    last: Instant,
    fps_frames: u32,
    fps_since: Instant,
}

impl Game {
    pub fn new(target: Target, emit: Emit) -> Self {
        let now = Instant::now();
        Self {
            emit,
            target,
            size: [0.0; 2],
            gpu: None,
            gpu_failed: false,
            balls: Vec::new(),
            drags: HashMap::new(),
            paused: false,
            speed: 1.0,
            score: 0,
            time: 0.0,
            seed: 0x9e37_79b9,
            last: now,
            fps_frames: 0,
            fps_since: now,
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

    fn emit_score(&self) {
        (self.emit)("score", json!({ "score": self.score, "balls": self.balls.len() }));
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

    fn uniforms(&self, size: [f32; 2], scale: f32) -> Uniforms {
        let mut balls = [[0.0; 4]; MAX_BALLS];
        for (slot, b) in balls.iter_mut().zip(&self.balls) {
            *slot = [b.pos[0], b.pos[1], b.radius, b.hue];
        }
        Uniforms {
            size,
            scale,
            time: self.time,
            count: self.balls.len() as u32,
            _pad: [0; 3],
            balls,
        }
    }

    /// 每帧调用：推进逻辑、按需创建/调整 GPU 资源、绘制。
    pub fn tick(&mut self, viewport: Viewport) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.05);
        self.last = now;

        let Viewport { size, pixels: [pw, ph] } = viewport;
        if size[0] < 1.0 || size[1] < 1.0 || pw == 0 || ph == 0 {
            return;
        }
        self.size = size;
        let scale = pw as f32 / size[0];

        if self.gpu.is_none() && !self.gpu_failed {
            match Gpu::new(self.target, pw, ph) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(e) => {
                    self.gpu_failed = true;
                    log::error!("游戏视图初始化 GPU 失败: {e}");
                    (self.emit)("error", json!({ "message": e }));
                    return;
                }
            }
        }
        if self.balls.is_empty() && self.score == 0 {
            self.reset(size);
        }
        if !self.paused {
            self.time += dt;
            self.step(dt, size);
        }

        let uniforms = self.uniforms(size, scale);
        if let Some(gpu) = &mut self.gpu {
            gpu.resize(pw, ph);
            gpu.draw(&uniforms);
        }

        self.fps_frames += 1;
        let elapsed = (now - self.fps_since).as_secs_f32();
        if elapsed >= 1.0 {
            let fps = (self.fps_frames as f32 / elapsed).round() as u32;
            (self.emit)("fps", json!({ "fps": fps }));
            self.fps_frames = 0;
            self.fps_since = now;
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
        (self.emit)("state", json!({ "paused": self.paused }));
    }
}

fn clamp_len(v: [f32; 2], max: f32) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len > max { [v[0] / len * max, v[1] / len * max] } else { v }
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;

    use raw_window_handle::{AppKitDisplayHandle, AppKitWindowHandle};

    use super::*;

    /// 不会创建 GPU（不调用 tick），句柄只占位。
    fn game() -> Game {
        let target = Target {
            window: RawWindowHandle::AppKit(AppKitWindowHandle::new(NonNull::dangling())),
            display: RawDisplayHandle::AppKit(AppKitDisplayHandle::new()),
        };
        let mut game = Game::new(target, Box::new(|_, _| {}));
        game.size = [400.0, 600.0];
        game
    }

    fn touch(game: &mut Game, phase: &str, x: f32, y: f32, t: f64) {
        game.command("touch", &json!({ "phase": phase, "id": 1, "x": x, "y": y, "t": t }));
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
