//! Where a flowchart's nodes, edges and edge labels go: a small layered
//! (Sugiyama) layout, good for hand-written flowcharts of a few dozen
//! nodes, after dagre's approach:
//!
//! 1. Break cycles by reversing the edges a depth-first search finds going
//!    back up it.
//! 2. Rank each node by the longest path to it, then pull sources down
//!    next to what they point at.
//! 3. Split edges spanning several ranks into chains of dummy vertices. A
//!    labelled edge's label rides the middle one, and when any edge has a
//!    label every edge is twice as long, so each has a rank in its middle.
//! 4. Order each rank with barycentre sweeps and a transpose pass, keeping
//!    the order with the fewest crossings.
//! 5. Place each rank's vertices across the page as close as their order
//!    and spacing let them be to the mean of their neighbours
//!    (pool-adjacent-violators).
//! 6. Place ranks down the page by their heights.
//!
//! The layout is worked out top-down; the other directions transpose or
//! flip it. Nothing depends on hash order, so a graph always lays out the
//! same way.

use super::Direction;

/// Space between two nodes in a rank.
pub const NODE_SEP: f32 = 30.0;
/// Space between edges passing through a rank.
pub const EDGE_SEP: f32 = 12.0;
/// Space between ranks.
pub const RANK_SEP: f32 = 44.0;
/// How far a self-loop sticks out of its node's side.
pub const LOOP: f32 = 22.0;
/// Space between a self-loop and its label.
pub const LOOP_LABEL_GAP: f32 = 4.0;
/// The most ranks a layout can have, counting the extra ones edge labels
/// add, and the most bends its edges can have between them: past these a
/// diagram would be too tall to read and slow to lay out.
pub const MAX_RANKS: usize = 600;
pub const MAX_BENDS: usize = 10_000;

/// Why a graph wasn't laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct TooBig(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// An edge to lay out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeIn {
    pub from: usize,
    pub to: usize,
    /// The fewest ranks it spans.
    pub min_len: usize,
    /// Its label's size, if it has one.
    pub label: Option<Size>,
}

/// Where an edge goes.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// From its source's centre through its bends to its target's centre.
    /// Empty for a self-loop, which is drawn off the side of its node:
    /// the right in TD and BT, the bottom in LR and RL, [`LOOP`] out.
    pub points: Vec<Point>,
    /// Its label's centre.
    pub label: Option<Point>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    pub width: f32,
    pub height: f32,
    /// Node centres.
    pub nodes: Vec<Point>,
    pub edges: Vec<Route>,
}

/// A vertex in the top-down working: a node, an edge label, or a bend.
#[derive(Debug, Clone)]
struct V {
    /// Room left and right of its centre.
    left: f32,
    right: f32,
    /// Its height down the page.
    h: f32,
    rank: usize,
    x: f32,
    /// A node or label, rather than a bend in an edge.
    solid: bool,
}

/// Lays out `nodes` (their sizes) and `edges` in `direction`, unless that
/// takes more than [`MAX_RANKS`] ranks or [`MAX_BENDS`] bends.
pub fn layout(direction: Direction, nodes: &[Size], edges: &[EdgeIn]) -> Result<Layout, TooBig> {
    let horizontal = direction.horizontal();
    // Sizes as the top-down working sees them.
    let tr = |s: Size| {
        if horizontal {
            Size { w: s.h, h: s.w }
        } else {
            s
        }
    };
    let n = nodes.len();
    let mut vs: Vec<V> = nodes
        .iter()
        .map(|&s| {
            let s = tr(s);
            V {
                left: s.w / 2.0,
                right: s.w / 2.0,
                h: s.h,
                rank: 0,
                x: 0.0,
                solid: true,
            }
        })
        .collect();
    // Room for self-loops and their labels on each node's right.
    let mut loop_room = vec![0.0f32; n];
    for e in edges.iter().filter(|e| e.from == e.to) {
        let label = e.label.map_or(0.0, |l| LOOP_LABEL_GAP + tr(l).w);
        loop_room[e.from] = loop_room[e.from].max(LOOP + label);
    }
    for (v, room) in vs.iter_mut().zip(&loop_room) {
        v.right += room;
    }

    let reversed = break_cycles(n, edges);
    let labelled = edges.iter().any(|e| e.label.is_some() && e.from != e.to);
    let stretch = if labelled { 2 } else { 1 };
    // Edges as ranked: (upper, lower, length, edge index).
    let dag: Vec<(usize, usize, usize, usize)> = edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.from != e.to)
        .map(|(i, e)| {
            let (a, b) = if reversed[i] {
                (e.to, e.from)
            } else {
                (e.from, e.to)
            };
            (a, b, e.min_len.clamp(1, MAX_RANKS) * stretch, i)
        })
        .collect();
    rank(&mut vs, &dag);
    let ranks = vs.iter().map(|v| v.rank + 1).max().unwrap_or(0);
    if ranks > MAX_RANKS {
        return Err(TooBig(format!(
            "the flowchart is too deep to draw: it needs {ranks} ranks, and at most {MAX_RANKS} fit"
        )));
    }
    let bends: usize = dag
        .iter()
        .map(|&(a, b, _, _)| vs[b].rank - vs[a].rank - 1)
        .sum();
    if bends > MAX_BENDS {
        return Err(TooBig(format!(
            "the flowchart's links are too long to draw: they bend {bends} times, and at most {MAX_BENDS} fit"
        )));
    }

    // 3. Each edge's chain of vertices, from its upper end.
    let mut chains: Vec<Vec<usize>> = vec![Vec::new(); edges.len()];
    let mut label_at: Vec<Option<usize>> = vec![None; edges.len()];
    for &(a, b, _, i) in &dag {
        let (ra, rb) = (vs[a].rank, vs[b].rank);
        let mid = ra + (rb - ra) / 2;
        let mut chain = vec![a];
        for r in ra + 1..rb {
            let label = edges[i].label.filter(|_| r == mid).map(tr);
            if label.is_some() {
                label_at[i] = Some(vs.len());
            }
            let s = label.unwrap_or_default();
            vs.push(V {
                left: s.w / 2.0,
                right: s.w / 2.0,
                h: s.h,
                rank: r,
                x: 0.0,
                solid: label.is_some(),
            });
            chain.push(vs.len() - 1);
        }
        chain.push(b);
        chains[i] = chain;
    }
    let mut up = vec![Vec::new(); vs.len()];
    let mut down = vec![Vec::new(); vs.len()];
    for chain in &chains {
        for w in chain.windows(2) {
            down[w[0]].push(w[1]);
            up[w[1]].push(w[0]);
        }
    }

    let ranks = order(&vs, &up, &down);
    place_x(&mut vs, &ranks, &up, &down);

    // 6. Ranks down the page.
    let mut rank_y = vec![0f32; ranks.len()];
    let sep = RANK_SEP / stretch as f32;
    let mut y = 0.0;
    for (r, rank) in ranks.iter().enumerate() {
        let h = rank.iter().map(|&v| vs[v].h).fold(0.0, f32::max);
        rank_y[r] = y + h / 2.0;
        y += h + sep;
    }
    let depth = (y - sep).max(0.0);
    let min_x = vs
        .iter()
        .map(|v| v.x - v.left)
        .fold(f32::INFINITY, f32::min);
    let min_x = if min_x.is_finite() { min_x } else { 0.0 };
    let breadth = vs.iter().map(|v| v.x + v.right - min_x).fold(0.0, f32::max);

    // Top-down (x, y) to the diagram's direction.
    let point = |x: f32, y: f32| match direction {
        Direction::TopDown => Point { x, y },
        Direction::BottomUp => Point { x, y: depth - y },
        Direction::LeftRight => Point { x: y, y: x },
        Direction::RightLeft => Point { x: depth - y, y: x },
    };
    let at = |v: usize| point(vs[v].x - min_x, rank_y[vs[v].rank]);
    let routes = edges
        .iter()
        .enumerate()
        .map(|(i, e)| {
            if e.from == e.to {
                let v = &vs[e.from];
                let label = e.label.map(|l| {
                    let half = (v.right - loop_room[e.from]).max(0.0);
                    point(
                        v.x - min_x + half + LOOP + LOOP_LABEL_GAP + tr(l).w / 2.0,
                        rank_y[v.rank],
                    )
                });
                return Route {
                    points: Vec::new(),
                    label,
                };
            }
            let mut points: Vec<Point> = chains[i].iter().map(|&v| at(v)).collect();
            if reversed[i] {
                points.reverse();
            }
            Route {
                points,
                label: label_at[i].map(at),
            }
        })
        .collect();
    let (width, height) = if horizontal {
        (depth, breadth)
    } else {
        (breadth, depth)
    };
    Ok(Layout {
        width,
        height,
        nodes: (0..n).map(at).collect(),
        edges: routes,
    })
}

/// Phase 1: which edges to turn round so the graph has no cycles: those a
/// depth-first search, from each node in order, finds leading back to a
/// node it's still in. Self-loops are left alone.
fn break_cycles(n: usize, edges: &[EdgeIn]) -> Vec<bool> {
    let mut out = vec![Vec::new(); n];
    for (i, e) in edges.iter().enumerate() {
        if e.from != e.to {
            out[e.from].push(i);
        }
    }
    let mut reversed = vec![false; edges.len()];
    // 0: not seen, 1: on the stack, 2: done.
    let mut state = vec![0u8; n];
    for root in 0..n {
        if state[root] != 0 {
            continue;
        }
        state[root] = 1;
        let mut stack = vec![(root, 0usize)];
        while let Some((v, k)) = stack.last_mut() {
            let v = *v;
            if let Some(&i) = out[v].get(*k) {
                *k += 1;
                let t = edges[i].to;
                match state[t] {
                    0 => {
                        state[t] = 1;
                        stack.push((t, 0));
                    }
                    1 => reversed[i] = true,
                    _ => {}
                }
            } else {
                state[v] = 2;
                stack.pop();
            }
        }
    }
    reversed
}

/// Phase 2: ranks for the nodes of `dag`, which has no cycles: the longest path
/// to each, then each source pulled down to just above what it points at.
fn rank(vs: &mut [V], dag: &[(usize, usize, usize, usize)]) {
    let n = vs.len();
    let mut out: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n];
    let mut indeg = vec![0; n];
    for &(a, b, len, _) in dag {
        out[a].push((b, len));
        indeg[b] += 1;
    }
    let has_pred: Vec<bool> = indeg.iter().map(|&d| d > 0).collect();
    let mut topo: Vec<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    let mut k = 0;
    while k < topo.len() {
        let v = topo[k];
        k += 1;
        for &(t, len) in &out[v] {
            vs[t].rank = vs[t].rank.max(vs[v].rank + len);
            indeg[t] -= 1;
            if indeg[t] == 0 {
                topo.push(t);
            }
        }
    }
    for &v in topo.iter().rev() {
        if has_pred[v] {
            continue;
        }
        if let Some(r) = out[v].iter().map(|&(t, len)| vs[t].rank - len).min() {
            vs[v].rank = r;
        }
    }
    let low = vs.iter().map(|v| v.rank).min().unwrap_or(0);
    for v in vs.iter_mut() {
        v.rank -= low;
    }
}

/// Phase 4: the vertices of each rank, left to right.
fn order(vs: &[V], up: &[Vec<usize>], down: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let max_rank = vs.iter().map(|v| v.rank).max().unwrap_or(0);
    // Start in depth-first order, so each chain's vertices start together.
    let mut ranks: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    let mut seen = vec![false; vs.len()];
    for root in 0..vs.len() {
        if seen[root] {
            continue;
        }
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            if std::mem::replace(&mut seen[v], true) {
                continue;
            }
            ranks[vs[v].rank].push(v);
            stack.extend(down[v].iter().rev());
        }
    }
    let mut pos = vec![0usize; vs.len()];
    let set_pos = |ranks: &[Vec<usize>], pos: &mut [usize]| {
        for rank in ranks {
            for (i, &v) in rank.iter().enumerate() {
                pos[v] = i;
            }
        }
    };
    set_pos(&ranks, &mut pos);

    let mut best = (crossings(&ranks, down, &pos), ranks.clone());
    // Bigger graphs get fewer sweeps.
    let sweeps = if vs.len() > 1500 { 4 } else { 16 };
    for sweep in 0..sweeps {
        if best.0 == 0 {
            break;
        }
        let downward = sweep % 2 == 0;
        let order: Vec<usize> = if downward {
            (1..=max_rank).collect()
        } else {
            (0..max_rank).rev().collect()
        };
        for r in order {
            let side = if downward { up } else { down };
            let mut keyed: Vec<(f32, usize)> = ranks[r]
                .iter()
                .map(|&v| {
                    let ns = &side[v];
                    let key = if ns.is_empty() {
                        pos[v] as f32
                    } else {
                        ns.iter().map(|&u| pos[u] as f32).sum::<f32>() / ns.len() as f32
                    };
                    (key, v)
                })
                .collect();
            keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
            ranks[r] = keyed.into_iter().map(|(_, v)| v).collect();
            for (i, &v) in ranks[r].iter().enumerate() {
                pos[v] = i;
            }
        }
        transpose(&mut ranks, &mut pos, up, down);
        let c = crossings(&ranks, down, &pos);
        if c < best.0 {
            best = (c, ranks.clone());
        }
    }
    best.1
}

/// How many pairs of edge segments cross.
fn crossings(ranks: &[Vec<usize>], down: &[Vec<usize>], pos: &[usize]) -> usize {
    let mut total = 0;
    for rank in ranks {
        let segs: Vec<(usize, usize)> = rank
            .iter()
            .flat_map(|&u| down[u].iter().map(move |&w| (pos[u], pos[w])))
            .collect();
        for (i, a) in segs.iter().enumerate() {
            total += segs[i + 1..]
                .iter()
                .filter(|b| (a.0 < b.0 && a.1 > b.1) || (a.0 > b.0 && a.1 < b.1))
                .count();
        }
    }
    total
}

/// Swaps neighbours in a rank while that crosses fewer edges.
fn transpose(ranks: &mut [Vec<usize>], pos: &mut [usize], up: &[Vec<usize>], down: &[Vec<usize>]) {
    // Crossings among the edges of `u` and `v`, with `u` left of `v`.
    let cross = |u: usize, v: usize, pos: &[usize]| {
        [up, down]
            .iter()
            .map(|side| {
                side[u]
                    .iter()
                    .map(|&a| side[v].iter().filter(|&&b| pos[a] > pos[b]).count())
                    .sum::<usize>()
            })
            .sum::<usize>()
    };
    for _ in 0..8 {
        let mut improved = false;
        for rank in ranks.iter_mut() {
            for i in 1..rank.len() {
                let (u, v) = (rank[i - 1], rank[i]);
                if cross(v, u, pos) < cross(u, v, pos) {
                    rank.swap(i - 1, i);
                    pos[u] = i;
                    pos[v] = i - 1;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
}

/// Phase 5: each vertex's x: packed in order at first, then moved towards the
/// mean of its neighbours above, then below, and so on, keeping the order
/// and spacing.
fn place_x(vs: &mut [V], ranks: &[Vec<usize>], up: &[Vec<usize>], down: &[Vec<usize>]) {
    let gap = |vs: &[V], a: usize, b: usize| {
        let sep = match (vs[a].solid, vs[b].solid) {
            (true, true) => NODE_SEP,
            (false, false) => EDGE_SEP,
            _ => (NODE_SEP + EDGE_SEP) / 2.0,
        };
        vs[a].right + sep + vs[b].left
    };
    for rank in ranks {
        let mut x = 0.0;
        for (i, &v) in rank.iter().enumerate() {
            if i > 0 {
                x += gap(vs, rank[i - 1], v);
            }
            vs[v].x = x;
        }
    }
    let max_rank = ranks.len().saturating_sub(1);
    // Alternate pulling towards the ranks above and below, then settle
    // between both.
    for pass in 0..10 {
        let (order, sides): (Vec<usize>, Vec<&[Vec<usize>]>) = match pass {
            8.. => ((0..=max_rank).collect(), vec![up, down]),
            p if p % 2 == 0 => ((1..=max_rank).collect(), vec![up]),
            _ => ((0..max_rank).rev().collect(), vec![down]),
        };
        for r in order {
            let rank = &ranks[r];
            if rank.is_empty() {
                continue;
            }
            let targets: Vec<f32> = rank
                .iter()
                .map(|&v| {
                    let (sum, count) = sides
                        .iter()
                        .flat_map(|side| side[v].iter())
                        .fold((0.0, 0), |(s, c), &u| (s + vs[u].x, c + 1));
                    if count == 0 {
                        vs[v].x
                    } else {
                        sum / count as f32
                    }
                })
                .collect();
            let gaps: Vec<f32> = rank.windows(2).map(|w| gap(vs, w[0], w[1])).collect();
            for (&v, x) in rank.iter().zip(fit(&targets, &gaps)) {
                vs[v].x = x;
            }
        }
    }
}

/// The positions closest (least squares) to `targets`, in their order,
/// with `x[i + 1] - x[i] >= gaps[i]`: the gaps are taken out, the
/// pool-adjacent-violators algorithm makes the rest non-decreasing, and
/// the gaps are put back.
fn fit(targets: &[f32], gaps: &[f32]) -> Vec<f32> {
    let mut offset = vec![0.0; targets.len()];
    for i in 1..targets.len() {
        offset[i] = offset[i - 1] + gaps[i - 1];
    }
    // Blocks of (sum, count).
    let mut blocks: Vec<(f32, usize)> = Vec::new();
    for (t, o) in targets.iter().zip(&offset) {
        blocks.push((t - o, 1));
        while let [.., (s1, c1), (s2, c2)] = blocks[..] {
            if s1 / c1 as f32 <= s2 / c2 as f32 {
                break;
            }
            blocks.pop();
            *blocks.last_mut().expect("two blocks") = (s1 + s2, c1 + c2);
        }
    }
    let mut out = Vec::with_capacity(targets.len());
    for (sum, count) in blocks {
        let mean = sum / count as f32;
        for _ in 0..count {
            out.push(mean + offset[out.len()]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(w: f32, h: f32) -> Size {
        Size { w, h }
    }

    fn e(from: usize, to: usize) -> EdgeIn {
        EdgeIn {
            from,
            to,
            min_len: 1,
            label: None,
        }
    }

    fn labelled(from: usize, to: usize) -> EdgeIn {
        EdgeIn {
            label: Some(sq(30.0, 14.0)),
            ..e(from, to)
        }
    }

    /// Asserts no two nodes overlap and all lie within the layout.
    fn check(l: &Layout, sizes: &[Size]) {
        let rect = |i: usize| {
            let (c, s) = (l.nodes[i], sizes[i]);
            (
                c.x - s.w / 2.0,
                c.y - s.h / 2.0,
                c.x + s.w / 2.0,
                c.y + s.h / 2.0,
            )
        };
        for i in 0..sizes.len() {
            let a = rect(i);
            let eps = 0.01;
            assert!(
                a.0 >= -eps && a.1 >= -eps && a.2 <= l.width + eps && a.3 <= l.height + eps,
                "node {i} {a:?} outside {} × {}",
                l.width,
                l.height
            );
            for j in i + 1..sizes.len() {
                let b = rect(j);
                let apart =
                    a.2 <= b.0 + eps || b.2 <= a.0 + eps || a.3 <= b.1 + eps || b.3 <= a.1 + eps;
                assert!(apart, "nodes {i} {a:?} and {j} {b:?} overlap");
            }
        }
        for (r, route) in l.edges.iter().enumerate() {
            for p in route.points.iter().chain(&route.label) {
                assert!(p.x.is_finite() && p.y.is_finite(), "edge {r}: {p:?}");
            }
        }
    }

    fn sizes(n: usize) -> Vec<Size> {
        (0..n)
            .map(|i| sq(40.0 + 10.0 * (i % 3) as f32, 30.0))
            .collect()
    }

    #[test]
    fn empty_and_single() {
        let l = layout(Direction::TopDown, &[], &[]).unwrap();
        assert_eq!((l.width, l.height), (0.0, 0.0));
        assert!(l.nodes.is_empty());

        let s = [sq(50.0, 20.0)];
        let l = layout(Direction::LeftRight, &s, &[]).unwrap();
        assert_eq!((l.width, l.height), (50.0, 20.0));
        assert_eq!(l.nodes, [Point { x: 25.0, y: 10.0 }]);
    }

    #[test]
    fn edges_go_down_a_rank() {
        // A diamond with a shortcut, and a longer edge.
        let s = sizes(5);
        let edges = [e(0, 1), e(0, 2), e(1, 3), e(2, 3), e(0, 3), e(3, 4)];
        let l = layout(Direction::TopDown, &s, &edges).unwrap();
        check(&l, &s);
        for (edge, route) in edges.iter().zip(&l.edges) {
            let (a, b) = (l.nodes[edge.from], l.nodes[edge.to]);
            assert!(b.y > a.y, "{edge:?}");
            assert_eq!(route.points.first(), Some(&a));
            assert_eq!(route.points.last(), Some(&b));
            // Bends go steadily down.
            assert!(route.points.windows(2).all(|w| w[1].y > w[0].y));
        }
        // 0 → 3 spans two ranks, so it bends once.
        assert_eq!(l.edges[4].points.len(), 3);
        // Ranks line up.
        assert_eq!(l.nodes[1].y, l.nodes[2].y);
    }

    #[test]
    fn min_len_adds_ranks() {
        let s = sizes(3);
        let mut long = e(0, 2);
        long.min_len = 3;
        let l = layout(Direction::TopDown, &s, &[e(0, 1), long]).unwrap();
        check(&l, &s);
        assert_eq!(l.edges[1].points.len(), 4);
        assert!(l.nodes[2].y > l.nodes[1].y);
    }

    #[test]
    fn labels_sit_between_their_ends() {
        let s = sizes(3);
        let edges = [labelled(0, 1), e(1, 2), labelled(2, 0)];
        for dir in [Direction::TopDown, Direction::LeftRight] {
            let l = layout(dir, &s, &edges).unwrap();
            check(&l, &s);
            for (edge, route) in edges.iter().zip(&l.edges) {
                let (a, b) = (l.nodes[edge.from], l.nodes[edge.to]);
                match edge.label {
                    Some(_) => {
                        let m = route.label.expect("a label point");
                        let between = |m: f32, a: f32, b: f32| m > a.min(b) && m < a.max(b);
                        if dir == Direction::TopDown {
                            assert!(between(m.y, a.y, b.y), "{dir:?} {edge:?}: {m:?}");
                        } else {
                            assert!(between(m.x, a.x, b.x), "{dir:?} {edge:?}: {m:?}");
                        }
                        assert!(route.points.contains(&m));
                    }
                    None => assert_eq!(route.label, None),
                }
            }
        }
    }

    #[test]
    fn labels_dont_overlap_nodes() {
        let s = sizes(4);
        let edges = [
            labelled(0, 1),
            labelled(0, 2),
            labelled(0, 3),
            labelled(1, 3),
        ];
        let l = layout(Direction::TopDown, &s, &edges).unwrap();
        let mut all = s.clone();
        let mut with_labels = l.clone();
        for (edge, route) in edges.iter().zip(&l.edges) {
            all.push(edge.label.unwrap());
            with_labels.nodes.push(route.label.unwrap());
        }
        check(&with_labels, &all);
    }

    #[test]
    fn cycles_and_self_loops() {
        let s = sizes(4);
        let mut self_loop = labelled(2, 2);
        self_loop.label = Some(sq(40.0, 14.0));
        let edges = [
            e(0, 1),
            e(1, 2),
            e(2, 0),
            self_loop,
            e(3, 3),
            e(2, 3),
            e(3, 2),
        ];
        let l = layout(Direction::TopDown, &s, &edges).unwrap();
        check(&l, &s);
        assert!(l.edges[3].points.is_empty());
        assert!(l.edges[4].points.is_empty());
        // A reversed edge still runs from its source to its target.
        assert_eq!(l.edges[2].points.first(), Some(&l.nodes[2]));
        assert_eq!(l.edges[2].points.last(), Some(&l.nodes[0]));
        // The loop's label is right of node 2 and within the layout.
        let label = l.edges[3].label.unwrap();
        assert!(label.x > l.nodes[2].x + s[2].w / 2.0 + LOOP);
        assert!(label.x + 20.0 <= l.width + 0.01);
    }

    #[test]
    fn disconnected_parts_sit_side_by_side() {
        let s = sizes(6);
        let edges = [e(0, 1), e(2, 3), e(4, 4)];
        let l = layout(Direction::TopDown, &s, &edges).unwrap();
        check(&l, &s);
        assert_eq!(l.nodes[0].y, l.nodes[2].y);
        assert_eq!(l.nodes[5].y, l.nodes[0].y);
    }

    #[test]
    fn deterministic() {
        let s = sizes(12);
        let edges: Vec<EdgeIn> = (0..30).map(|i| e((i * 7) % 12, (i * 5 + 3) % 12)).collect();
        let a = layout(Direction::TopDown, &s, &edges).unwrap();
        check(&a, &s);
        for _ in 0..3 {
            assert_eq!(layout(Direction::TopDown, &s, &edges).unwrap(), a);
        }
    }

    #[test]
    fn directions_transpose_and_flip() {
        let s = sizes(5);
        let edges = [e(0, 1), labelled(0, 2), e(1, 3), e(2, 3), e(3, 4), e(4, 4)];
        let td = layout(Direction::TopDown, &s, &edges).unwrap();
        // LR of the transposed sizes is TD transposed.
        let t: Vec<Size> = s.iter().map(|s| sq(s.h, s.w)).collect();
        let transposed_edges: Vec<EdgeIn> = edges
            .iter()
            .map(|e| EdgeIn {
                label: e.label.map(|l| sq(l.h, l.w)),
                ..*e
            })
            .collect();
        let lr = layout(Direction::LeftRight, &t, &transposed_edges).unwrap();
        check(&lr, &t);
        assert_eq!((lr.width, lr.height), (td.height, td.width));
        let swap = |p: &Point| Point { x: p.y, y: p.x };
        assert_eq!(lr.nodes, td.nodes.iter().map(swap).collect::<Vec<_>>());
        for (a, b) in lr.edges.iter().zip(&td.edges) {
            assert_eq!(a.points, b.points.iter().map(swap).collect::<Vec<_>>());
            assert_eq!(a.label, b.label.as_ref().map(swap));
        }
        let bt = layout(Direction::BottomUp, &s, &edges).unwrap();
        check(&bt, &s);
        for (a, b) in bt.nodes.iter().zip(&td.nodes) {
            assert_eq!((a.x, a.y), (b.x, td.height - b.y));
        }
        let rl = layout(Direction::RightLeft, &t, &transposed_edges).unwrap();
        for (a, b) in rl.nodes.iter().zip(&lr.nodes) {
            assert_eq!((a.x, a.y), (lr.width - b.x, b.y));
        }
    }

    #[test]
    fn crossings_are_avoided() {
        // Two parents each with two children, listed crossed: 0 → 3, 1 → 2.
        let s = sizes(4);
        let edges = [e(0, 3), e(1, 2)];
        let l = layout(Direction::TopDown, &s, &edges).unwrap();
        let dx = |a: usize, b: usize| l.nodes[a].x - l.nodes[b].x;
        assert!(dx(0, 1).signum() == dx(3, 2).signum(), "{l:?}");

        // K(2,2) plus a chain under it lays out without crossings.
        let s = sizes(6);
        let edges = [e(0, 2), e(0, 3), e(1, 3), e(2, 4), e(3, 5), e(1, 5)];
        let l = layout(Direction::TopDown, &s, &edges).unwrap();
        check(&l, &s);
    }

    #[test]
    fn largest_flowchart() {
        // The parser's limits: 150 nodes and 300 edges, some long and
        // labelled, with cycles.
        let n = super::super::MAX_NODES;
        let s = sizes(n);
        let edges: Vec<EdgeIn> = (0..super::super::MAX_EDGES)
            .map(|i| {
                let from = (i * 37) % n;
                let to = (i * 11 + 5) % n;
                if i % 10 == 0 {
                    labelled(from, to)
                } else {
                    e(from, to)
                }
            })
            .collect();
        let l = layout(Direction::TopDown, &s, &edges).unwrap();
        check(&l, &s);
    }

    #[test]
    fn too_deep_or_bendy_is_an_error() {
        // A chain one rank too long.
        let s = sizes(MAX_RANKS + 1);
        let chain: Vec<EdgeIn> = (0..MAX_RANKS).map(|i| e(i, i + 1)).collect();
        let err = layout(Direction::TopDown, &s, &chain).unwrap_err();
        assert!(err.0.contains("too deep"), "{err:?}");
        assert!(layout(Direction::TopDown, &s[..MAX_RANKS], &chain[..MAX_RANKS - 1]).is_ok());
        // Labels double the ranks.
        let mut labelled_chain = chain[..MAX_RANKS / 2].to_vec();
        labelled_chain[0].label = Some(sq(10.0, 10.0));
        assert!(layout(Direction::TopDown, &s, &labelled_chain).is_err());
        // Huge lengths from a caller other than the parser.
        let mut long = e(0, 1);
        long.min_len = usize::MAX;
        assert!(layout(Direction::TopDown, &s[..2], &[long]).is_err());
        // Many long edges side by side.
        let s = sizes(120);
        let mut edges: Vec<EdgeIn> = (0..119).map(|i| e(i, i + 1)).collect();
        edges.extend((0..100).map(|i| e(i % 10, 119 - i % 10)));
        let err = layout(Direction::TopDown, &s, &edges).unwrap_err();
        assert!(err.0.contains("bend"), "{err:?}");
    }

    #[test]
    fn fit_keeps_order_and_gaps() {
        assert_eq!(fit(&[0.0, 0.0], &[10.0]), [-5.0, 5.0]);
        assert_eq!(fit(&[0.0, 100.0], &[10.0]), [0.0, 100.0]);
        let pooled = fit(&[50.0, 0.0, 0.0], &[10.0, 10.0]);
        for (x, want) in pooled.iter().zip([20.0 / 3.0, 50.0 / 3.0, 80.0 / 3.0]) {
            assert!((x - want).abs() < 1e-4, "{pooled:?}");
        }
        assert!(fit(&[], &[]).is_empty());
    }
}
