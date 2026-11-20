//! 两个命令行程序启动时的选择（纯函数，方便测）：鉴权开不开、`nge-token` 的参数。

use nge_net::auth::{AuthError, MAX_TTL, SECRET_ENV, Secret};

/// 鉴权要显式选：有密钥就鉴权，`--dev` 才不鉴权；两个都没有、都有、密钥不能用都不起（`Err` 是给人看的原因）。
pub fn choose_auth(secret: Result<Option<Secret>, AuthError>, dev: bool) -> Result<Option<Secret>, String> {
    match (secret, dev) {
        (Err(e), _) => Err(format!("{SECRET_ENV} 不能用: {e}")),
        (Ok(Some(_)), true) => Err(format!("{SECRET_ENV} 和 --dev 只能二选一")),
        (Ok(Some(secret)), false) => Ok(Some(secret)),
        (Ok(None), true) => Ok(None),
        (Ok(None), false) => Err(format!("没有鉴权密钥：设环境变量 {SECRET_ENV}（至少 16 字节），或者本机开发加 --dev 不鉴权")),
    }
}

/// `nge-token` 的参数：账号、名字、有效期（秒，默认一小时）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenArgs {
    pub sub: String,
    pub name: String,
    pub ttl: u64,
}

/// 解 `--sub 账号 --name 名字 [--ttl 秒]`；有效期夹在 1 秒到 `MAX_TTL` 之间。
pub fn token_args(args: impl IntoIterator<Item = String>) -> Option<TokenArgs> {
    let (mut sub, mut name, mut ttl) = (None, None, 3600u64);
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args.next()?;
        let ok = match flag.as_str() {
            "--sub" => sub.replace(value).is_none(),
            "--name" => name.replace(value).is_none(),
            "--ttl" => value.parse().map(|t: u64| ttl = t.clamp(1, MAX_TTL)).is_ok(),
            _ => false,
        };
        if !ok {
            return None;
        }
    }
    Some(TokenArgs { sub: sub?, name: name?, ttl })
}

/// 中继启动时的时钟和随机数种子：`(现在的 Unix 秒, 握手随机数的种子)`。
/// 种子混进系统随机数，重启以后（哪怕时钟一样）随机数序列也不一样；时钟早于 1970 年或者拿不到系统随机数时不起。
pub fn boot(since_epoch: Result<std::time::Duration, std::time::SystemTimeError>, os_random: Result<[u8; 8], String>) -> Result<(u64, u64), String> {
    let since = since_epoch.map_err(|_| "系统时钟早于 1970 年：判不了 token 过期，不起".to_owned())?;
    let random = os_random.map_err(|e| format!("拿不到系统随机数（{e}），不起"))?;
    Ok((since.as_secs(), since.as_nanos() as u64 ^ u64::from_le_bytes(random)))
}

/// 从系统取 8 个字节的随机数。
pub fn os_random() -> Result<[u8; 8], String> {
    let mut buf = [0; 8];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> Secret {
        Secret::new(b"startup-test-secret-0123456789".to_vec()).unwrap()
    }

    #[test]
    fn auth_must_be_chosen_explicitly() {
        assert!(choose_auth(Ok(Some(secret())), false).unwrap().is_some(), "有密钥：鉴权");
        assert!(choose_auth(Ok(None), true).unwrap().is_none(), "--dev：不鉴权");
        assert!(choose_auth(Ok(None), false).unwrap_err().contains(SECRET_ENV), "两个都没有：不起");
        assert!(choose_auth(Ok(Some(secret())), true).unwrap_err().contains("二选一"), "两个都有：不起");
        for dev in [false, true] {
            assert!(choose_auth(Err(AuthError::WeakSecret), dev).is_err(), "密钥太短，--dev={dev}");
            assert!(choose_auth(Err(AuthError::NotUnicode), dev).unwrap_err().contains("UTF-8"), "密钥不是 UTF-8，--dev={dev}");
        }
    }

    fn args(s: &str) -> Option<TokenArgs> {
        token_args(s.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn token_arguments() {
        assert_eq!(args("--sub a --name 小红"), Some(TokenArgs { sub: "a".into(), name: "小红".into(), ttl: 3600 }));
        assert_eq!(args("--name n --sub a --ttl 60").map(|a| a.ttl), Some(60));
        assert_eq!(args("--sub a --name n --ttl 0").map(|a| a.ttl), Some(1), "至少 1 秒");
        assert_eq!(args(&format!("--sub a --name n --ttl {}", MAX_TTL * 10)).map(|a| a.ttl), Some(MAX_TTL), "不超过上限");
        for bad in ["", "--sub a", "--name n", "--sub a --name n --ttl x", "--sub a --name n --ttl", "--sub a --sub b --name n", "--sub a --name n --what 1"] {
            assert_eq!(args(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_nonce_seed_mixes_in_os_randomness_and_needs_a_sane_clock() {
        use std::time::{Duration, UNIX_EPOCH};
        let clock = || Ok(Duration::new(1_800_000_000, 5));
        let (unix, a) = boot(clock(), Ok([1; 8])).unwrap();
        assert_eq!(unix, 1_800_000_000);
        let (_, b) = boot(clock(), Ok([2; 8])).unwrap();
        assert_ne!(a, b, "时钟一样，系统随机数不一样：种子不一样");
        let before_1970 = UNIX_EPOCH.duration_since(UNIX_EPOCH + Duration::from_secs(1));
        assert!(boot(before_1970, Ok([1; 8])).unwrap_err().contains("1970"));
        assert!(boot(clock(), Err("没有".into())).is_err(), "拿不到系统随机数：不起");
    }
}
