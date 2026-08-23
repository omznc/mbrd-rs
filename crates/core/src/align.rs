//! Lining things up, spacing them out, and pushing them off each other.
//!
//! Three operations that are the same shape and are therefore one module: each
//! takes the cards somebody selected and hands back where they should go. None
//! of them touches a board — the caller opens a step and writes the answer
//! through the door, which is what makes all of this testable without one.
//!
//! **Everything here measures the tilted box**, [`Rect::of_item`], and not the
//! card's own width. A card turned thirty degrees genuinely reaches further
//! than it is wide, and "align left" means the left of what you can see. Lining
//! up the untilted box would leave a row of turned cards visibly ragged along
//! the very edge the command was asked to make straight.
//!
//! **What comes back is only what moved.** A card already on the line is not in
//! the result, which matters at the door: an edit that touches nothing records
//! no step, so "align left" on an already-aligned row does not fill the undo
//! ledger with rungs that undo to the same picture.

use crate::geometry::Rect;
use crate::model::Item;

/// Which edge, or which centre line, is being made to agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    CentreX,
    Right,
    Top,
    Middle,
    Bottom,
}

impl Edge {
    /// Every edge, in the order a menu wants to offer them: the three across,
    /// then the three down.
    ///
    /// Here rather than written out again wherever a row of them is built, for
    /// the reason the format's own enums keep their list — see
    /// `model::named_enum`. An edge added here is offered everywhere the
    /// moment it exists, and `alignment_offers_every_edge_there_is` fails to
    /// compile if this list is not the whole of them.
    pub const ALL: [Self; 6] =
        [Self::Left, Self::CentreX, Self::Right, Self::Top, Self::Middle, Self::Bottom];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::CentreX => "centre",
            Self::Right => "right",
            Self::Top => "top",
            Self::Middle => "middle",
            Self::Bottom => "bottom",
        }
    }
}

/// Which way things are being spread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// Both axes. See [`Edge::ALL`] for why the list lives on the type.
    pub const ALL: [Self; 2] = [Self::Horizontal, Self::Vertical];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
        }
    }
}

/// Where one card should end up: its new **centre**, in world units.
#[derive(Debug, Clone, PartialEq)]
pub struct Move {
    pub id: String,
    pub x: f32,
    pub y: f32,
}

/// Two cards in the same place are not "already aligned", they are the same
/// float twice. Below this, a move is not worth a step.
const NUDGE: f32 = 0.001;

fn moved(item: &Item, x: f32, y: f32) -> Option<Move> {
    if (x - item.x).abs() < NUDGE && (y - item.y).abs() < NUDGE {
        return None;
    }
    Some(Move { id: item.id.clone(), x, y })
}

/// Bring every card's named edge onto the same line.
///
/// The line is taken from the selection itself rather than from a nominated
/// card: the outermost edge for `Left`/`Right`/`Top`/`Bottom`, and the middle
/// of the whole group's box for the two centre lines. That means the command
/// has one obvious answer whatever order the cards were selected in — an
/// "align to the first one you clicked" rule reads as random the moment
/// somebody band-selects instead.
pub fn align(items: &[&Item], edge: Edge) -> Vec<Move> {
    if items.len() < 2 {
        return Vec::new();
    }
    let boxes: Vec<Rect> = items.iter().map(|it| Rect::of_item(it)).collect();
    let whole = boxes.iter().copied().reduce(|a, b| Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    });
    let Some(whole) = whole else { return Vec::new() };

    let mut out = Vec::new();
    for (item, box_) in items.iter().zip(&boxes) {
        // The centre is a half-extent in from the edge, and the half-extent is
        // the tilted box's, so a turned card lands where it looks like it does.
        let (hw, hh) = (box_.width() / 2.0, box_.height() / 2.0);
        let (x, y) = match edge {
            Edge::Left => (whole.x0 + hw, item.y),
            Edge::CentreX => (whole.centre().x, item.y),
            Edge::Right => (whole.x1 - hw, item.y),
            Edge::Bottom => (item.x, whole.y0 + hh),
            Edge::Middle => (item.x, whole.centre().y),
            Edge::Top => (item.x, whole.y1 - hh),
        };
        if let Some(m) = moved(item, x, y) {
            out.push(m);
        }
    }
    out
}

/// Put an equal gap between every neighbouring pair along one axis.
///
/// The two outermost cards do not move — they are the span, and moving them
/// would mean the command had to invent one. Everything between them is placed
/// so the *gaps* are equal, not the centres: three cards of very different
/// widths spaced by their centres look wrong in exactly the way somebody
/// reaches for this command to fix.
///
/// Fewer than three cards is not an error, it is a no-op: with two, the gap
/// between them is already the only gap there is.
pub fn distribute(items: &[&Item], axis: Axis) -> Vec<Move> {
    if items.len() < 3 {
        return Vec::new();
    }
    let mut ordered: Vec<(&&Item, Rect)> = items.iter().map(|it| (it, Rect::of_item(it))).collect();
    let along = |r: &Rect| if axis == Axis::Horizontal { r.centre().x } else { r.centre().y };
    let size = |r: &Rect| if axis == Axis::Horizontal { r.width() } else { r.height() };
    ordered.sort_by(|a, b| along(&a.1).total_cmp(&along(&b.1)));

    let first = &ordered[0].1;
    let last = &ordered[ordered.len() - 1].1;
    let (lo, hi) = if axis == Axis::Horizontal { (first.x0, last.x1) } else { (first.y0, last.y1) };
    let filled: f32 = ordered.iter().map(|(_, r)| size(r)).sum();
    // Negative when the cards overlap more than the span can hold. Left
    // negative on purpose: the result is an even overlap, which is at least
    // regular, rather than a pile at one end.
    let gap = (hi - lo - filled) / (ordered.len() - 1) as f32;

    let mut out = Vec::new();
    let mut edge = lo;
    for (item, box_) in &ordered {
        let centre = edge + size(box_) / 2.0;
        let (x, y) = if axis == Axis::Horizontal { (centre, item.y) } else { (item.x, centre) };
        if let Some(m) = moved(item, x, y) {
            out.push(m);
        }
        edge += size(box_) + gap;
    }
    out
}

/// How many passes the push-apart makes before it settles for what it has.
///
/// Relaxation converges quickly on a board and slowly on a pile, and the pile
/// is exactly where a person is most likely to hit the command. A ceiling means
/// the worst case is a board that is *better* rather than a window that stops
/// answering — and there is no arrangement this can leave worse than it found.
const PASSES: usize = 32;

/// Push overlapping cards off each other, leaving `gap` between them.
///
/// Relaxation rather than a packer: each pass finds every overlapping pair and
/// slides both of them apart along whichever axis they overlap by *least*,
/// which is the direction that costs the least movement and the one that keeps
/// a row a row. A packer would produce a tidier result and would also decide
/// the arrangement, which is not what somebody who has laid cards out by hand
/// and wants them to stop touching is asking for.
///
/// Cards do not move towards where they were — there is no spring here — so a
/// board that is already clear comes back empty rather than slightly stirred.
pub fn separate(items: &[&Item], gap: f32) -> Vec<Move> {
    if items.len() < 2 {
        return Vec::new();
    }
    // Worked in a scratch list of centres so a pass sees the previous pass's
    // result. Doing it against the original positions would push a card in a
    // pile of three the same way twice and leave two of them still touching.
    let mut at: Vec<(f32, f32)> = items.iter().map(|it| (it.x, it.y)).collect();
    let half: Vec<(f32, f32)> = items
        .iter()
        .map(|it| {
            let r = Rect::of_item(it);
            (r.width() / 2.0 + gap / 2.0, r.height() / 2.0 + gap / 2.0)
        })
        .collect();

    for _ in 0..PASSES {
        let mut touched = false;
        for i in 0..at.len() {
            for j in (i + 1)..at.len() {
                let dx = at[j].0 - at[i].0;
                let dy = at[j].1 - at[i].1;
                let reach_x = half[i].0 + half[j].0;
                let reach_y = half[i].1 + half[j].1;
                let over_x = reach_x - dx.abs();
                let over_y = reach_y - dy.abs();
                if over_x <= 0.0 || over_y <= 0.0 {
                    continue;
                }
                touched = true;
                if over_x < over_y {
                    // Two cards at exactly the same x have no side to be
                    // pushed to, so one is chosen — by index, so the same
                    // board always comes apart the same way.
                    let way = if dx.abs() < NUDGE {
                        if i < j {
                            -1.0
                        } else {
                            1.0
                        }
                    } else {
                        dx.signum()
                    };
                    let step = over_x / 2.0 * way;
                    at[i].0 -= step;
                    at[j].0 += step;
                } else {
                    let way = if dy.abs() < NUDGE {
                        if i < j {
                            -1.0
                        } else {
                            1.0
                        }
                    } else {
                        dy.signum()
                    };
                    let step = over_y / 2.0 * way;
                    at[i].1 -= step;
                    at[j].1 += step;
                }
            }
        }
        if !touched {
            break;
        }
    }

    items.iter().zip(&at).filter_map(|(item, (x, y))| moved(item, *x, *y)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemType;

    fn card(id: &str, x: f32, y: f32, w: f32, h: f32) -> Item {
        let mut item = Item::new(id, ItemType::Image);
        item.x = x;
        item.y = y;
        item.w = w;
        item.h = h;
        item
    }

    fn refs(items: &[Item]) -> Vec<&Item> {
        items.iter().collect()
    }

    fn placed(moves: &[Move], id: &str) -> (f32, f32) {
        let m = moves.iter().find(|m| m.id == id).expect("no move for {id}");
        (m.x, m.y)
    }

    #[test]
    fn aligning_left_puts_every_left_edge_on_the_leftmost_one() {
        let items = vec![card("a", 0.0, 0.0, 100.0, 50.0), card("b", 200.0, 80.0, 40.0, 50.0)];
        let moves = align(&refs(&items), Edge::Left);
        // `a` already has the leftmost edge, so it does not move and is not
        // in the result at all.
        assert_eq!(moves.len(), 1);
        // `b`'s left edge lands at -50, so its centre is a half-width in.
        assert_eq!(placed(&moves, "b"), (-30.0, 80.0));
    }

    #[test]
    fn aligning_on_one_axis_leaves_the_other_alone() {
        let items = vec![card("a", 0.0, 0.0, 100.0, 50.0), card("b", 7.0, 900.0, 40.0, 50.0)];
        let moves = align(&refs(&items), Edge::Bottom);
        assert_eq!(placed(&moves, "b").0, 7.0);
    }

    #[test]
    fn a_turned_card_lines_up_by_what_you_can_see_of_it() {
        let mut turned = card("b", 400.0, 0.0, 200.0, 40.0);
        turned.rot = 45.0;
        let items = vec![card("a", 0.0, 0.0, 100.0, 50.0), turned];
        let moves = align(&refs(&items), Edge::Left);
        // The tilted box is about 170 wide, so its centre sits ~85 right of the
        // line — not the 100 an untilted 200-wide card would have wanted.
        let (x, _) = placed(&moves, "b");
        let reach = Rect::of_item(&items[1]).width() / 2.0;
        assert!((x - (-50.0 + reach)).abs() < 0.01);
        assert!(x < 40.0, "an untilted measurement would have put it at 50");
    }

    #[test]
    fn a_row_already_aligned_asks_for_nothing() {
        let items = vec![card("a", 0.0, 0.0, 100.0, 50.0), card("a2", 0.0, 90.0, 100.0, 50.0)];
        assert!(align(&refs(&items), Edge::Left).is_empty());
        assert!(align(&refs(&items), Edge::CentreX).is_empty());
    }

    #[test]
    fn one_card_is_already_aligned_with_itself() {
        let items = vec![card("a", 3.0, 4.0, 100.0, 50.0)];
        assert!(align(&refs(&items), Edge::Right).is_empty());
    }

    #[test]
    fn distributing_equalises_the_gaps_and_not_the_centres() {
        // Widths 100, 20, 100: spacing by centres would leave the wide ones
        // nearly touching the narrow one on one side and far off on the other.
        let items = vec![
            card("a", 0.0, 0.0, 100.0, 50.0),
            card("b", 200.0, 0.0, 20.0, 50.0),
            card("c", 600.0, 0.0, 100.0, 50.0),
        ];
        let moves = distribute(&refs(&items), Axis::Horizontal);
        // Span is -50..650 = 700, filled is 220, so each of the two gaps is 240.
        // `b` runs from -50+100+240 = 290 to 310, centre 300.
        assert_eq!(placed(&moves, "b"), (300.0, 0.0));
        // The ends held still.
        assert!(!moves.iter().any(|m| m.id == "a" || m.id == "c"));
    }

    #[test]
    fn distributing_reads_the_order_off_the_board_not_the_selection() {
        // Handed in back to front. The answer is the same, because which cards
        // are the ends is a fact about where they are.
        let items = vec![
            card("c", 600.0, 0.0, 100.0, 50.0),
            card("b", 200.0, 0.0, 20.0, 50.0),
            card("a", 0.0, 0.0, 100.0, 50.0),
        ];
        let moves = distribute(&refs(&items), Axis::Horizontal);
        assert_eq!(placed(&moves, "b"), (300.0, 0.0));
    }

    #[test]
    fn two_cards_have_only_one_gap_and_it_is_already_even() {
        let items = vec![card("a", 0.0, 0.0, 100.0, 50.0), card("b", 900.0, 0.0, 100.0, 50.0)];
        assert!(distribute(&refs(&items), Axis::Horizontal).is_empty());
    }

    #[test]
    fn separating_a_pile_leaves_nothing_overlapping() {
        let items = vec![
            card("a", 0.0, 0.0, 100.0, 100.0),
            card("b", 10.0, 8.0, 100.0, 100.0),
            card("c", -6.0, 12.0, 100.0, 100.0),
        ];
        let moves = separate(&refs(&items), 12.0);
        let mut at: Vec<(f32, f32)> = items.iter().map(|i| (i.x, i.y)).collect();
        for m in &moves {
            let n = items.iter().position(|i| i.id == m.id).unwrap();
            at[n] = (m.x, m.y);
        }
        for i in 0..at.len() {
            for j in (i + 1)..at.len() {
                let apart_x = (at[j].0 - at[i].0).abs() >= 100.0 + 12.0 - 0.01;
                let apart_y = (at[j].1 - at[i].1).abs() >= 100.0 + 12.0 - 0.01;
                assert!(apart_x || apart_y, "{i} and {j} still touch: {at:?}");
            }
        }
    }

    #[test]
    fn separating_a_board_that_is_already_clear_stirs_nothing() {
        let items = vec![
            card("a", 0.0, 0.0, 100.0, 100.0),
            card("b", 400.0, 0.0, 100.0, 100.0),
            card("c", 0.0, 400.0, 100.0, 100.0),
        ];
        assert!(separate(&refs(&items), 12.0).is_empty());
    }

    #[test]
    fn two_cards_in_exactly_the_same_place_still_come_apart() {
        // No direction to push in, so one is picked — and picked the same way
        // every time, so the same board never comes apart two different ways.
        let items = vec![card("a", 0.0, 0.0, 100.0, 100.0), card("b", 0.0, 0.0, 100.0, 100.0)];
        let first = separate(&refs(&items), 10.0);
        let again = separate(&refs(&items), 10.0);
        assert_eq!(first, again);
        assert_eq!(first.len(), 2);
        // Both overlaps are the same depth, so which axis wins is a tie broken
        // by the comparison rather than by anything meaningful — what matters
        // is that they come apart on one of them, and always the same one.
        let apart = (first[0].x - first[1].x).abs().max((first[0].y - first[1].y).abs());
        assert!(apart >= 110.0 - 0.01, "{first:?}");
    }

    #[test]
    fn cards_slide_apart_the_short_way() {
        // Barely overlapping side to side and deeply overlapping top to bottom:
        // the cheap move is sideways, and a row stays a row.
        let items = vec![card("a", 0.0, 0.0, 100.0, 100.0), card("b", 96.0, 4.0, 100.0, 100.0)];
        let moves = separate(&refs(&items), 0.0);
        assert_eq!(moves.len(), 2);
        for m in &moves {
            let was = items.iter().find(|i| i.id == m.id).unwrap();
            assert_eq!(m.y, was.y, "nothing should have moved vertically");
        }
    }

    /// The list on the type is the whole of the enum, and stays that way.
    ///
    /// Two guards rather than one, because a list that is only ever iterated
    /// over cannot notice what was never put into it — which is exactly how
    /// the app's `Command::ALL` quietly lost three entries. The match fails to
    /// compile when an edge is added; the count then fails until it is added
    /// to `ALL` too. Neither alone is enough.
    #[test]
    fn every_edge_there_is_is_in_the_list_of_them() {
        for edge in Edge::ALL {
            match edge {
                Edge::Left
                | Edge::CentreX
                | Edge::Right
                | Edge::Top
                | Edge::Middle
                | Edge::Bottom => assert!(Edge::ALL.contains(&edge)),
            }
        }
        assert_eq!(Edge::ALL.len(), 6, "an edge was added to the enum and not to Edge::ALL");
    }

    #[test]
    fn every_axis_there_is_is_in_the_list_of_them() {
        for axis in Axis::ALL {
            match axis {
                Axis::Horizontal | Axis::Vertical => assert!(Axis::ALL.contains(&axis)),
            }
        }
        assert_eq!(Axis::ALL.len(), 2, "an axis was added to the enum and not to Axis::ALL");
    }
}
