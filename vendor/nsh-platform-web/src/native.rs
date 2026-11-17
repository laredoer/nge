//! 原生组件（本仓库加的，nsh 0.4.0 的 web 平台没有）：照 macOS 平台的
//! `NativeComponent` / `ComponentContext` / `ComponentFactory`。
//!
//! - 生成的 web 运行时不调用应用的组件注册函数，所以用全局的
//!   [`register_component`]：应用在自己的 `#[native_module]` 里注册（打包时
//!   只有 TSX import 了这个模块，它的 gate 才会保留）。
//! - 组件的 DOM 由组件自己建：容器是 NSH 的 view 元素，带
//!   `data-nsh-id`；页面上可能有几个播放器，选择器限在本播放器的根
//!   （`data-nsh-player`，[`ComponentContext::selector`]）。
//! - 不支持 `ComponentPropSlots`（属性里放 TSX 子树）：这样的属性组件收不到。
//! - 每批挂载项先交给 JS 建好 DOM，之后（[`crate::flush`] 放开 `APP` 以后）
//!   按顺序处理这一批的组件创建、属性、`command`、删除。

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use nsh_core::{Command, EventKind, EventPayload, MountItem, NodeId, Points, Size};
use nsh_engine::EngineControl;
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value as JsonValue;

/// 应用注册的原生组件。
pub trait NativeComponent {
    /// 属性 `name` 变了（`None`：删掉了）。组件刚建好时会收到应用设的全部属性。
    fn set_prop(&mut self, name: &str, value: Option<&JsonValue>);

    /// JS 调了 `ref.command(name, args)`。
    fn command(&mut self, name: &str, args: &JsonValue) {
        let _ = (name, args);
    }
}

/// 组件能对应用做的事。
#[derive(Clone)]
pub struct ComponentContext {
    id: NodeId,
    /// 播放器的编号（`data-nsh-player`）；0：页面没说。
    player: u32,
    /// 测试里没有引擎。
    control: Option<EngineControl>,
}

impl std::fmt::Debug for ComponentContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentContext").field("id", &self.id).finish()
    }
}

impl ComponentContext {
    /// 容器 view 的编号。
    pub fn id(&self) -> NodeId {
        self.id
    }

    /// 找容器元素用的 CSS 选择器：本播放器根下的 `[data-nsh-id="…"]`。
    pub fn selector(&self) -> String {
        let own = format!("[data-nsh-id=\"{}\"]", self.id);
        match self.player {
            0 => own,
            player => format!("[data-nsh-player=\"{player}\"] {own}"),
        }
    }

    /// 发事件 `name`：TSX 的 `on<Name>` 收到 `data`。
    pub fn emit(&self, name: &str, data: JsonValue) {
        let Some(control) = &self.control else { return };
        control.dispatch_event(
            self.id,
            EventKind::NativeEvent,
            EventPayload::Native { name: name.into(), data },
        );
    }

    /// 组件自己的大小（point）：样式没定的那一维用它。
    pub fn set_size(&self, width: f32, height: f32) {
        let Some(control) = &self.control else { return };
        control.set_component_size(self.id, Size::new(Points(width), Points(height)));
    }
}

/// 按上下文给一个元素造组件。
pub type ComponentFactory = Rc<dyn Fn(ComponentContext) -> Box<dyn NativeComponent>>;

thread_local! {
    static FACTORIES: RefCell<FxHashMap<Arc<str>, ComponentFactory>> = RefCell::default();
    /// 报过没注册的名字，每个只报一次。
    static MISSING: RefCell<FxHashSet<Arc<str>>> = RefCell::default();
    /// 活着的组件，按容器；组件代码运行期间取出来（`None`），免得重入时借用冲突。
    static LIVE: RefCell<FxHashMap<NodeId, Option<Box<dyn NativeComponent>>>> = RefCell::default();
    /// 这个实例属于哪个播放器（[`set_player`]）。
    static PLAYER: Cell<u32> = const { Cell::new(0) };
}

/// 页面告诉这个实例它的播放器编号（根元素的 `data-nsh-player`）。
pub(crate) fn set_player(key: u32) {
    PLAYER.with(|p| p.set(key));
}

/// 把 `factory` 注册成 `name`（同名的替换掉）。`nsh/` 下的名字是 NSH 自己的，只记 warn。
pub fn register_component(name: &str, factory: ComponentFactory) {
    if nsh_core::platform::is_reserved_component(name) {
        log::warn!("native component `{name}` is not registered: names under `nsh/` are NSH's");
        return;
    }
    FACTORIES.with(|f| f.borrow_mut().insert(name.into(), factory));
    MISSING.with(|m| m.borrow_mut().remove(name));
}

/// 一批里和组件有关的事，DOM 建好以后按顺序做。
pub(crate) enum Op {
    Make(NodeId, Arc<str>),
    Props(NodeId, Vec<(Arc<str>, Option<JsonValue>)>),
    Command(NodeId, String, JsonValue),
    Delete(NodeId),
}

/// 挑出 `item` 里和组件有关的事。
pub(crate) fn collect(item: &MountItem, ops: &mut Vec<Op>) {
    match item {
        MountItem::Component { id, name } => ops.push(Op::Make(*id, name.clone())),
        MountItem::ComponentProps { id, props } => ops.push(Op::Props(*id, props.clone())),
        MountItem::Command { id, command: Command::Native { name, args } } => {
            ops.push(Op::Command(*id, name.to_string(), args.clone()))
        }
        MountItem::Delete { id } => ops.push(Op::Delete(*id)),
        _ => {}
    }
}

/// 按顺序做 [`collect`] 挑出的事。调用时不能借着 `APP`：组件可能发事件。
pub(crate) fn run(ops: Vec<Op>, control: &EngineControl) {
    apply(ops, Some(control));
}

fn apply(ops: Vec<Op>, control: Option<&EngineControl>) {
    for op in ops {
        match op {
            Op::Make(id, name) => make(id, &name, control),
            Op::Props(id, props) => with(id, |c| {
                for (name, value) in &props {
                    c.set_prop(name, value.as_ref());
                }
            }),
            Op::Command(id, name, args) => with(id, |c| c.command(&name, &args)),
            Op::Delete(id) => {
                let gone = LIVE.with(|l| l.borrow_mut().remove(&id));
                drop(gone);
            }
        }
    }
}

fn make(id: NodeId, name: &Arc<str>, control: Option<&EngineControl>) {
    let factory = FACTORIES.with(|f| f.borrow().get(name).cloned());
    let Some(factory) = factory else {
        if MISSING.with(|m| m.borrow_mut().insert(name.clone())) {
            log::warn!("no native component is registered as `{name}`: it renders an empty view");
        }
        return;
    };
    let player = PLAYER.with(Cell::get);
    let made = factory(ComponentContext { id, player, control: control.cloned() });
    let old = LIVE.with(|l| l.borrow_mut().insert(id, Some(made)));
    drop(old);
}

/// 取出组件跑 `f`，再放回去（期间被删掉的就不放回）。
fn with(id: NodeId, f: impl FnOnce(&mut dyn NativeComponent)) {
    let Some(mut component) = LIVE.with(|l| l.borrow_mut().get_mut(&id).and_then(Option::take)) else {
        return;
    };
    f(component.as_mut());
    LIVE.with(|l| {
        if let Some(slot) = l.borrow_mut().get_mut(&id) {
            *slot = Some(component);
        }
    });
}

/// 应用停了：组件全部释放。
pub(crate) fn clear() {
    let all = LIVE.with(|l| std::mem::take(&mut *l.borrow_mut()));
    drop(all);
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    thread_local! {
        /// 这个线程上记下的 warn 条数（测试并行跑，各数各的）。
        static WARNS: Cell<usize> = const { Cell::new(0) };
    }

    struct Count;

    impl log::Log for Count {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            if record.level() == log::Level::Warn {
                WARNS.with(|w| w.set(w.get() + 1));
            }
        }
        fn flush(&self) {}
    }

    fn warns() -> usize {
        let _ = log::set_logger(&Count);
        log::set_max_level(log::LevelFilter::Warn);
        WARNS.with(Cell::get)
    }

    type Log = Rc<RefCell<Vec<String>>>;

    /// 把收到的事记进 `log` 的组件。
    struct Spy {
        id: NodeId,
        log: Log,
    }

    impl NativeComponent for Spy {
        fn set_prop(&mut self, name: &str, value: Option<&JsonValue>) {
            self.log.borrow_mut().push(format!("{} prop {name}={}", self.id, value.map_or("-".into(), |v| v.to_string())));
        }
        fn command(&mut self, name: &str, _: &JsonValue) {
            self.log.borrow_mut().push(format!("{} command {name}", self.id));
        }
    }

    impl Drop for Spy {
        fn drop(&mut self) {
            self.log.borrow_mut().push(format!("{} drop", self.id));
        }
    }

    fn spy(name: &str) -> Log {
        let log = Log::default();
        let l = log.clone();
        register_component(
            name,
            Rc::new(move |ctx: ComponentContext| {
                l.borrow_mut().push(format!("{} make", ctx.id()));
                Box::new(Spy { id: ctx.id(), log: l.clone() })
            }),
        );
        log
    }

    fn id(n: NodeId) -> NodeId {
        n
    }

    fn prop(n: NodeId, name: &str, v: i32) -> Op {
        Op::Props(id(n), vec![(name.into(), Some(v.into()))])
    }

    #[test]
    fn create_props_command_and_delete_run_in_order() {
        let log = spy("test/order");
        apply(
            vec![
                Op::Make(id(1), "test/order".into()),
                prop(1, "a", 1),
                Op::Command(id(1), "go".into(), JsonValue::Null),
                prop(1, "a", 2),
                Op::Delete(id(1)),
                prop(1, "a", 3),
            ],
            None,
        );
        assert_eq!(*log.borrow(), ["1 make", "1 prop a=1", "1 command go", "1 prop a=2", "1 drop"], "删掉以后的属性不再送");
    }

    #[test]
    fn the_same_id_deleted_and_made_again_is_a_new_component() {
        let log = spy("test/again");
        apply(vec![Op::Make(id(2), "test/again".into()), prop(2, "a", 1)], None);
        apply(vec![Op::Delete(id(2)), Op::Make(id(2), "test/again".into()), prop(2, "a", 2)], None);
        assert_eq!(*log.borrow(), ["2 make", "2 prop a=1", "2 drop", "2 make", "2 prop a=2"]);
        apply(vec![Op::Make(id(2), "test/again".into())], None);
        assert_eq!(log.borrow()[5..], ["2 make", "2 drop"], "同一个编号再建一次：旧的放掉");
        clear();
    }

    #[test]
    fn an_unregistered_name_warns_once() {
        let before = warns();
        for _ in 0..3 {
            apply(vec![Op::Make(id(3), "test/missing".into()), prop(3, "a", 1)], None);
        }
        assert_eq!(warns() - before, 1);
        spy("test/missing");
        apply(vec![Op::Make(id(3), "test/missing".into())], None);
        assert_eq!(warns() - before, 1, "注册了就能建");
    }

    #[test]
    fn the_selector_is_scoped_to_the_player() {
        let ctx = ComponentContext { id: id(7), player: 3, control: None };
        assert_eq!(ctx.selector(), "[data-nsh-player=\"3\"] [data-nsh-id=\"7\"]");
        let ctx = ComponentContext { id: id(7), player: 0, control: None };
        assert_eq!(ctx.selector(), "[data-nsh-id=\"7\"]", "页面没告诉是哪个播放器时不加范围");
    }
}
