//! `cargo run -p nge-relay -- --dev`（本机开发，不鉴权）
//! `NGE_AUTH_SECRET=… cargo run -p nge-relay`（要鉴权：token 用 `nge-token` 签）
//! 其他参数：`--addr 127.0.0.1:9100 --room-size 8 --max-frame 8192 --rate 60 --max-conns 1024`

use nge_net::auth::{SECRET_ENV, Secret};
use nge_relay::Config;

const USAGE: &str = "用法: nge-relay (--dev | 设环境变量 NGE_AUTH_SECRET) [--addr 127.0.0.1:9100] [--room-size 8] [--max-frame 8192] [--rate 60] [--max-conns 1024]";

fn main() {
    let mut addr = "127.0.0.1:9100".to_owned();
    let mut config = Config::default();
    let mut dev = false;
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--dev" {
            dev = true;
            continue;
        }
        let value = args.next();
        let ok = match (flag.as_str(), value.as_deref()) {
            ("--addr", Some(v)) => {
                addr = v.to_owned();
                true
            }
            ("--room-size", Some(v)) => v.parse().map(|n: usize| config.room_size = n.clamp(1, 256)).is_ok(),
            ("--max-frame", Some(v)) => v.parse().map(|n: usize| config.max_frame = n.clamp(64, 60 * 1024)).is_ok(),
            ("--rate", Some(v)) => v.parse().map(|r: f64| (config.rate, config.burst) = (r.max(1.0), r.max(1.0) * 2.0)).is_ok(),
            ("--max-conns", Some(v)) => v.parse().map(|n: usize| config.max_conns = n.clamp(1, 1 << 20)).is_ok(),
            _ => false,
        };
        if !ok {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }
    config.auth = match nge_relay::startup::choose_auth(Secret::from_env(SECRET_ENV), dev) {
        Ok(auth) => auth,
        Err(why) => fail(&format!("{why}\n{USAGE}")),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("起不了运行时: {e}");
            std::process::exit(1);
        }
    };
    let result = runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        let mode = if config.auth.is_some() { "要鉴权" } else { "开发模式，不鉴权" };
        println!("中继在 ws://{} 上听着（{mode}，房间最多 {} 人，排行榜 http://{}/scores）", listener.local_addr()?, config.room_size, listener.local_addr()?);
        nge_relay::serve(listener, config).await
    });
    if let Err(e) = result {
        eprintln!("中继停了: {e}");
        std::process::exit(1);
    }
}

fn fail(why: &str) -> ! {
    eprintln!("{why}");
    std::process::exit(2);
}
