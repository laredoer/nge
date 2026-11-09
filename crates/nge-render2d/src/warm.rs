//! 管线预热：着色器库到了（或换了一代）就在后台线程先把管线编好，渲染线程每帧收现成的（见 docs/effects2d.md）。
//!
//! Metal 等驱动第一次编某种管线要一两百毫秒，等到要画时才编会卡一下。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

/// 后台编好的东西，按开工的批次打标签；换了一批后旧批次的结果丢掉，旧线程也尽早收工。
pub(crate) struct Warm<T> {
    tx: Sender<(u64, Option<T>)>,
    rx: Receiver<(u64, Option<T>)>,
    /// 最新一批的编号，后台线程拿它判断自己是不是过时了。
    batch: Arc<AtomicU64>,
    /// 最新一批还没收工。
    busy: bool,
}

impl<T: Send + 'static> Warm<T> {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self { tx, rx, batch: Arc::new(AtomicU64::new(0)), busy: false }
    }

    /// 开新的一批：`job` 在后台线程里跑，每编好一个交给 `send`；`send` 返回 false 时这批已经过时，别再编了。
    pub fn start(&mut self, job: impl FnOnce(&dyn Fn(T) -> bool) + Send + 'static) {
        let batch = self.batch.fetch_add(1, Ordering::SeqCst) + 1;
        self.busy = true;
        let (tx, current) = (self.tx.clone(), self.batch.clone());
        let spawned = std::thread::Builder::new().name("nge 管线预热".into()).spawn(move || {
            // 编管线 panic 了也要报收工
            let _done = Done(tx.clone(), batch);
            let send = |item: T| current.load(Ordering::SeqCst) == batch && tx.send((batch, Some(item))).is_ok();
            job(&send);
        });
        if let Err(e) = spawned {
            // 开不了线程就不预热，用到时照常当场编
            log::warn!("开不了管线预热的线程：{e}");
            self.busy = false;
        }
    }

    /// 收下最新一批已经编好的（不等）。
    pub fn drain(&mut self) -> Vec<T> {
        let mut items = Vec::new();
        while let Ok(message) = self.rx.try_recv() {
            self.take(message, &mut items);
        }
        items
    }

    fn take(&mut self, (batch, item): (u64, Option<T>), items: &mut Vec<T>) {
        if batch != self.batch.load(Ordering::SeqCst) {
            return;
        }
        match item {
            Some(item) => items.push(item),
            None => self.busy = false,
        }
    }

    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.busy
    }

    /// 等最新一批收工，返回还没收下的。
    #[cfg(test)]
    pub fn finish(&mut self) -> Vec<T> {
        let mut items = Vec::new();
        while self.busy {
            let message = self.rx.recv().expect("自己还拿着发送端");
            self.take(message, &mut items);
        }
        items
    }
}

/// 后台线程结束（包括 panic）时告诉渲染线程这一批收工了。
struct Done<T>(Sender<(u64, Option<T>)>, u64);

impl<T> Drop for Done<T> {
    fn drop(&mut self) {
        let _ = self.0.send((self.1, None));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::Warm;

    #[test]
    fn hands_over_what_the_worker_sends() {
        let mut warm = Warm::new();
        warm.start(|send| {
            for i in 0..3 {
                assert!(send(i));
            }
        });
        let mut got = warm.drain();
        got.extend(warm.finish());
        assert_eq!(got, [0, 1, 2]);
        assert!(warm.drain().is_empty());
    }

    #[test]
    fn drops_results_of_an_outdated_batch_and_tells_the_worker_to_stop() {
        let mut warm = Warm::new();
        let (go_tx, go_rx) = channel::<()>();
        let (result_tx, result_rx) = channel();
        warm.start(move |send| {
            go_rx.recv().unwrap();
            result_tx.send(send(1)).unwrap();
        });
        warm.start(|send| {
            send(2);
        });
        go_tx.send(()).unwrap();
        // 旧的一批被告知过时
        assert!(!result_rx.recv().unwrap());
        assert_eq!(warm.finish(), [2]);
        assert!(warm.drain().is_empty());
    }

    #[test]
    fn a_worker_that_panics_still_finishes_its_batch() {
        let mut warm = Warm::new();
        warm.start(|send| {
            send(1);
            panic!("编管线出错");
        });
        assert_eq!(warm.finish(), [1]);
        assert!(!warm.busy());
    }
}
