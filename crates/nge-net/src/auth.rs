//! 鉴权：服务端用密钥给账号签 token（HMAC-SHA256），客户端连上时拿 token 和一个「证明」换进房间。
//!
//! - 凭据（发给玩家的那一串）是 `v1.<声明>.<签名>.<会话密钥>`；线上只发前三段（token），会话密钥不出客户端。
//! - 声明是 `{ sub, name, exp }`：账号、显示的名字、过期时间（Unix 秒）。签名 = HMAC(密钥, 声明)。
//! - 会话密钥 = HMAC(密钥, 声明)（换一个前缀），服务端随时能从 token 算回来，不用存。
//! - 每条连接服务端先发一个不重复的随机数（`Challenge`），客户端回 `Hello` 时带上 HMAC(会话密钥, 随机数 + 房间名)。
//!   抓到一次握手原样重发没用：下一条连接的随机数不一样；只抓到 token 也没用：没有会话密钥算不出证明。
//!
//! 密钥只从环境变量或配置来，代码里没有默认的；测试用测试自己的密钥。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::proto::{MAX_NAME, clean_text};

type HmacSha256 = Hmac<Sha256>;

/// 密钥至少几个字节。
pub const MIN_SECRET: usize = 16;
/// 线上的 token 最多几个字节。
pub const MAX_TOKEN: usize = 512;
/// 账号最多几个字符。
pub const MAX_SUB: usize = 64;
/// token 最长能管多久（秒）：签得再长也按不合规拒掉。
pub const MAX_TTL: u64 = 30 * 24 * 3600;
/// 握手随机数的长度。
pub const NONCE: usize = 16;
/// 证明的长度。
pub const PROOF: usize = 32;
/// 读密钥的环境变量。
pub const SECRET_ENV: &str = "NGE_AUTH_SECRET";

const VERSION: &str = "v1";
const TAG_SIGN: &[u8] = b"nge-token-v1";
const TAG_KEY: &[u8] = b"nge-key-v1";
const TAG_HELLO: &[u8] = b"nge-hello-v1";
const TAG_NONCE: &[u8] = b"nge-nonce-v1";

/// token 里的声明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// 账号：排行榜的成绩、名字归它。
    pub sub: String,
    /// 显示的名字（房间里、排行榜上）。
    pub name: String,
    /// 过期时间，Unix 秒。
    pub exp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// 密钥太短。
    WeakSecret,
    /// 格式不对：段数、base64、长度。
    Malformed,
    /// 签名对不上（伪造的、改过的、别的密钥签的）。
    BadSignature,
    Expired,
    /// 账号、名字不合规，或者有效期长得离谱。
    BadClaims,
    /// 握手的证明对不上（重放的、房间不对、没有会话密钥）。
    BadProof,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let why = match self {
            Self::WeakSecret => "密钥太短",
            Self::Malformed => "token 格式不对",
            Self::BadSignature => "token 签名不对",
            Self::Expired => "token 过期了",
            Self::BadClaims => "token 的内容不合规",
            Self::BadProof => "握手证明不对",
        };
        f.write_str(why)
    }
}

impl std::error::Error for AuthError {}

/// 服务端的密钥。`Debug` 不打印内容。
#[derive(Clone)]
pub struct Secret(Vec<u8>);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret({} 字节)", self.0.len())
    }
}

impl Secret {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Result<Self, AuthError> {
        let bytes = bytes.into();
        if bytes.len() < MIN_SECRET {
            return Err(AuthError::WeakSecret);
        }
        Ok(Self(bytes))
    }

    /// 从环境变量读：没设是 `Ok(None)`，设了但太短是错。
    pub fn from_env(var: &str) -> Result<Option<Self>, AuthError> {
        match std::env::var(var) {
            Ok(v) => Self::new(v.into_bytes()).map(Some),
            Err(_) => Ok(None),
        }
    }

    /// 第 `counter` 条连接的握手随机数：`seed` 每次启动不一样（启动时刻），同一次启动里 `counter` 不重复。
    pub fn nonce(&self, seed: u64, counter: u64) -> [u8; NONCE] {
        let full = mac(&self.0, &[TAG_NONCE, &seed.to_le_bytes(), &counter.to_le_bytes()]).finalize().into_bytes();
        let mut out = [0; NONCE];
        out.copy_from_slice(&full[..NONCE]);
        out
    }

    /// 签一张凭据（给玩家的整串，含会话密钥）。
    pub fn issue(&self, claims: &Claims) -> Result<String, AuthError> {
        check_claims(claims)?;
        let body = postcard::to_stdvec(claims).map_err(|_| AuthError::BadClaims)?;
        let sig = mac(&self.0, &[TAG_SIGN, &body]).finalize().into_bytes();
        let key = self.session_key(&body);
        Ok(format!("{VERSION}.{}.{}.{}", B64.encode(&body), B64.encode(sig), B64.encode(key)))
    }

    /// 校验握手：token 的签名、有效期，以及对这条连接的随机数和房间名的证明。`now` 是 Unix 秒。
    pub fn verify(&self, token: &str, nonce: &[u8; NONCE], room: &str, proof: &[u8], now: u64) -> Result<Claims, AuthError> {
        if token.len() > MAX_TOKEN {
            return Err(AuthError::Malformed);
        }
        let mut parts = token.split('.');
        let (Some(VERSION), Some(body), Some(sig), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
            return Err(AuthError::Malformed);
        };
        let body = B64.decode(body).map_err(|_| AuthError::Malformed)?;
        let sig = B64.decode(sig).map_err(|_| AuthError::Malformed)?;
        // 先验签名再解：不是自己签的东西不去碰
        mac(&self.0, &[TAG_SIGN, &body]).verify_slice(&sig).map_err(|_| AuthError::BadSignature)?;
        let claims: Claims = postcard::from_bytes(&body).map_err(|_| AuthError::Malformed)?;
        check_claims(&claims)?;
        if now >= claims.exp {
            return Err(AuthError::Expired);
        }
        if claims.exp - now > MAX_TTL {
            return Err(AuthError::BadClaims);
        }
        let key = self.session_key(&body);
        mac(&key, &[TAG_HELLO, nonce, room.as_bytes()]).verify_slice(proof).map_err(|_| AuthError::BadProof)?;
        Ok(claims)
    }

    fn session_key(&self, body: &[u8]) -> [u8; 32] {
        mac(&self.0, &[TAG_KEY, body]).finalize().into_bytes().into()
    }
}

/// 客户端手里的凭据：线上发的 token 和只在本地用的会话密钥。`Debug` 不打印密钥。
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    pub token: String,
    key: [u8; 32],
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credential({})", self.token)
    }
}

impl Credential {
    /// 解析 `issue` 给的整串；格式不对是 `None`。
    pub fn parse(s: &str) -> Option<Self> {
        let (token, key) = s.trim().rsplit_once('.')?;
        let key: [u8; 32] = B64.decode(key).ok()?.try_into().ok()?;
        (token.len() <= MAX_TOKEN && token.split('.').count() == 3 && token.starts_with("v1.")).then(|| Self { token: token.to_owned(), key })
    }

    /// 对这条连接的随机数和房间名的证明。
    pub fn prove(&self, nonce: &[u8; NONCE], room: &str) -> Vec<u8> {
        mac(&self.key, &[TAG_HELLO, nonce, room.as_bytes()]).finalize().into_bytes().to_vec()
    }
}

fn check_claims(c: &Claims) -> Result<(), AuthError> {
    let ok = clean_text(&c.sub, MAX_SUB).is_some_and(|s| s == c.sub) && clean_text(&c.name, MAX_NAME).is_some_and(|n| n == c.name);
    if ok { Ok(()) } else { Err(AuthError::BadClaims) }
}

fn mac(key: &[u8], parts: &[&[u8]]) -> HmacSha256 {
    // HMAC 收任意长度的 key，不会失败
    let mut m = <HmacSha256 as Mac>::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    for p in parts {
        m.update(p);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const NONCE_A: [u8; NONCE] = [7; NONCE];
    const NONCE_B: [u8; NONCE] = [8; NONCE];

    fn secret() -> Secret {
        Secret::new(b"test-secret-only-for-unit-tests".to_vec()).unwrap()
    }

    fn claims() -> Claims {
        Claims { sub: "acct-1".into(), name: "小红".into(), exp: NOW + 3600 }
    }

    fn hello(cred: &Credential, nonce: &[u8; NONCE], room: &str) -> (String, Vec<u8>) {
        (cred.token.clone(), cred.prove(nonce, room))
    }

    #[test]
    fn an_issued_credential_passes_the_handshake() {
        let s = secret();
        let cred = Credential::parse(&s.issue(&claims()).unwrap()).unwrap();
        let (token, proof) = hello(&cred, &NONCE_A, "arena");
        assert_eq!(s.verify(&token, &NONCE_A, "arena", &proof, NOW), Ok(claims()));
        assert!(!format!("{cred:?}").contains(&B64.encode(cred.key)), "Debug 不打印会话密钥");
        assert!(!format!("{s:?}").contains("test-secret"), "Debug 不打印密钥");
    }

    #[test]
    fn short_secrets_are_refused() {
        assert_eq!(Secret::new(b"short".to_vec()).err(), Some(AuthError::WeakSecret));
        assert!(Secret::new(vec![1; MIN_SECRET]).is_ok());
    }

    #[test]
    fn expired_tokens_are_refused() {
        let s = secret();
        let cred = Credential::parse(&s.issue(&claims()).unwrap()).unwrap();
        let (token, proof) = hello(&cred, &NONCE_A, "r");
        assert_eq!(s.verify(&token, &NONCE_A, "r", &proof, NOW + 3600), Err(AuthError::Expired), "到点就过期");
        assert!(s.verify(&token, &NONCE_A, "r", &proof, NOW + 3599).is_ok());
        let forever = Claims { exp: NOW + MAX_TTL + 10, ..claims() };
        let cred = Credential::parse(&s.issue(&forever).unwrap()).unwrap();
        let (token, proof) = hello(&cred, &NONCE_A, "r");
        assert_eq!(s.verify(&token, &NONCE_A, "r", &proof, NOW), Err(AuthError::BadClaims), "有效期长得离谱");
    }

    #[test]
    fn forged_and_tampered_tokens_are_refused() {
        let s = secret();
        let other = Secret::new(b"another-secret-for-unit-tests!!".to_vec()).unwrap();
        // 别的密钥签的
        let cred = Credential::parse(&other.issue(&claims()).unwrap()).unwrap();
        let (token, proof) = hello(&cred, &NONCE_A, "r");
        assert_eq!(s.verify(&token, &NONCE_A, "r", &proof, NOW), Err(AuthError::BadSignature));
        // 改了声明（换名字、延长有效期），签名还是原来的
        let cred = Credential::parse(&s.issue(&claims()).unwrap()).unwrap();
        let sig = cred.token.split('.').nth(2).unwrap();
        for forged in [Claims { name: "管理员".into(), ..claims() }, Claims { exp: NOW + 7200, ..claims() }] {
            let body = B64.encode(postcard::to_stdvec(&forged).unwrap());
            let token = format!("v1.{body}.{sig}");
            assert_eq!(s.verify(&token, &NONCE_A, "r", &cred.prove(&NONCE_A, "r"), NOW), Err(AuthError::BadSignature), "{forged:?}");
        }
        // 格式不对
        for bad in ["", "v1", "v1.a.b.c", "v2.a.b", "v1.!!.??", &"v1.".repeat(200)] {
            assert_eq!(s.verify(bad, &NONCE_A, "r", &[0; PROOF], NOW), Err(AuthError::Malformed), "{bad}");
        }
    }

    #[test]
    fn replayed_handshakes_are_refused() {
        let s = secret();
        let cred = Credential::parse(&s.issue(&claims()).unwrap()).unwrap();
        // 抓到的一次握手（token + 证明），拿到下一条连接（随机数不同）上重发
        let (token, proof) = hello(&cred, &NONCE_A, "r");
        assert_eq!(s.verify(&token, &NONCE_B, "r", &proof, NOW), Err(AuthError::BadProof));
        // 换个房间也不行
        assert_eq!(s.verify(&token, &NONCE_A, "other", &proof, NOW), Err(AuthError::BadProof));
        // 只有 token、没有会话密钥：瞎猜的证明
        assert_eq!(s.verify(&token, &NONCE_A, "r", &[0; PROOF], NOW), Err(AuthError::BadProof));
        assert_eq!(s.verify(&token, &NONCE_A, "r", &[], NOW), Err(AuthError::BadProof));
    }

    #[test]
    fn nonces_differ_per_connection_and_per_start() {
        let s = secret();
        assert_ne!(s.nonce(1, 1), s.nonce(1, 2));
        assert_ne!(s.nonce(1, 1), s.nonce(2, 1));
        assert_eq!(s.nonce(1, 1), s.nonce(1, 1));
    }

    #[test]
    fn bad_claims_are_not_signed_and_bad_credentials_do_not_parse() {
        let s = secret();
        for bad in [Claims { sub: "".into(), ..claims() }, Claims { name: "a\nb".into(), ..claims() }, Claims { name: "n".repeat(MAX_NAME + 1), ..claims() }, Claims { sub: " x".into(), ..claims() }] {
            assert_eq!(s.issue(&bad), Err(AuthError::BadClaims), "{bad:?}");
        }
        let good = s.issue(&claims()).unwrap();
        assert!(Credential::parse(&good).is_some());
        assert!(Credential::parse(good.rsplit_once('.').unwrap().0).is_none(), "少了会话密钥");
        assert!(Credential::parse("v1.a.b.c").is_none());
        assert!(Credential::parse("").is_none());
    }
}
