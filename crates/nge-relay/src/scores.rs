//! 内存里的排行榜：分数小的在前（比如通关秒数），每个名字在一个榜上只留最好的一条，每个榜最多 100 条；
//! 最多 64 个榜，满了去掉最久没写过的。只是给 HTTP 演示用，谁都能交分。

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
struct Board {
    entries: Vec<Entry>,
    /// 最后一次写入的序号：榜满了先去掉最小的。
    written: u64,
}

#[derive(Debug, Default)]
pub struct Leaderboard {
    boards: HashMap<String, Board>,
    writes: u64,
}

impl Leaderboard {
    /// 交一个分数，返回名次（从 1 数）；没进前 100、不比这个名字以前的好时是 `None`。
    pub fn submit(&mut self, board: &str, name: &str, score: f64) -> Result<Option<usize>, &'static str> {
        let board = clean_text(board, MAX_BOARD).ok_or("榜名不合规")?;
        let name = clean_text(name, MAX_NAME).ok_or("名字不合规")?;
        if !(score.is_finite() && (0.0..MAX_SCORE).contains(&score)) {
            return Err("分数不合规");
        }
        if !self.boards.contains_key(&board) && self.boards.len() >= MAX_BOARDS {
            let oldest = self.boards.iter().min_by_key(|(_, b)| b.written).map(|(k, _)| k.clone());
            if let Some(oldest) = oldest {
                self.boards.remove(&oldest);
            }
        }
        self.writes += 1;
        let writes = self.writes;
        let b = self.boards.entry(board).or_default();
        b.written = writes;
        let entries = &mut b.entries;
        if let Some(old) = entries.iter().position(|e| e.name == name) {
            if entries[old].score <= score {
                return Ok(None);
            }
            entries.remove(old);
        }
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
        self.boards.get(board).map(|b| b.entries.iter().take(TOP).cloned().collect()).unwrap_or_default()
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
            b.submit("l", &format!("p{i}"), i as f64).unwrap();
        }
        assert_eq!(b.submit("l", "slow", 1000.0), Ok(None));
        assert_eq!(b.submit("l", "fast", 0.5), Ok(Some(2)));
        assert_eq!(b.top("l").len(), TOP);
        assert_eq!(b.boards["l"].entries.len(), MAX_ENTRIES);
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
    }

    #[test]
    fn one_entry_per_name_and_only_a_better_score_replaces_it() {
        let mut b = Leaderboard::default();
        assert_eq!(b.submit("l", "a", 10.0), Ok(Some(1)));
        assert_eq!(b.submit("l", "a", 12.0), Ok(None), "更差的不记");
        assert_eq!(b.submit("l", "b", 11.0), Ok(Some(2)));
        assert_eq!(b.submit("l", "a", 10.0), Ok(None), "一样的不重复记");
        assert_eq!(b.submit("l", "b", 9.0), Ok(Some(1)));
        // 名字前后的空白不算不同的名字
        assert_eq!(b.submit("l", " b ", 8.0), Ok(Some(1)));
        let top: Vec<(String, f64)> = b.top("l").into_iter().map(|e| (e.name, e.score)).collect();
        assert_eq!(top, [("b".to_owned(), 8.0), ("a".to_owned(), 10.0)]);
        // 一个名字刷不满榜
        for i in 0..MAX_ENTRIES * 2 {
            let _ = b.submit("l", "spam", 100.0 - i as f64 * 0.1);
        }
        assert_eq!(b.boards["l"].entries.len(), 3);
    }

    #[test]
    fn a_full_set_of_boards_drops_the_one_written_longest_ago() {
        let mut b = Leaderboard::default();
        for i in 0..MAX_BOARDS {
            b.submit(&format!("b{i}"), "a", 1.0).unwrap();
        }
        // b0 最近又写过，b1 成了最久没写的
        b.submit("b0", "x", 2.0).unwrap();
        assert_eq!(b.submit("new-level", "a", 1.0), Ok(Some(1)), "新关卡照样能进");
        assert_eq!(b.boards.len(), MAX_BOARDS);
        assert!(b.top("b1").is_empty());
        assert_eq!(b.top("b0").len(), 2);
        assert_eq!(b.top("b2").len(), 1);
    }
}
