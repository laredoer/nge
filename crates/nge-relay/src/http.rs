//! 中继端口上的一点 HTTP：排行榜（`GET /scores?board=…`、`POST /scores`）。只认最简单的 HTTP/1.1：一个请求一个连接，带 `Content-Length` 的体。

use serde::Deserialize;
use serde_json::json;

use crate::scores::Leaderboard;

/// 请求头最多几个字节。
pub const MAX_HEAD: usize = 8 * 1024;
/// 请求体最多几个字节。
pub const MAX_BODY: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub content_length: usize,
    /// `Upgrade: websocket`。
    pub websocket: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// 请求头结束的位置（`\r\n\r\n` 之后）；还没收全时是 `None`。
pub fn head_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// 解析请求头。
pub fn parse_head(bytes: &[u8]) -> Result<Head, &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| "请求头不是 UTF-8")?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next().ok_or("没有请求行")?.split(' ');
    let (Some(method), Some(target), Some(version)) = (first.next(), first.next(), first.next()) else { return Err("请求行不对") };
    if !version.starts_with("HTTP/1.") {
        return Err("只支持 HTTP/1.x");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut head = Head { method: method.to_owned(), path: path.to_owned(), query: parse_query(query), content_length: 0, websocket: false };
    for line in lines.take_while(|l| !l.is_empty()) {
        let Some((name, value)) = line.split_once(':') else { return Err("请求头格式不对") };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        match name.as_str() {
            "content-length" => head.content_length = value.parse().map_err(|_| "Content-Length 不是数")?,
            "upgrade" => head.websocket = value.eq_ignore_ascii_case("websocket"),
            "transfer-encoding" => return Err("不支持分块传输"),
            _ => {}
        }
    }
    Ok(head)
}

fn parse_query(query: &str) -> Vec<(String, String)> {
    query.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').unwrap_or((p, ""))).map(|(k, v)| (decode(k), decode(v))).collect()
}

/// 百分号解码，`+` 是空格；解出来不是 UTF-8 的字节换成 �。
fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(hi << 4 | lo);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[derive(Deserialize)]
struct Submit {
    board: String,
    name: String,
    score: f64,
}

/// 处理一个请求。
pub fn route(head: &Head, body: &[u8], board: &mut Leaderboard) -> Response {
    let ok = |body: serde_json::Value| Response { status: 200, body: body.to_string() };
    let bad = |status: u16, why: &str| Response { status, body: json!({ "error": why }).to_string() };
    match (head.method.as_str(), head.path.as_str()) {
        ("OPTIONS", _) => Response { status: 204, body: String::new() },
        ("GET", "/scores") => match head.query.iter().find(|(k, _)| k == "board") {
            Some((_, name)) => ok(json!({ "board": name, "top": board.top(name) })),
            None => bad(400, "缺 board"),
        },
        ("POST", "/scores") => match serde_json::from_slice::<Submit>(body) {
            Ok(s) => match board.submit(&s.board, &s.name, s.score) {
                Ok(rank) => ok(json!({ "rank": rank })),
                Err(why) => bad(400, why),
            },
            Err(_) => bad(400, "请求体要是 {board, name, score}"),
        },
        (_, "/scores") => bad(405, "只支持 GET、POST"),
        _ => bad(404, "没有这个地址"),
    }
}

/// 写成 HTTP 响应（带 CORS，浏览器里也能调）。
pub fn render(response: &Response) -> Vec<u8> {
    let reason = match response.status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    format!(
        "HTTP/1.1 {} {reason}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n{}",
        response.status,
        response.body.len(),
        response.body
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(raw: &str) -> Head {
        parse_head(raw.as_bytes()).unwrap()
    }

    #[test]
    fn parses_the_request_line_query_and_headers() {
        let raw = "GET /scores?board=platformer-1&x=a%20b+c HTTP/1.1\r\nHost: x\r\nUpgrade: WebSocket\r\nContent-Length: 12\r\n\r\n";
        assert_eq!(head_end(raw.as_bytes()), Some(raw.len()));
        assert_eq!(head_end(b"GET / HTTP/1.1\r\nHost"), None);
        let h = head(raw);
        assert_eq!((h.method.as_str(), h.path.as_str(), h.content_length, h.websocket), ("GET", "/scores", 12, true));
        assert_eq!(h.query, vec![("board".into(), "platformer-1".into()), ("x".into(), "a b c".into())]);
        assert_eq!(decode("%E5%85%B3%2"), "关%2", "不完整的转义原样留着");
        assert_eq!(decode("%关"), "%关", "% 后面是多字节字符也不崩");
    }

    #[test]
    fn rejects_what_it_does_not_speak() {
        assert!(parse_head(b"GET /\r\n\r\n").is_err());
        assert!(parse_head(b"GET / SPDY/3\r\n\r\n").is_err());
        assert!(parse_head(b"POST / HTTP/1.1\r\nContent-Length: lots\r\n\r\n").is_err());
        assert!(parse_head(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n").is_err());
    }

    #[test]
    fn scores_can_be_submitted_and_read() {
        let mut board = Leaderboard::default();
        let post = head("POST /scores HTTP/1.1\r\n\r\n");
        let r = route(&post, br#"{"board":"l1","name":"a","score":9.5}"#, &mut board);
        assert_eq!(r, Response { status: 200, body: r#"{"rank":1}"#.into() });
        let r = route(&head("GET /scores?board=l1 HTTP/1.1\r\n\r\n"), b"", &mut board);
        assert_eq!(r.status, 200);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&r.body).unwrap(), json!({ "board": "l1", "top": [{ "name": "a", "score": 9.5 }] }));
        assert_eq!(route(&post, b"{", &mut board).status, 400);
        assert_eq!(route(&post, br#"{"board":"l1","name":"a","score":-1}"#, &mut board).status, 400);
        assert_eq!(route(&head("GET /scores HTTP/1.1\r\n\r\n"), b"", &mut board).status, 400);
        assert_eq!(route(&head("GET /nope HTTP/1.1\r\n\r\n"), b"", &mut board).status, 404);
        assert_eq!(route(&head("DELETE /scores HTTP/1.1\r\n\r\n"), b"", &mut board).status, 405);
        assert_eq!(route(&head("OPTIONS /scores HTTP/1.1\r\n\r\n"), b"", &mut board).status, 204);
    }

    #[test]
    fn responses_carry_length_and_cors() {
        let text = String::from_utf8(render(&Response { status: 200, body: "{}".into() })).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 2\r\n") && text.contains("Access-Control-Allow-Origin: *"));
        assert!(text.ends_with("\r\n\r\n{}"));
    }
}
