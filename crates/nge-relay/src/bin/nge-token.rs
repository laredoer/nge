//! 签一张 token（凭据）：`NGE_AUTH_SECRET=… cargo run -p nge-relay --bin nge-token -- --sub 账号 --name 名字 [--ttl 秒]`
//! 打印的整串交给玩家（场景的 `net.token`，原生也可以放环境变量 `NGE_TOKEN`）。没有账号系统：谁拿什么账号由调用的人决定。

use nge_net::auth::{Claims, MAX_TTL, SECRET_ENV, Secret};

const USAGE: &str = "用法: NGE_AUTH_SECRET=… nge-token --sub 账号 --name 名字 [--ttl 3600]";

fn main() {
    let (mut sub, mut name, mut ttl) = (None, None, 3600u64);
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args.next();
        let ok = match (flag.as_str(), value) {
            ("--sub", Some(v)) => sub.replace(v).is_none(),
            ("--name", Some(v)) => name.replace(v).is_none(),
            ("--ttl", Some(v)) => v.parse().map(|t: u64| ttl = t.clamp(1, MAX_TTL)).is_ok(),
            _ => false,
        };
        if !ok {
            fail(USAGE);
        }
    }
    let (Some(sub), Some(name)) = (sub, name) else { fail(USAGE) };
    let secret = match Secret::from_env(SECRET_ENV) {
        Ok(Some(s)) => s,
        Ok(None) => fail(&format!("没设 {SECRET_ENV}\n{USAGE}")),
        Err(e) => fail(&format!("{SECRET_ENV} 不能用: {e}")),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    match secret.issue(&Claims { sub, name, exp: now + ttl }) {
        Ok(token) => println!("{token}"),
        Err(e) => fail(&format!("签不了: {e}")),
    }
}

fn fail(why: &str) -> ! {
    eprintln!("{why}");
    std::process::exit(2);
}
