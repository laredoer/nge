//! 签一张 token（凭据）：`NGE_AUTH_SECRET=… cargo run -p nge-relay --bin nge-token -- --sub 账号 --name 名字 [--ttl 秒]`
//! 打印的整串交给玩家（场景的 `net.token`，原生也可以放环境变量 `NGE_TOKEN`）。没有账号系统：谁拿什么账号由调用的人决定。

use nge_net::auth::{Claims, SECRET_ENV, Secret};
use nge_relay::startup::{TokenArgs, token_args};

const USAGE: &str = "用法: NGE_AUTH_SECRET=… nge-token --sub 账号 --name 名字 [--ttl 3600]";

fn main() {
    let Some(TokenArgs { sub, name, ttl }) = token_args(std::env::args().skip(1)) else { fail(USAGE) };
    let secret = match Secret::from_env(SECRET_ENV) {
        Ok(Some(s)) => s,
        Ok(None) => fail(&format!("没设 {SECRET_ENV}\n{USAGE}")),
        Err(e) => fail(&format!("{SECRET_ENV} 不能用: {e}")),
    };
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()) else { fail("系统时钟早于 1970 年") };
    match secret.issue(&Claims { sub, name, iat: now, exp: now + ttl }) {
        Ok(token) => println!("{token}"),
        Err(e) => fail(&format!("签不了: {e}")),
    }
}

fn fail(why: &str) -> ! {
    eprintln!("{why}");
    std::process::exit(2);
}
