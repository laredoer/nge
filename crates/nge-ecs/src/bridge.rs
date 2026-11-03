//! 把 NSH 的类型化通道接进 ECS：prop → 资源，command → 消息，资源变化 → State 推送。

use bevy_ecs::prelude::*;
use nge_core::bridge::{CommandKey, EventKey, PropKey, StateKey, is_reserved};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::app::{App, Last};
use crate::resources::Outbox;

impl App {
    /// NSH 绑定的值成为资源 `T`（先用 `T::default()`）；NSH 移除这个 prop 时恢复默认值。
    pub fn add_prop<T: Resource + DeserializeOwned + Default>(&mut self, key: PropKey<T>) -> &mut Self {
        self.init_resource::<T>();
        let handler = move |world: &mut World, value: Option<&serde_json::Value>| match value.map(|v| key.decode(v)) {
            Some(Ok(v)) => world.insert_resource(v),
            Some(Err(e)) => log::warn!("{e}"),
            None => world.insert_resource(T::default()),
        };
        if self.props.contains_key(key.name()) {
            log::error!("prop {} 重复注册，忽略后一次", key.name());
            return self;
        }
        self.props.insert(key.name(), Box::new(handler));
        self
    }

    /// NSH 发来的 command 成为消息 `T`（`MessageReader<T>` 读取）。
    pub fn add_command<T: Message + DeserializeOwned>(&mut self, key: CommandKey<T>) -> &mut Self {
        if is_reserved(key.name(), false) {
            log::error!("command {} 是引擎保留名，忽略", key.name());
            return self;
        }
        if self.commands.contains_key(key.name()) {
            log::error!("command {} 重复注册，忽略后一次", key.name());
            return self;
        }
        self.add_message::<T>();
        let handler = move |world: &mut World, args: &serde_json::Value| match key.decode(args) {
            Ok(v) => {
                world.write_message(v);
            }
            Err(e) => log::warn!("{e}"),
        };
        self.commands.insert(key.name(), Box::new(handler));
        self
    }

    /// 资源 `T` 被修改（含首次插入）时，在帧末推给 NSH。没有这个资源时不推。
    /// 资源被移除时不推送，NSH 侧保留最后的值。
    pub fn add_state<T: Resource + Serialize>(&mut self, key: StateKey<T>) -> &mut Self {
        if !self.claim_outbound(key.name()) {
            return self;
        }
        self.add_systems(Last, move |state: Option<Res<T>>, mut outbox: ResMut<Outbox>| {
            if let Some(state) = state
                && state.is_changed()
            {
                outbox.emit(key.name(), key.encode(&state));
            }
        })
    }

    /// 登记一个事件（发送不需要登记）：用于检查和 State / 其他事件 / 保留名重名。
    pub fn add_event<T>(&mut self, key: EventKey<T>) -> &mut Self {
        self.claim_outbound(key.name());
        self
    }

    /// State 和 Event 共用 NSH 的事件名空间，名字不能重复，也不能用引擎保留名。
    fn claim_outbound(&mut self, name: &'static str) -> bool {
        if is_reserved(name, true) {
            log::error!("{name} 是引擎保留的事件名，忽略");
            return false;
        }
        if !self.outbound.insert(name) {
            log::error!("事件名 {name} 已被 State / Event 占用，忽略");
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use nge_core::{EventKey, Viewport};
    use serde::Deserialize;
    use serde_json::json;

    use super::*;
    use crate::{Startup, Update};

    #[derive(Resource, Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
    struct Difficulty(u8);

    #[derive(Message, Debug, Clone, PartialEq, Deserialize)]
    enum Control {
        Pause,
        Jump { height: f32 },
    }

    #[derive(Resource, Debug, Default, Clone, PartialEq, Serialize)]
    struct Hud {
        score: u32,
    }

    #[derive(Serialize)]
    struct Over {
        score: u32,
    }

    const DIFFICULTY: PropKey<Difficulty> = PropKey::new("difficulty");
    const CONTROL: CommandKey<Control> = CommandKey::new("control");
    const HUD: StateKey<Hud> = StateKey::new("hud");
    const OVER: EventKey<Over> = EventKey::new("over");

    fn viewport() -> Viewport {
        Viewport { size: [100.0, 100.0], pixels: [100, 100] }
    }

    #[test]
    fn prop_becomes_resource_and_resets_on_remove() {
        let mut app = App::new();
        app.add_prop(DIFFICULTY);
        assert_eq!(*app.world().resource::<Difficulty>(), Difficulty(0));
        app.prop("difficulty", Some(&json!(3)));
        assert_eq!(*app.world().resource::<Difficulty>(), Difficulty(3));
        // 类型不对：保留原值
        app.prop("difficulty", Some(&json!("hard")));
        assert_eq!(*app.world().resource::<Difficulty>(), Difficulty(3));
        app.prop("difficulty", None);
        assert_eq!(*app.world().resource::<Difficulty>(), Difficulty(0));
    }

    #[test]
    fn command_becomes_typed_message() {
        #[derive(Resource, Default)]
        struct Seen(Vec<Control>);
        let mut app = App::new();
        app.init_resource::<Seen>().add_command(CONTROL).add_systems(
            Update,
            |mut reader: MessageReader<Control>, mut seen: ResMut<Seen>| seen.0.extend(reader.read().cloned()),
        );
        app.command("control", &json!("Pause"));
        app.command("control", &json!({ "Jump": { "height": 2.0 } }));
        app.command("control", &json!({ "Fly": {} }));
        app.tick(viewport(), 0.016);
        assert_eq!(app.world().resource::<Seen>().0, vec![Control::Pause, Control::Jump { height: 2.0 }]);
    }

    #[test]
    fn state_pushed_on_insert_and_change_only() {
        let mut app = App::new();
        app.insert_resource(Hud::default()).add_state(HUD).add_systems(Update, |time: Res<crate::Time>, mut hud: ResMut<Hud>| {
            // 第 3 帧才真正改分数；set_if_neq 避免每帧标记修改
            hud.set_if_neq(Hud { score: if time.frame >= 3 { 10 } else { 0 } });
        });
        let frames: Vec<_> = (0..4).map(|_| app.tick(viewport(), 0.016)).collect();
        assert_eq!(frames[0], vec![("hud".to_owned(), json!({ "score": 0 }))]);
        assert!(frames[1].is_empty());
        assert_eq!(frames[2], vec![("hud".to_owned(), json!({ "score": 10 }))]);
        assert!(frames[3].is_empty());
    }

    #[test]
    fn state_inserted_later_by_startup_is_pushed() {
        let mut app = App::new();
        app.add_state(HUD).add_systems(Startup, |mut commands: Commands| commands.insert_resource(Hud { score: 1 }));
        assert_eq!(app.tick(viewport(), 0.016), vec![("hud".to_owned(), json!({ "score": 1 }))]);
    }

    #[test]
    fn reserved_and_duplicate_names_are_rejected() {
        let mut app = App::new();
        app.insert_resource(Hud::default())
            .add_state(HUD)
            .add_state(HUD)
            .add_event(EventKey::<Over>::new("hud"))
            .add_state(StateKey::<Hud>::new("fps"))
            .add_command(CommandKey::<Control>::new("touch"))
            .add_command(CommandKey::<Control>::new("nge.custom"));
        // 只推一次（重复的 add_state 被忽略，保留名的也没注册）
        assert_eq!(app.tick(viewport(), 0.016), vec![("hud".to_owned(), json!({ "score": 0 }))]);
        assert!(app.commands.is_empty());
        // touch 仍然进 Input
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 0.0, "y": 0.0 }));
        assert_eq!(app.world().resource::<crate::Input>().pointers_down().count(), 1);
    }

    #[test]
    fn prop_before_first_tick_is_visible_in_startup_and_update() {
        #[derive(Resource, Default)]
        struct Seen(Vec<u8>);
        let mut app = App::new();
        app.init_resource::<Seen>()
            .add_prop(DIFFICULTY)
            .add_systems(Startup, |d: Res<Difficulty>, mut seen: ResMut<Seen>| seen.0.push(d.0))
            .add_systems(Update, |d: Res<Difficulty>, mut seen: ResMut<Seen>| seen.0.push(d.0));
        app.prop("difficulty", Some(&json!(2)));
        app.tick(viewport(), 0.016);
        assert_eq!(app.world().resource::<Seen>().0, vec![2, 2]);
    }

    #[test]
    fn registered_command_is_not_also_an_nsh_command() {
        let mut app = App::new();
        app.add_command(CONTROL);
        app.command("control", &json!("Pause"));
        assert!(app.world().resource::<bevy_ecs::message::Messages<crate::NshCommand>>().is_empty());
    }

    #[test]
    fn typed_event_and_touch() {
        let mut app = App::new();
        app.add_systems(Update, |input: Res<crate::Input>, mut outbox: ResMut<Outbox>| {
            if input.pointers_pressed().next().is_some() {
                outbox.send(OVER, &Over { score: 7 });
            }
        });
        app.command("touch", &json!({ "phase": "start", "id": 1, "x": 1.0, "y": 2.0 }));
        assert_eq!(app.tick(viewport(), 0.016), vec![("over".to_owned(), json!({ "score": 7 }))]);
    }
}
