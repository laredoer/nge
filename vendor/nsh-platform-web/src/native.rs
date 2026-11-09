//! 原生组件（本仓库加的，nsh 0.4.0 的 web 平台没有）：照 macOS 平台的
//! `NativeComponent` / `ComponentContext` / `ComponentFactory`。
//!
//! - 生成的 web 运行时不调用应用的组件注册函数，所以用全局的
//!   [`register_component`]：应用在自己的 `#[native_module]` 里注册（打包时
//!   只有 TSX import 了这个模块，它的 gate 才会保留）。
//! - 组件的 DOM 由组件自己建：容器是 NSH 的 view 元素，带
//!   `data-nsh-id`（[`ComponentContext::selector`]）。
//! - 每批挂载项先交给 JS 建好 DOM，之后（[`crate::flush`] 放开 `APP` 以后）
//!   按顺序处理这一批的组件创建、属性、`command`、删除。

use std::{cell::RefCell, rc::Rc, sync::Arc};

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
    control: EngineControl,
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

    /// 找容器元素用的 CSS 选择器（`[data-nsh-id="…"]`）。
    pub fn selector(&self) -> String {
        format!("[data-nsh-id=\"{}\"]", self.id)
    }

    /// 发事件 `name`：TSX 的 `on<Name>` 收到 `data`。
    pub fn emit(&self, name: &str, data: JsonValue) {
        self.control.dispatch_event(
            self.id,
            EventKind::NativeEvent,
            EventPayload::Native { name: name.into(), data },
        );
    }

    /// 组件自己的大小（point）：样式没定的那一维用它。
    pub fn set_size(&self, width: f32, height: f32) {
        self.control.set_component_size(self.id, Size::new(Points(width), Points(height)));
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

fn make(id: NodeId, name: &Arc<str>, control: &EngineControl) {
    let factory = FACTORIES.with(|f| f.borrow().get(name).cloned());
    let Some(factory) = factory else {
        if MISSING.with(|m| m.borrow_mut().insert(name.clone())) {
            log::warn!("no native component is registered as `{name}`: it renders an empty view");
        }
        return;
    };
    let made = factory(ComponentContext { id, control: control.clone() });
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
