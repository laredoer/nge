//! 测试里起中继（随机端口），用真的原生客户端连上去。

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use nge_net::proto::{Item, Payload, Snapshot};
use nge_net::{ClientEvent, NetClient, Peer, Status, Target};
use nge_relay::Config;

fn relay() -> String {
    let addr = nge_relay::spawn("127.0.0.1:0", Config::default()).unwrap();
    addr.to_string()
}

fn client(addr: &str, room: &str, name: &str) -> NetClient {
    let mut c = NetClient::new(Box::new(nge_net::link::connect));
    c.set_target(Some(Target { url: format!("ws://{addr}"), room: room.into(), name: name.into() }));
    c
}

/// 让几个客户端一起跑，直到 `done` 说够了（最多 5 秒），返回每个收到的事件。
fn run(clients: &mut [&mut NetClient], start: Instant, mut done: impl FnMut(&[Vec<ClientEvent>]) -> bool) -> Vec<Vec<ClientEvent>> {
    let mut events = vec![Vec::new(); clients.len()];
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let now = start.elapsed().as_secs_f64();
        for (c, e) in clients.iter_mut().zip(events.iter_mut()) {
            e.extend(c.update(now));
        }
        if done(&events) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    events
}

fn online(events: &[ClientEvent]) -> bool {
    events.contains(&ClientEvent::Status(Status::Online))
}

fn snapshot(x: f32) -> Payload {
    Payload::Snapshot(Snapshot { seq: 1, t: 100, items: vec![Item { key: "player".into(), x, y: 2.0, rotation: 0.0 }] })
}

#[test]
fn two_clients_in_one_room_see_each_others_snapshots_and_leaving() {
    let addr = relay();
    let start = Instant::now();
    let (mut a, mut b) = (client(&addr, "room-1", "甲"), client(&addr, "room-1", "乙"));
    // 甲先进，乙再进：两边都在线、互相看见
    run(&mut [&mut a], start, |e| online(&e[0]));
    let events = run(&mut [&mut a, &mut b], start, |e| e[0].iter().any(|x| matches!(x, ClientEvent::Joined(_))) && e[1].iter().any(|x| matches!(x, ClientEvent::Joined(_))));
    assert_eq!((a.status(), b.status()), (Status::Online, Status::Online), "{events:?}");
    let (ida, idb) = (a.me().unwrap(), b.me().unwrap());
    assert_eq!(a.peers(), vec![Peer { id: idb, name: "乙".into() }]);
    assert_eq!(b.peers(), vec![Peer { id: ida, name: "甲".into() }]);

    assert!(a.send_payload(&snapshot(1.0), None));
    assert!(b.send_payload(&snapshot(2.0), None));
    let got = |e: &[ClientEvent], from| e.iter().any(|x| matches!(x, ClientEvent::Payload { from: f, payload } if *f == from && matches!(payload, Payload::Snapshot(_))));
    let events = run(&mut [&mut a, &mut b], start, |e| got(&e[0], idb) && got(&e[1], ida));
    assert!(got(&events[0], idb) && got(&events[1], ida), "{events:?}");
    assert!(events[0].contains(&ClientEvent::Payload { from: idb, payload: snapshot(2.0) }));

    // 乙走了：甲收到离开
    b.set_target(None);
    b.update(start.elapsed().as_secs_f64());
    drop(b);
    let events = run(&mut [&mut a], start, |e| !e[0].is_empty());
    assert_eq!(events[0], vec![ClientEvent::Left(Peer { id: idb, name: "乙".into() })]);
    assert!(a.peers().is_empty());
}

#[test]
fn different_rooms_do_not_see_each_other() {
    let addr = relay();
    let start = Instant::now();
    let (mut a, mut b) = (client(&addr, "r1", "a"), client(&addr, "r2", "b"));
    run(&mut [&mut a, &mut b], start, |e| online(&e[0]) && online(&e[1]));
    // 再跑一会儿：也没有对方进来的消息
    let events = run(&mut [&mut a, &mut b], start, |_| start.elapsed() > Duration::from_millis(300));
    assert!(events.iter().flatten().all(|e| !matches!(e, ClientEvent::Joined(_))), "{events:?}");
    assert_eq!((a.status(), b.status()), (Status::Online, Status::Online));
    assert!(a.peers().is_empty() && b.peers().is_empty());
}

fn http(addr: &str, request: &str) -> String {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn the_leaderboard_answers_over_http_on_the_same_port() {
    let addr = relay();
    let body = r#"{"board":"platformer-1","name":"甲","score":12.5}"#;
    let post = format!("POST /scores HTTP/1.1\r\nHost: x\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    let r = http(&addr, &post);
    assert!(r.starts_with("HTTP/1.1 200") && r.ends_with(r#"{"rank":1}"#), "{r}");
    let r = http(&addr, "GET /scores?board=platformer-1 HTTP/1.1\r\nHost: x\r\n\r\n");
    assert!(r.contains("Access-Control-Allow-Origin: *") && r.contains(r#""top":[{"name":"甲","score":12.5}]"#), "{r}");
    let r = http(&addr, &format!("POST /scores HTTP/1.1\r\nContent-Length: {}\r\n\r\n", 1 << 20));
    assert!(r.starts_with("HTTP/1.1 413"), "{r}");
}
