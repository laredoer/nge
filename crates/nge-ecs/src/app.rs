//! App：World + 四个调度 + 渲染器列表；实现 `nge_core::Game` 接进引擎。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy_ecs::message::Messages;
use bevy_ecs::prelude::*;
use bevy_ecs::schedule::{IntoScheduleConfigs, ScheduleLabel};
use bevy_ecs::system::ScheduleSystem;
use glam::Vec2;
use nge_core::bridge::TOUCH;
use nge_core::{Ctx, Game, GameFactory, Gfx, Viewport, wgpu};
use serde_json::Value as JsonValue;

use crate::input::Input;
use crate::resources::{NshCommand, Outbox, Props, Screen, Time};

/// 第一帧 Update 之前跑一次。
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Startup;
/// 每帧最先跑：引擎内部（如把指针换算成世界坐标）。
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreUpdate;
/// 游戏逻辑。
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct Update;
/// 游戏逻辑之后：引擎内部（如整理渲染数据）。
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub struct PostUpdate;
/// 每帧最后：把变化了的 State 推给 NSH（引擎内部）。
#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Last;

/// 收到某个 prop / command 时把它转成类型化资源 / 消息。
pub(crate) type PropHandler = Box<dyn Fn(&mut World, Option<&JsonValue>)>;
pub(crate) type CommandHandler = Box<dyn Fn(&mut World, &JsonValue)>;

pub trait Plugin {
    fn build(&self, app: &mut App);
}

/// 渲染线程上每帧调用，把 World 画到目标上。由插件注册。
pub trait Renderer: 'static {
    fn render(&mut self, world: &mut World, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView);
}

pub struct App {
    world: World,
    renderers: Vec<Box<dyn Renderer>>,
    /// 每帧末尾轮转的消息队列（双缓冲），不轮转会无限增长。
    message_updaters: Vec<fn(&mut World)>,
    pub(crate) props: HashMap<&'static str, PropHandler>,
    pub(crate) commands: HashMap<&'static str, CommandHandler>,
    /// 已注册的游戏 → NSH 通道名（State 和 Event 共用 NSH 的事件名空间）。
    pub(crate) outbound: HashSet<&'static str>,
    started: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<Screen>();
        world.init_resource::<Props>();
        world.init_resource::<Outbox>();
        world.init_resource::<Input>();
        for schedule in [Schedule::new(Startup), Schedule::new(PreUpdate), Schedule::new(Update), Schedule::new(PostUpdate), Schedule::new(Last)] {
            world.add_schedule(schedule);
        }
        let mut app = Self {
            world,
            renderers: Vec::new(),
            message_updaters: Vec::new(),
            props: HashMap::new(),
            commands: HashMap::new(),
            outbound: HashSet::new(),
            started: false,
        };
        app.add_message::<NshCommand>();
        app
    }

    /// 注册一种消息：`MessageWriter<M>` / `MessageReader<M>` 可用，每帧末尾轮转。
    pub fn add_message<M: Message>(&mut self) -> &mut Self {
        if !self.world.contains_resource::<Messages<M>>() {
            self.world.init_resource::<Messages<M>>();
            self.message_updaters.push(|world| world.resource_mut::<Messages<M>>().update());
        }
        self
    }

    pub fn init_resource<R: Resource + FromWorld>(&mut self) -> &mut Self {
        self.world.init_resource::<R>();
        self
    }

    pub fn add_plugins(&mut self, plugin: impl Plugin) -> &mut Self {
        plugin.build(self);
        self
    }

    pub fn add_systems<M>(
        &mut self,
        schedule: impl ScheduleLabel,
        systems: impl IntoScheduleConfigs<ScheduleSystem, M>,
    ) -> &mut Self {
        self.world.resource_mut::<Schedules>().add_systems(schedule, systems);
        self
    }

    pub fn insert_resource(&mut self, resource: impl Resource) -> &mut Self {
        self.world.insert_resource(resource);
        self
    }

    pub fn add_renderer(&mut self, renderer: impl Renderer) -> &mut Self {
        self.renderers.push(Box::new(renderer));
        self
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// 收到 NSH 的 command：触摸进 Input，注册过的变成类型化消息，其余变成 [`NshCommand`]。
    pub fn command(&mut self, name: &str, args: &JsonValue) {
        if name == TOUCH.name() {
            match TOUCH.decode(args) {
                Ok(t) => self.world.resource_mut::<Input>().touch(t.phase, t.id, Vec2::new(t.x, t.y)),
                Err(e) => log::warn!("{e}"),
            }
        } else if let Some(handler) = self.commands.get(name) {
            handler(&mut self.world, args);
        } else {
            self.world.write_message(NshCommand { name: name.to_owned(), args: args.clone() });
        }
    }

    pub fn prop(&mut self, name: &str, value: Option<&JsonValue>) {
        let mut props = self.world.resource_mut::<Props>();
        match value {
            Some(v) => props.0.insert(name.to_owned(), v.clone()),
            None => props.0.remove(name),
        };
        if let Some(handler) = self.props.get(name) {
            handler(&mut self.world, value);
        }
    }

    /// 跑一帧逻辑，返回本帧要发回 NSH 的事件。
    pub fn tick(&mut self, viewport: Viewport, dt: f32) -> Vec<(String, JsonValue)> {
        let screen = Screen { size: Vec2::from(viewport.size), scale: viewport.scale() };
        if *self.world.resource::<Screen>() != screen {
            self.world.insert_resource(screen);
        }
        {
            let mut time = self.world.resource_mut::<Time>();
            time.delta = dt;
            time.elapsed += f64::from(dt);
            time.frame += 1;
        }
        if !self.started {
            self.started = true;
            self.world.run_schedule(Startup);
        }
        // 注：首帧之后再加的 Startup 系统不会运行
        self.world.run_schedule(PreUpdate);
        self.world.run_schedule(Update);
        self.world.run_schedule(PostUpdate);
        self.world.run_schedule(Last);

        self.world.resource_mut::<Input>().end_frame();
        for update in &self.message_updaters {
            update(&mut self.world);
        }
        // 轮转 RemovedComponents 等变更追踪缓冲，否则 despawn 记录会一直累积
        self.world.clear_trackers();
        self.world.resource_mut::<Outbox>().drain().collect()
    }
}

struct EcsGame(App);

impl Game for EcsGame {
    fn command(&mut self, _ctx: &mut Ctx, name: &str, args: &JsonValue) {
        self.0.command(name, args);
    }

    fn prop(&mut self, _ctx: &mut Ctx, name: &str, value: Option<&JsonValue>) {
        self.0.prop(name, value);
    }

    fn update(&mut self, ctx: &mut Ctx, dt: f32) {
        for (name, data) in self.0.tick(ctx.viewport, dt) {
            ctx.emit(&name, data);
        }
    }

    fn render(&mut self, gfx: &Gfx, encoder: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        let App { world, renderers, .. } = &mut self.0;
        for renderer in renderers {
            renderer.render(world, gfx, encoder, target);
        }
    }
}

/// 把构建 App 的函数包成引擎的游戏工厂，交给平台层注册。App 在渲染线程上构建。
pub fn factory(build: impl Fn() -> App + Send + Sync + 'static) -> GameFactory {
    Arc::new(move |_gfx: &Gfx| Box::new(EcsGame(build())) as Box<dyn Game>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn viewport() -> Viewport {
        Viewport { size: [400.0, 300.0], pixels: [800, 600] }
    }

    #[derive(Resource, Default)]
    struct Log(Vec<String>);

    #[test]
    fn startup_runs_once_before_update() {
        let mut app = App::new();
        app.init();
        app.add_systems(Startup, |mut log: ResMut<Log>| log.0.push("startup".into()))
            .add_systems(Update, |mut log: ResMut<Log>| log.0.push("update".into()));
        app.tick(viewport(), 0.016);
        app.tick(viewport(), 0.016);
        assert_eq!(app.world().resource::<Log>().0, ["startup", "update", "update"]);
    }

    #[test]
    fn time_and_screen_are_updated() {
        let mut app = App::new();
        app.tick(viewport(), 0.5);
        app.tick(viewport(), 0.25);
        let time = *app.world().resource::<Time>();
        assert_eq!((time.delta, time.elapsed, time.frame), (0.25, 0.75, 2));
        let screen = *app.world().resource::<Screen>();
        assert_eq!((screen.size, screen.scale), (Vec2::new(400.0, 300.0), 2.0));
    }

    #[test]
    fn commands_become_messages_and_outbox_is_flushed() {
        let mut app = App::new();
        app.add_systems(Update, |mut reader: MessageReader<NshCommand>, mut outbox: ResMut<Outbox>| {
            for cmd in reader.read() {
                outbox.emit("echo", json!({ "name": cmd.name, "args": cmd.args }));
            }
        });
        app.command("spawn", &json!({ "n": 3 }));
        let out = app.tick(viewport(), 0.016);
        assert_eq!(out, vec![("echo".to_owned(), json!({ "name": "spawn", "args": { "n": 3 } }))]);
        // 同一条消息不会被同一个 reader 读第二次
        assert!(app.tick(viewport(), 0.016).is_empty());
    }

    #[test]
    fn touch_goes_to_input_with_edges_cleared_after_frame() {
        let mut app = App::new();
        app.add_systems(Update, |input: Res<Input>, mut outbox: ResMut<Outbox>| {
            for p in input.pointers_pressed() {
                outbox.emit("tap", json!({ "x": p.screen.x, "y": p.screen.y }));
            }
        });
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 5.0, "y": 6.0 }));
        assert_eq!(app.tick(viewport(), 0.016), vec![("tap".to_owned(), json!({ "x": 5.0, "y": 6.0 }))]);
        assert!(app.tick(viewport(), 0.016).is_empty());
        assert_eq!(app.world().resource::<Input>().pointers_down().count(), 1);
    }

    #[derive(Message)]
    struct Ping;

    #[test]
    fn custom_messages_rotate_and_are_dropped_after_two_frames() {
        let mut app = App::new();
        app.add_message::<Ping>();
        app.world_mut().write_message(Ping);
        app.tick(viewport(), 0.016);
        assert_eq!(app.world().resource::<Messages<Ping>>().len(), 1);
        app.tick(viewport(), 0.016);
        assert_eq!(app.world().resource::<Messages<Ping>>().len(), 0);
    }

    #[test]
    fn removed_components_do_not_accumulate() {
        #[derive(Component)]
        struct Marker;
        let mut app = App::new();
        let e = app.world_mut().spawn(Marker).id();
        app.tick(viewport(), 0.016);
        app.world_mut().despawn(e);
        app.tick(viewport(), 0.016);
        app.tick(viewport(), 0.016);
        let id = app.world().component_id::<Marker>().unwrap();
        let removed = app.world().removed_components().get(id).map_or(0, |m| m.len());
        assert_eq!(removed, 0);
    }

    #[test]
    fn props_set_and_removed() {
        let mut app = App::new();
        app.prop("speed", Some(&json!(2.0)));
        assert_eq!(app.world().resource::<Props>().get("speed"), Some(&json!(2.0)));
        app.prop("speed", None);
        assert!(app.world().resource::<Props>().get("speed").is_none());
    }

    impl App {
        fn init(&mut self) {
            self.world.init_resource::<Log>();
        }
    }
}
