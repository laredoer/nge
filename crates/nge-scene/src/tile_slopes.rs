//! 瓦片斜坡的碰撞体：一条直线上连着的斜坡格（含它们下面的实心格）并成一块，坡和坡相接处没有棱角（见 docs/gameplay2d.md）。

use crate::tilemap::TileKind;

/// 一块斜坡碰撞体（格子坐标，行从上往下）：从第 `col` 列起 `w` 列宽，底边是第 `bottom` 行的底边，
/// 高 `h` 格；顶边左右两头的高是 `left` / `right`（`h` 的几分之几，和 `Shape::Slope` 一样）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SlopePiece {
    pub col: usize,
    pub w: usize,
    pub bottom: usize,
    pub h: f32,
    pub left: f32,
    pub right: f32,
    /// 第一格的下标（取材质用）。
    pub first: usize,
}

const EPS: f32 = 1e-4;

fn slope(kinds: &[TileKind], cols: usize, c: usize, r: usize) -> Option<(f32, f32)> {
    match kinds.get(r * cols + c) {
        Some(&TileKind::Slope(l, rr)) if c < cols => Some((l, rr)),
        _ => None,
    }
}

/// 顺着坡往右的下一格（同一条直线上）。
fn next(kinds: &[TileKind], cols: usize, c: usize, r: usize) -> Option<(usize, usize)> {
    let (l, rr) = slope(kinds, cols, c, r)?;
    let g = rr - l;
    let (nc, nr, want) = if g > EPS && rr >= 1.0 - EPS {
        (c + 1, r.checked_sub(1)?, 0.0)
    } else if g < -EPS && rr <= EPS {
        (c + 1, r + 1, 1.0)
    } else {
        (c + 1, r, rr)
    };
    if nc >= cols {
        return None;
    }
    let (nl, nrr) = slope(kinds, cols, nc, nr)?;
    ((nl - want).abs() < EPS && ((nrr - nl) - g).abs() < EPS).then_some((nc, nr))
}

/// 斜坡格并成的块，和被并进去的实心格（这些格子不再单独合并成矩形）。
/// 一条直线上连着的斜坡格并成一块，底边一直到最低那一格的底；中间盖住的格子都得是实心的，不是的地方断开成几段。
pub(crate) fn slope_pieces(kinds: &[TileKind], cols: usize) -> (Vec<SlopePiece>, Vec<bool>) {
    let rows = kinds.len().checked_div(cols).unwrap_or(0);
    let mut covered = vec![false; kinds.len()];
    let mut seen = vec![false; kinds.len()];
    let mut pieces = Vec::new();
    // 按列从左往右找：碰到的第一格就是一条坡最左边的那格
    for c in 0..cols {
        for r in 0..rows {
            let i = r * cols + c;
            if seen[i] || slope(kinds, cols, c, r).is_none() {
                continue;
            }
            let mut chain = vec![(c, r)];
            while let Some(n) = next(kinds, cols, chain[chain.len() - 1].0, chain[chain.len() - 1].1) {
                if seen[n.1 * cols + n.0] {
                    break;
                }
                chain.push(n);
            }
            for part in filled_parts(kinds, cols, chain) {
                for &(c, r) in &part {
                    seen[r * cols + c] = true;
                }
                pieces.push(piece(kinds, cols, &part, &mut covered));
            }
        }
    }
    (pieces, covered)
}

/// 把一条坡从左往右拆成尽量长的几段：每段里坡下面到这段底边之间的格子都是实心的（悬空的坡不把空格填上）。
fn filled_parts(kinds: &[TileKind], cols: usize, chain: Vec<(usize, usize)>) -> Vec<Vec<(usize, usize)>> {
    let filled = |part: &[(usize, usize)]| {
        let bottom = part.iter().map(|&(_, r)| r).max().unwrap_or(0);
        part.iter().all(|&(c, r)| (r + 1..=bottom).all(|rr| kinds[rr * cols + c] == TileKind::Solid))
    };
    let mut parts: Vec<Vec<(usize, usize)>> = Vec::new();
    for tile in chain {
        let joined = parts.last().is_some_and(|part| filled(&[part.as_slice(), &[tile]].concat()));
        match parts.last_mut() {
            Some(part) if joined => part.push(tile),
            _ => parts.push(vec![tile]),
        }
    }
    parts
}

fn piece(kinds: &[TileKind], cols: usize, part: &[(usize, usize)], covered: &mut [bool]) -> SlopePiece {
    let bottom = part.iter().map(|&(_, r)| r).max().expect("至少一格");
    // 从底边往上量的高（格）
    let height = |r: usize, f: f32| (bottom - r) as f32 + f;
    let (c0, r0) = part[0];
    let (c1, r1) = part[part.len() - 1];
    let (l, _) = slope(kinds, cols, c0, r0).expect("是斜坡");
    let (_, rr) = slope(kinds, cols, c1, r1).expect("是斜坡");
    let (yl, yr) = (height(r0, l), height(r1, rr));
    let h = yl.max(yr);
    for &(c, r) in part {
        for rr in r + 1..=bottom {
            covered[rr * cols + c] = true;
        }
    }
    SlopePiece { col: c0, w: c1 - c0 + 1, bottom, h, left: yl / h, right: yr / h, first: r0 * cols + c0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: TileKind = TileKind::Empty;
    const S: TileKind = TileKind::Solid;
    const UP: TileKind = TileKind::Slope(0.0, 1.0);

    #[test]
    fn a_filled_45_degree_staircase_is_one_piece() {
        // . . /
        // . / S
        // / S S
        let kinds = [E, E, UP, E, UP, S, UP, S, S];
        let (pieces, covered) = slope_pieces(&kinds, 3);
        assert_eq!(pieces, [SlopePiece { col: 0, w: 3, bottom: 2, h: 3.0, left: 0.0, right: 1.0, first: 6 }]);
        assert_eq!(covered, [false, false, false, false, false, true, false, true, true]);
    }

    #[test]
    fn gentle_slopes_join_across_rows() {
        let (lo, hi) = (TileKind::Slope(0.0, 0.5), TileKind::Slope(0.5, 1.0));
        let kinds = [E, E, lo, hi, lo, hi, S, S];
        let (pieces, _) = slope_pieces(&kinds, 4);
        assert_eq!(pieces, [SlopePiece { col: 0, w: 4, bottom: 1, h: 2.0, left: 0.0, right: 1.0, first: 4 }]);
    }

    #[test]
    fn falling_slopes_and_floating_ones() {
        let down = TileKind::Slope(1.0, 0.0);
        // \ .
        // S \
        let (pieces, _) = slope_pieces(&[down, E, S, down], 2);
        assert_eq!(pieces, [SlopePiece { col: 0, w: 2, bottom: 1, h: 2.0, left: 1.0, right: 0.0, first: 0 }]);
        // 下面空着（悬空的坡）：每格单独一块，不把空格填上
        let (pieces, covered) = slope_pieces(&[E, UP, UP, E], 2);
        assert_eq!(pieces.len(), 2);
        assert!(covered.iter().all(|c| !c));
        // 只有一部分下面填满：填满的那几格并成一段，悬空的单独
        // / . . .      第 0 列最低那格悬空（下面第 1 列空着），
        // . / S S      右边三格下面都是实心的
        let kinds = [E, E, E, UP, E, E, UP, S, E, UP, S, S, UP, E, S, S];
        let (pieces, covered) = slope_pieces(&kinds, 4);
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        assert_eq!((pieces[0].col, pieces[0].w), (0, 1));
        assert_eq!((pieces[1].col, pieces[1].w, pieces[1].bottom), (1, 3, 2));
        assert!(!covered[13], "悬空的坡下面的空格不填");
        // 坡度不一样的不并
        let (pieces, _) = slope_pieces(&[UP, TileKind::Slope(0.0, 0.5)], 2);
        assert_eq!(pieces.len(), 2);
    }
}
