//! 탭 레이아웃 트리 조작과 화면 영역 계산. 서버(PTY 크기)와 클라이언트(그리기)가 같은 함수를 쓴다.

use kmttt_proto::{LayoutNode, SplitDir};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// 트리에 있는 pane ID들(왼쪽·위쪽부터).
pub fn panes(node: &LayoutNode) -> Vec<String> {
    let mut out = Vec::new();
    collect(node, &mut out);
    out
}

fn collect(node: &LayoutNode, out: &mut Vec<String>) {
    match node {
        LayoutNode::Leaf { pane } => out.push(pane.clone()),
        LayoutNode::Split { a, b, .. } => {
            collect(a, out);
            collect(b, out);
        }
    }
}

pub fn contains(node: &LayoutNode, pane: &str) -> bool {
    panes(node).iter().any(|p| p == pane)
}

/// `target` 자리를 둘로 나누고 `new_pane`을 오른쪽/아래에 넣는다. 성공하면 true.
pub fn split(node: &mut LayoutNode, target: &str, new_pane: &str, dir: SplitDir) -> bool {
    match node {
        LayoutNode::Leaf { pane } if pane == target => {
            let old = LayoutNode::Leaf { pane: pane.clone() };
            *node = LayoutNode::Split {
                dir,
                ratio: 0.5,
                a: Box::new(old),
                b: Box::new(LayoutNode::Leaf {
                    pane: new_pane.to_string(),
                }),
            };
            true
        }
        LayoutNode::Leaf { .. } => false,
        LayoutNode::Split { a, b, .. } => {
            split(a, target, new_pane, dir) || split(b, target, new_pane, dir)
        }
    }
}

/// pane 하나를 빼고 형제가 그 자리를 차지한다. 트리에 마지막 pane이면 None을 돌려준다.
pub fn remove(node: LayoutNode, pane: &str) -> Option<LayoutNode> {
    match node {
        LayoutNode::Leaf { pane: p } => {
            if p == pane {
                None
            } else {
                Some(LayoutNode::Leaf { pane: p })
            }
        }
        LayoutNode::Split { dir, ratio, a, b } => match (remove(*a, pane), remove(*b, pane)) {
            (Some(a), Some(b)) => Some(LayoutNode::Split {
                dir,
                ratio,
                a: Box::new(a),
                b: Box::new(b),
            }),
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        },
    }
}

/// 영역 계산. 나뉜 두 쪽 사이에는 1칸 경계선을 둔다.
pub fn compute(node: &LayoutNode, area: Rect) -> Vec<(String, Rect)> {
    let mut out = Vec::new();
    compute_into(node, area, &mut out);
    out
}

fn compute_into(node: &LayoutNode, area: Rect, out: &mut Vec<(String, Rect)>) {
    match node {
        LayoutNode::Leaf { pane } => out.push((pane.clone(), area)),
        LayoutNode::Split { dir, ratio, a, b } => {
            let ratio = ratio.clamp(0.1, 0.9);
            match dir {
                SplitDir::Right => {
                    let usable = area.w.saturating_sub(1);
                    let aw = ((usable as f32) * ratio).round() as u16;
                    let aw = aw.clamp(1.min(usable), usable.saturating_sub(1).max(1));
                    let bw = usable.saturating_sub(aw);
                    compute_into(a, Rect { w: aw, ..area }, out);
                    compute_into(
                        b,
                        Rect {
                            x: area.x + aw + 1,
                            w: bw,
                            ..area
                        },
                        out,
                    );
                }
                SplitDir::Down => {
                    let usable = area.h.saturating_sub(1);
                    let ah = ((usable as f32) * ratio).round() as u16;
                    let ah = ah.clamp(1.min(usable), usable.saturating_sub(1).max(1));
                    let bh = usable.saturating_sub(ah);
                    compute_into(a, Rect { h: ah, ..area }, out);
                    compute_into(
                        b,
                        Rect {
                            y: area.y + ah + 1,
                            h: bh,
                            ..area
                        },
                        out,
                    );
                }
            }
        }
    }
}

/// 마우스로 잡은 경계선: 루트에서 그 분할까지의 경로(false=a, true=b), 분할 전체 영역, 방향.
#[derive(Debug, Clone, PartialEq)]
pub struct DividerHit {
    pub path: Vec<bool>,
    pub area: Rect,
    pub dir: SplitDir,
}

/// `(col, row)`에 있는 경계선을 찾는다.
pub fn divider_at(node: &LayoutNode, area: Rect, col: u16, row: u16) -> Option<DividerHit> {
    let mut path = Vec::new();
    divider_at_inner(node, area, col, row, &mut path)
}

fn divider_at_inner(
    node: &LayoutNode,
    area: Rect,
    col: u16,
    row: u16,
    path: &mut Vec<bool>,
) -> Option<DividerHit> {
    let LayoutNode::Split { dir, ratio, a, b } = node else {
        return None;
    };
    let ratio = ratio.clamp(0.1, 0.9);
    let (ra, line, rb) = match dir {
        SplitDir::Right => {
            let usable = area.w.saturating_sub(1);
            let aw = (((usable as f32) * ratio).round() as u16)
                .clamp(1.min(usable), usable.saturating_sub(1).max(1));
            (
                Rect { w: aw, ..area },
                col == area.x + aw && row >= area.y && row < area.y + area.h,
                Rect {
                    x: area.x + aw + 1,
                    w: usable.saturating_sub(aw),
                    ..area
                },
            )
        }
        SplitDir::Down => {
            let usable = area.h.saturating_sub(1);
            let ah = (((usable as f32) * ratio).round() as u16)
                .clamp(1.min(usable), usable.saturating_sub(1).max(1));
            (
                Rect { h: ah, ..area },
                row == area.y + ah && col >= area.x && col < area.x + area.w,
                Rect {
                    y: area.y + ah + 1,
                    h: usable.saturating_sub(ah),
                    ..area
                },
            )
        }
    };
    if line {
        return Some(DividerHit {
            path: path.clone(),
            area,
            dir: *dir,
        });
    }
    for (side, child, r) in [(false, a, ra), (true, b, rb)] {
        path.push(side);
        if let Some(h) = divider_at_inner(child, r, col, row, path) {
            return Some(h);
        }
        path.pop();
    }
    None
}

/// 마우스 위치로 새 비율을 계산한다(0.1~0.9).
pub fn ratio_for(hit: &DividerHit, col: u16, row: u16) -> f32 {
    let (pos, start, len) = match hit.dir {
        SplitDir::Right => (col, hit.area.x, hit.area.w.saturating_sub(1)),
        SplitDir::Down => (row, hit.area.y, hit.area.h.saturating_sub(1)),
    };
    if len == 0 {
        return 0.5;
    }
    ((pos.saturating_sub(start)) as f32 / len as f32).clamp(0.1, 0.9)
}

/// 경로가 가리키는 분할의 비율을 바꾼다. 경로가 분할을 가리키지 않으면 false.
pub fn set_ratio(node: &mut LayoutNode, path: &[bool], new_ratio: f32) -> bool {
    let LayoutNode::Split { ratio, a, b, .. } = node else {
        return false;
    };
    match path.split_first() {
        None => {
            *ratio = new_ratio.clamp(0.1, 0.9);
            true
        }
        Some((false, rest)) => set_ratio(a, rest, new_ratio),
        Some((true, rest)) => set_ratio(b, rest, new_ratio),
    }
}

/// 경계선 위치(그리기용): (x, y, 길이, 세로인지)
pub fn dividers(node: &LayoutNode, area: Rect) -> Vec<(u16, u16, u16, bool)> {
    let mut out = Vec::new();
    dividers_into(node, area, &mut out);
    out
}

fn dividers_into(node: &LayoutNode, area: Rect, out: &mut Vec<(u16, u16, u16, bool)>) {
    if let LayoutNode::Split { dir, ratio, a, b } = node {
        let ratio = ratio.clamp(0.1, 0.9);
        match dir {
            SplitDir::Right => {
                let usable = area.w.saturating_sub(1);
                let aw = (((usable as f32) * ratio).round() as u16)
                    .clamp(1.min(usable), usable.saturating_sub(1).max(1));
                out.push((area.x + aw, area.y, area.h, true));
                dividers_into(a, Rect { w: aw, ..area }, out);
                dividers_into(
                    b,
                    Rect {
                        x: area.x + aw + 1,
                        w: usable.saturating_sub(aw),
                        ..area
                    },
                    out,
                );
            }
            SplitDir::Down => {
                let usable = area.h.saturating_sub(1);
                let ah = (((usable as f32) * ratio).round() as u16)
                    .clamp(1.min(usable), usable.saturating_sub(1).max(1));
                out.push((area.x, area.y + ah, area.w, false));
                dividers_into(a, Rect { h: ah, ..area }, out);
                dividers_into(
                    b,
                    Rect {
                        y: area.y + ah + 1,
                        h: usable.saturating_sub(ah),
                        ..area
                    },
                    out,
                );
            }
        }
    }
}

/// 방향 이동: 현재 pane에서 dx/dy 쪽으로 가장 가까운 pane.
pub fn neighbor(node: &LayoutNode, area: Rect, from: &str, dx: i32, dy: i32) -> Option<String> {
    let rects = compute(node, area);
    let cur = rects.iter().find(|(p, _)| p == from)?.1;
    let (cx, cy) = (
        cur.x as i32 + cur.w as i32 / 2,
        cur.y as i32 + cur.h as i32 / 2,
    );
    rects
        .iter()
        .filter(|(p, _)| p != from)
        .filter(|(_, r)| {
            let (rx, ry) = (r.x as i32 + r.w as i32 / 2, r.y as i32 + r.h as i32 / 2);
            (dx > 0 && rx > cx) || (dx < 0 && rx < cx) || (dy > 0 && ry > cy) || (dy < 0 && ry < cy)
        })
        .min_by_key(|(_, r)| {
            let (rx, ry) = (r.x as i32 + r.w as i32 / 2, r.y as i32 + r.h as i32 / 2);
            (rx - cx).abs() * if dx != 0 { 1 } else { 3 }
                + (ry - cy).abs() * if dy != 0 { 1 } else { 3 }
        })
        .map(|(p, _)| p.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(p: &str) -> LayoutNode {
        LayoutNode::Leaf { pane: p.into() }
    }

    #[test]
    fn split_and_compute() {
        let mut t = leaf("p1");
        assert!(split(&mut t, "p1", "p2", SplitDir::Right));
        assert!(split(&mut t, "p2", "p3", SplitDir::Down));
        assert_eq!(panes(&t), vec!["p1", "p2", "p3"]);
        let r = compute(
            &t,
            Rect {
                x: 0,
                y: 0,
                w: 81,
                h: 25,
            },
        );
        assert_eq!(
            r[0].1,
            Rect {
                x: 0,
                y: 0,
                w: 40,
                h: 25
            }
        );
        assert_eq!(
            r[1].1,
            Rect {
                x: 41,
                y: 0,
                w: 40,
                h: 12
            }
        );
        assert_eq!(
            r[2].1,
            Rect {
                x: 41,
                y: 13,
                w: 40,
                h: 12
            }
        );
        // 경계선 두 개
        assert_eq!(
            dividers(
                &t,
                Rect {
                    x: 0,
                    y: 0,
                    w: 81,
                    h: 25
                }
            )
            .len(),
            2
        );
    }

    #[test]
    fn remove_collapses() {
        let mut t = leaf("p1");
        split(&mut t, "p1", "p2", SplitDir::Right);
        let t = remove(t, "p1").unwrap();
        assert_eq!(t, leaf("p2"));
        assert!(remove(t, "p2").is_none());
    }

    #[test]
    fn tiny_area_does_not_panic() {
        let mut t = leaf("a");
        split(&mut t, "a", "b", SplitDir::Right);
        split(&mut t, "b", "c", SplitDir::Down);
        for w in 0..4 {
            for h in 0..4 {
                let _ = compute(&t, Rect { x: 0, y: 0, w, h });
            }
        }
    }

    #[test]
    fn neighbor_moves() {
        let mut t = leaf("p1");
        split(&mut t, "p1", "p2", SplitDir::Right);
        let area = Rect {
            x: 0,
            y: 0,
            w: 80,
            h: 24,
        };
        assert_eq!(neighbor(&t, area, "p1", 1, 0).as_deref(), Some("p2"));
        assert_eq!(neighbor(&t, area, "p2", -1, 0).as_deref(), Some("p1"));
        assert_eq!(neighbor(&t, area, "p1", 0, 1), None);
    }

    #[test]
    fn drag_divider_changes_ratio() {
        let mut l = leaf("p1");
        split(&mut l, "p1", "p2", SplitDir::Right);
        split(&mut l, "p2", "p3", SplitDir::Down);
        let area = Rect {
            x: 0,
            y: 0,
            w: 81,
            h: 21,
        };
        // 세로 경계선(가운데 열)
        let hit = divider_at(&l, area, 40, 5).expect("root divider");
        assert!(hit.path.is_empty());
        assert_eq!(hit.dir, SplitDir::Right);
        let r = ratio_for(&hit, 60, 5);
        assert!((r - 0.75).abs() < 0.01, "{r}");
        assert!(set_ratio(&mut l, &hit.path, r));
        // 오른쪽 아래 분할의 가로 경계선
        let rects = compute(&l, area);
        let p2 = rects.iter().find(|(id, _)| id == "p2").unwrap().1;
        let hit2 = divider_at(&l, area, p2.x + 2, p2.y + p2.h).expect("nested divider");
        assert_eq!(hit2.path, vec![true]);
        assert_eq!(hit2.dir, SplitDir::Down);
        assert!(divider_at(&l, area, 1, 1).is_none());
    }
}
