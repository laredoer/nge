//! 把弹球接到引擎上：逻辑交给 Sim，绘制交给 Renderer。

use nge_core::{Ctx, Game, Gfx, wgpu};
use serde_json::Value as JsonValue;

use crate::render::Renderer;
use crate::sim::Sim;

pub struct Bounce {
    sim: Sim,
    renderer: Renderer,
}

impl Bounce {
    pub fn new(gfx: &Gfx) -> Self {
        Self { sim: Sim::default(), renderer: Renderer::new(gfx) }
    }

    fn flush(&mut self, ctx: &Ctx) {
        for (name, data) in self.sim.drain_events() {
            ctx.emit(name, data);
        }
    }
}

impl Game for Bounce {
    fn command(&mut self, ctx: &mut Ctx, name: &str, args: &JsonValue) {
        self.sim.command(name, args);
        self.flush(ctx);
    }

    fn prop(&mut self, _ctx: &mut Ctx, name: &str, value: Option<&JsonValue>) {
        if name == "speed" {
            self.sim.set_speed(value);
        }
    }

    fn update(&mut self, ctx: &mut Ctx, dt: f32) {
        self.sim.update(dt, ctx.viewport.size);
        self.flush(ctx);
    }

    fn render(&mut self, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        self.renderer.render(gfx, &self.sim, encoder, target);
    }
}
