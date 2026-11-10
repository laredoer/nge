//! 内存里的排行榜：分数小的在前（比如通关秒数），每个榜最多留 100 条，最多 64 个榜。只是给 HTTP 演示用。

use std::collections::HashMap;

use nge_net::proto::{MAX_NAME, clean_text};
use serde::{Deserialize, Serialize};

/// 返回前几名。
pub const TOP: usize = 10;
/// 每个榜最多留几条。
pub const MAX_ENTRIES: usize = 100;
/// 最多几个榜。
pub const MAX_BOARDS: usize = 64;
/// 榜名最多几个字符。
pub const MAX_BOARD: usize = 64;
/// 分数的上限（不含）。
pub const MAX_SCORE: f64 = 1.0e9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub score: f64,
}

#[derive(Debug, Default)]
pub struct Leaderboard {
    boards: HashMap<String, Vec<Entry>>,
}

impl Leaderboard {
    /// 交一个分数，返回名次（从 1 数）；没进前 100 时是 `None`。
    pub fn submit(&mut self, board: &str, name: &str, score: f64) -> Result<Option<usize>, &'static str> {
        let board = clean_text(board, MAX_BOARD).ok_or("榜名不合规")?;
        let name = clean_text(name, MAX_NAME).ok_or("名字不合规")?;
        if !(score.is_finite() && (0.0..MAX_SCORE).contains(&score)) {
            return Err("分数不合规");
        }
        if !self.boards.contains_key(&board) && self.boards.len() >= MAX_BOARDS {
            return Err("榜太多了");
        }
        let entries = self.boards.entry(board).or_default();
        // 同分的排在先交的后面
        let rank = entries.partition_point(|e| e.score <= score);
        if rank >= MAX_ENTRIES {
            return Ok(None);
        }
        entries.insert(rank, Entry { name, score });
        entries.truncate(MAX_ENTRIES);
        Ok(Some(rank + 1))
    }

    pub fn top(&self, board: &str) -> Vec<Entry> {
        self.boards.get(board).map(|e| e.iter().take(TOP).cloned().collect()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smaller_scores_rank_first_and_ties_keep_order() {
        let mut b = Leaderboard::default();
        assert_eq!(b.submit("l1", "a", 12.5), Ok(Some(1)));
        assert_eq!(b.submit("l1", "b", 9.0), Ok(Some(1)));
        assert_eq!(b.submit("l1", "c", 12.5), Ok(Some(3)), "同分排在后面");
        let names: Vec<String> = b.top("l1").into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["b", "a", "c"]);
        assert!(b.top("l2").is_empty());
    }

    #[test]
    fn only_the_best_hundred_are_kept_and_ten_are_returned() {
        let mut b = Leaderboard::default();
        for i in 0..MAX_ENTRIES {
            b.submit("l", "p", i as f64).unwrap();
        }
        assert_eq!(b.submit("l", "slow", 1000.0), Ok(None));
        assert_eq!(b.submit("l", "fast", 0.5), Ok(Some(2)));
        assert_eq!(b.top("l").len(), TOP);
        assert_eq!(b.boards["l"].len(), MAX_ENTRIES);
    }

    #[test]
    fn bad_input_is_refused() {
        let mut b = Leaderboard::default();
        for score in [f64::NAN, f64::INFINITY, -1.0, MAX_SCORE] {
            assert!(b.submit("l", "a", score).is_err(), "{score}");
        }
        assert!(b.submit("", "a", 1.0).is_err());
        assert!(b.submit("l", "a\u{1b}", 1.0).is_err());
        assert!(b.submit("l", &"n".repeat(MAX_NAME + 1), 1.0).is_err());
        for i in 0..MAX_BOARDS {
            b.submit(&format!("b{i}"), "a", 1.0).unwrap();
        }
        assert!(b.submit("one-more", "a", 1.0).is_err());
        assert!(b.submit("b0", "a", 1.0).is_ok(), "已有的榜照常交");
    }
}
