//! 内存里的排行榜：分数小的在前（比如通关秒数），每个名字在一个榜上只留最好的一条，每个榜最多 100 条；
//! 最多 64 个榜，满了去掉最久没写过的；一个账号最多建 `MAX_NEW_BOARDS` 个新榜（不带账号的开发模式不限）。
//! 名字归账号：鉴权后交的成绩带着账号，一个名字第一次被哪个账号用了就归它，别的账号（和不带账号的）再用这个名字交会被拒。
//! 比名字时先做 NFKC 规范化再转小写（`Alice`、`alice`、全角的 `Ａｌｉｃｅ` 算同一个名字）。
//! 不鉴权的本机开发模式里谁都能交分。

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
/// 最多记几个名字的归属：满了新名字交不了（只在内存里，重启清空）。
pub const MAX_OWNED: usize = 10_000;
/// 一个账号最多建几个新榜（只在内存里，重启清空）：拿到 token 的人刷不掉所有榜。
pub const MAX_NEW_BOARDS: usize = 8;
/// 名字归别的账号了。
pub const NAME_TAKEN: &str = "名字归别的账号了";
/// 名字归属表满了，新名字记不下。
pub const OWNERS_FULL: &str = "记的名字太多了";
/// 这个账号建的新榜够多了。
pub const TOO_MANY_BOARDS: &str = "这个账号建的新榜太多了";

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
    /// 名字（规范化以后）→ 账号。
    owners: HashMap<String, String>,
    /// 账号 → 建过几个新榜。
    created: HashMap<String, usize>,
}

impl Leaderboard {
    /// 不带账号交一个分数（本机开发模式），返回名次（从 1 数）；没进前 100、不比这个名字以前的好时是 `None`。
    pub fn submit(&mut self, board: &str, name: &str, score: f64) -> Result<Option<usize>, &'static str> {
        self.submit_as(board, None, name, score)
    }

    /// 以账号 `owner` 的名义交分（`None` 是不带账号）。名字归了别的账号时报 [`NAME_TAKEN`]。
    pub fn submit_as(&mut self, board: &str, owner: Option<&str>, name: &str, score: f64) -> Result<Option<usize>, &'static str> {
        let board = clean_text(board, MAX_BOARD).ok_or("榜名不合规")?;
        let name = clean_text(name, MAX_NAME).ok_or("名字不合规")?;
        if !(score.is_finite() && (0.0..MAX_SCORE).contains(&score)) {
            return Err("分数不合规");
        }
        let key = owner_key(&name);
        let claim = match (self.owners.get(&key), owner) {
            (Some(o), Some(me)) if o == me => false,
            (Some(_), _) => return Err(NAME_TAKEN),
            (None, Some(_)) if self.owners.len() >= MAX_OWNED => return Err(OWNERS_FULL),
            (None, Some(_)) => true,
            (None, None) => false,
        };
        let new_board = !self.boards.contains_key(&board);
        if let (true, Some(me)) = (new_board, owner) {
            let created = self.created.get(me).copied().unwrap_or(0);
            if created >= MAX_NEW_BOARDS {
                return Err(TOO_MANY_BOARDS);
            }
            self.created.insert(me.to_owned(), created + 1);
        }
        if let (true, Some(me)) = (claim, owner) {
            self.owners.insert(key, me.to_owned());
        }
        if new_board && self.boards.len() >= MAX_BOARDS {
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

    #[cfg(test)]
    pub(crate) fn fill_owners_for_tests(&mut self) {
        for i in self.owners.len()..MAX_OWNED {
            self.owners.insert(format!("filler-{i}"), format!("filler-acct-{i}"));
        }
    }

    pub fn top(&self, board: &str) -> Vec<Entry> {
        self.boards.get(board).map(|b| b.entries.iter().take(TOP).cloned().collect()).unwrap_or_default()
    }
}

/// 名字归属表的键：NFKC 规范化再转小写。
fn owner_key(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    name.nfkc().collect::<String>().to_lowercase()
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
    fn a_name_belongs_to_the_first_account_that_uses_it() {
        let mut b = Leaderboard::default();
        assert_eq!(b.submit_as("l", Some("acct-1"), "小红", 10.0), Ok(Some(1)));
        assert_eq!(b.submit_as("l", Some("acct-2"), "小红", 5.0), Err(NAME_TAKEN), "别的账号用不了这个名字");
        assert_eq!(b.submit("l", "小红", 5.0), Err(NAME_TAKEN), "不带账号也不行");
        assert_eq!(b.submit_as("l2", Some("acct-1"), "小红", 7.0), Ok(Some(1)), "自己在别的榜上照样用");
        assert_eq!(b.submit_as("l", Some("acct-1"), "小红", 9.0), Ok(Some(1)));
        assert_eq!(b.top("l"), vec![Entry { name: "小红".into(), score: 9.0 }]);
        // 一个账号可以有几个名字；不带账号交过的名字之后有账号来认领
        assert_eq!(b.submit_as("l", Some("acct-1"), "小红2", 20.0), Ok(Some(2)));
        assert_eq!(b.submit("l", "路人", 30.0), Ok(Some(3)));
        assert_eq!(b.submit_as("l", Some("acct-2"), "路人", 25.0), Ok(Some(3)));
        assert_eq!(b.submit("l", "路人", 1.0), Err(NAME_TAKEN));
        // 大小写、全角半角不同的同一个名字也归同一个账号
        assert_eq!(b.submit_as("l", Some("acct-1"), "Alice", 40.0), Ok(Some(4)));
        for lookalike in ["alice", "ALICE", "Ａｌｉｃｅ"] {
            assert_eq!(b.submit_as("l", Some("acct-2"), lookalike, 1.0), Err(NAME_TAKEN), "{lookalike}");
        }
        assert!(b.submit_as("l", Some("acct-1"), "alice", 39.0).is_ok(), "自己换个大小写照样能用");
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

    #[test]
    fn one_account_cannot_crowd_out_every_board() {
        let mut b = Leaderboard::default();
        for i in 0..MAX_BOARDS - MAX_NEW_BOARDS {
            b.submit_as(&format!("old{i}"), Some("acct-1"), "小红", 1.0).ok();
            b.created.clear();
        }
        for i in 0..MAX_NEW_BOARDS {
            assert_eq!(b.submit_as(&format!("spam{i}"), Some("acct-2"), "捣乱", 1.0), Ok(Some(1)));
        }
        assert_eq!(b.submit_as("spam-more", Some("acct-2"), "捣乱", 1.0), Err(TOO_MANY_BOARDS), "一个账号建的新榜有上限");
        assert_eq!(b.boards.len(), MAX_BOARDS);
        assert_eq!(b.top("old0").len(), 1, "别人的榜没被挤掉");
        assert_eq!(b.submit_as("spam0", Some("acct-2"), "捣乱", 0.5), Ok(Some(1)), "往自己建过的榜上交照样行");
        assert_eq!(b.submit_as("old0", Some("acct-2"), "捣乱", 0.5), Ok(Some(1)), "往已有的榜上交照样行");
        assert!(b.submit("dev-board", "路人", 1.0).is_ok(), "不带账号（开发模式）不限");
    }

    #[test]
    fn a_full_owner_table_refuses_new_names_with_its_own_error() {
        let mut b = Leaderboard::default();
        for i in 0..MAX_OWNED {
            b.owners.insert(format!("n{i}"), format!("acct-{i}"));
        }
        assert_eq!(b.submit_as("l", Some("acct-new"), "新名字", 1.0), Err(OWNERS_FULL));
        assert_eq!(b.submit_as("l", Some("acct-0"), "n0", 1.0), Ok(Some(1)), "已经归了的名字照样交");
        assert_eq!(b.submit_as("l", Some("acct-new"), "n1", 1.0), Err(NAME_TAKEN));
    }
}
