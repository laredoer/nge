//! 游戏逻辑（ECS）：一堆精灵在视口内反弹、自转；点击处生成一批。

use nge_ecs::prelude::*;
use nge_render2d::prelude::*;
use nge_render2d::AtlasId;

/// 环境变量：启动时的精灵数量。
pub const COUNT_ENV: &str = "NGE_SPRITES";
const DEFAULT_COUNT: u32 = 10_000;
const SPRITE_SIZE: f32 = 24.0;
const TAP_BURST: u32 = 100;

#[derive(Component)]
struct Velocity {
    linear: Vec2,
    angular: f32,
}

#[derive(Resource)]
struct Ships(AtlasId);

/// xorshift32，够压测用，不引依赖。
#[derive(Resource)]
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next()
    }
}

pub fn app() -> App {
    let mut app = App::new();
    app.add_plugins(Render2dPlugin)
        .insert_resource(Rng(0x9E37_79B9))
        .add_systems(Startup, setup)
        .add_systems(Update, (move_sprites, on_pointer, on_command, report).chain());
    app
}

fn setup(mut commands: Commands, mut assets: ResMut<Assets>, mut rng: ResMut<Rng>, screen: Res<Screen>) {
    let texture = assets.load_png(include_bytes!("../assets/ships.png")).expect("内置图集");
    let atlas = assets.atlas(texture, Grid { cols: 4, rows: 4 });
    commands.spawn(Camera2d::default());
    let count = std::env::var(COUNT_ENV).ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_COUNT);
    let half = screen.size / 2.0;
    spawn(&mut commands, &mut rng, atlas, count, |rng| Vec2::new(rng.range(-half.x, half.x), rng.range(-half.y, half.y)));
    commands.insert_resource(Ships(atlas));
}

fn spawn(commands: &mut Commands, rng: &mut Rng, atlas: AtlasId, n: u32, mut at: impl FnMut(&mut Rng) -> Vec2) {
    let batch: Vec<_> = (0..n)
        .map(|_| {
            let pos = at(rng);
            let index = (rng.next() * 16.0) as u32;
            let speed = rng.range(40.0, 220.0);
            let dir = rng.range(0.0, std::f32::consts::TAU);
            (
                Sprite::from_atlas(atlas, index).with_size(Vec2::splat(SPRITE_SIZE)),
                Transform2d { translation: pos, rotation: dir, z: index as f32, ..Default::default() },
                Velocity { linear: Vec2::from_angle(dir) * speed, angular: rng.range(-3.0, 3.0) },
            )
        })
        .collect();
    commands.spawn_batch(batch);
}

fn move_sprites(time: Res<Time>, screen: Res<Screen>, mut sprites: Query<(&mut Transform2d, &mut Velocity)>) {
    let half = screen.size / 2.0;
    let dt = time.delta;
    for (mut t, mut v) in &mut sprites {
        t.translation += v.linear * dt;
        t.rotation += v.angular * dt;
        for axis in 0..2 {
            if t.translation[axis].abs() > half[axis] {
                t.translation[axis] = t.translation[axis].clamp(-half[axis], half[axis]);
                v.linear[axis] = -v.linear[axis];
            }
        }
    }
}

fn on_pointer(mut commands: Commands, input: Res<Input>, ships: Res<Ships>, mut rng: ResMut<Rng>) {
    for p in input.pointers_pressed() {
        let at = p.world;
        spawn(&mut commands, &mut rng, ships.0, TAP_BURST, |_| at);
    }
}

fn on_command(
    mut commands: Commands,
    mut reader: MessageReader<NshCommand>,
    sprites: Query<Entity, With<Sprite>>,
    ships: Res<Ships>,
    mut rng: ResMut<Rng>,
    screen: Res<Screen>,
) {
    let half = screen.size / 2.0;
    for cmd in reader.read() {
        let n = cmd.args["n"].as_u64().unwrap_or(1000) as u32;
        match cmd.name.as_str() {
            "add" => spawn(&mut commands, &mut rng, ships.0, n, |rng| {
                Vec2::new(rng.range(-half.x, half.x), rng.range(-half.y, half.y))
            }),
            "remove" => {
                for e in sprites.iter().take(n as usize) {
                    commands.entity(e).despawn();
                }
            }
            other => log::warn!("未知 command {other}"),
        }
    }
}

/// 数量变了就告诉 NSH。
fn report(sprites: Query<(), With<Sprite>>, mut outbox: ResMut<Outbox>, mut last: Local<Option<usize>>) {
    let count = sprites.iter().len();
    if *last != Some(count) {
        *last = Some(count);
        outbox.emit("count", json!({ "count": count }));
    }
}
