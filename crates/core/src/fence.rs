//! Which cards belong to which fence, worked out rather than looked up.
//!
//! A fence is an item of type `fence` — a labelled rectangle, with the cards
//! inside it belonging to it. **There is no member list, and deliberately
//! nowhere to put one.** A card is in a fence when its centre falls inside the
//! fence's rectangle, which is a fact about geometry the file already carries.
//! So there is no array to disagree with the coordinates beside it, and no way
//! for a file to claim a grouping its own numbers contradict.
//!
//! That is the whole design, and everything below is a consequence of it:
//!
//! - **The centre, not the box.** A card half in and half out is in, or out,
//!   depending on which way it leans — a single answer rather than "partly".
//!   Using the box would make a large card belong to every fence it grazes.
//! - **The smallest containing fence wins.** Fences nest, and a card inside two
//!   of them belongs to the inner one, which is what nesting means.
//! - **A fence may only be held by one of strictly greater area.** That is what
//!   keeps the containment chain a strict order: two fences of identical size
//!   lying on each other cannot each be the other's parent, so there is no
//!   cycle for [`Fences::chain`] to walk forever.
//!
//! [`Item::fence`](crate::model::Item::fence) — `meta.fence` in the file — is a
//! *record* of this measurement and never the authority for it. It is kept for
//! two reasons the measurement cannot cover. A pixel of drift across a save
//! must not lose a grouping somebody plainly made, which is what `SLACK` below
//! answers. And membership is measured on **Desktop** geometry only: on Mobile
//! a fence is drawn as a full-width band with its members packed *beneath* it,
//! so nothing is geometrically inside its own fence there and measuring would
//! find every fence empty. This build is Desktop-only and so measures; the
//! stamp is written anyway, because the file is read by builds that are not.

use std::collections::HashMap;

use crate::geometry::{point, Rect};
use crate::model::{Item, ItemType};

/// How far outside a fence a card may sit and still be held by it, when the
/// file already said it was.
///
/// This is not a fudge factor for sloppy geometry — measurement alone is exact
/// and stable. It is for the one case measurement genuinely cannot see: a card
/// whose centre sat on a fence's edge, was written to the file, and came back a
/// float's breadth on the other side. Without it, saving and reopening a board
/// silently dissolves that grouping, and nothing the author did caused it.
///
/// One world unit, so it can only ever rescue a card that was already touching
/// the line. A card genuinely dragged out leaves by much more than this.
const SLACK: f32 = 1.0;

/// Who holds what, for one board's Desktop geometry.
///
/// Built by [`Fences::measure`] and then read; nothing here mutates a board.
/// Rebuild it when the board changes rather than updating it, for the same
/// reason [`Grid`](crate::index::Grid) is rebuilt: it is cheap, and a structure
/// that can be stale is a structure that will be.
#[derive(Debug, Clone, Default)]
pub struct Fences {
    /// Every item that is inside something, to the id of the fence holding it.
    /// Fences appear here too, as members of their parents.
    owner: HashMap<String, String>,
}

impl Fences {
    /// Measure the whole board.
    ///
    /// Costs one pass to collect the fences and one pass per item over them,
    /// which is linear in the items and quadratic only in the *fences*. A board
    /// with twenty thousand cards and a dozen fences is twenty thousand times
    /// twelve; a board with twenty thousand fences is a board that has other
    /// problems.
    pub fn measure(items: &[Item]) -> Self {
        let pens: Vec<(&str, Rect, f32)> = items
            .iter()
            .filter(|it| it.kind == ItemType::Fence)
            .map(|it| {
                let r = Rect::of_item(it);
                (it.id.as_str(), r, r.width() * r.height())
            })
            .collect();
        if pens.is_empty() {
            return Self::default();
        }

        let mut owner = HashMap::new();
        for item in items {
            let mine = if item.kind == ItemType::Fence {
                // A fence is held only by one of strictly greater area, which
                // is what makes the chain a strict order rather than a graph.
                let area = item.w * item.h;
                smallest(&pens, item, |_, a| a > area)
            } else {
                smallest(&pens, item, |_, _| true)
            };
            if let Some(id) = mine {
                owner.insert(item.id.clone(), id.to_string());
            }
        }
        Self { owner }
    }

    /// The fence directly holding this item, if any.
    pub fn owner_of(&self, id: &str) -> Option<&str> {
        self.owner.get(id).map(String::as_str)
    }

    /// Whether any fence holds anything. A board of loose cards answers `false`
    /// and every caller can stop there.
    pub fn is_empty(&self) -> bool {
        self.owner.is_empty()
    }

    /// The fences holding this item, innermost first, out to the outermost.
    ///
    /// Bounded by the number of fences on the board rather than trusted to
    /// terminate: the strictly-greater-area rule means it always does, and a
    /// board that arrives from somewhere else does not get to decide whether
    /// this function returns.
    pub fn chain(&self, id: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut at = id;
        while let Some(up) = self.owner_of(at) {
            if out.contains(&up) {
                break;
            }
            out.push(up);
            at = up;
            if out.len() > self.owner.len() {
                break;
            }
        }
        out
    }

    /// The ids this fence holds directly, in the order the items are in.
    ///
    /// Takes the item list rather than remembering an order of its own, so that
    /// what comes back is in board order — which is the order a caller drawing
    /// or moving them wants, and the order [`Fences`] has no business inventing.
    pub fn members_of<'a>(&self, fence: &str, items: &'a [Item]) -> Vec<&'a Item> {
        items.iter().filter(|it| self.owner_of(&it.id) == Some(fence)).collect()
    }

    /// Everything this fence holds, however deeply — the members of its
    /// members, and so on.
    ///
    /// This is what a drag on a fence takes hold of. A fence that moves and
    /// leaves its nested fence's cards behind has not moved a grouping, it has
    /// torn one, so the transitive set is the one that matters at a gesture.
    pub fn contents<'a>(&self, fence: &str, items: &'a [Item]) -> Vec<&'a Item> {
        items.iter().filter(|it| self.chain(&it.id).contains(&fence)).collect()
    }
}

/// The smallest fence containing this item's centre that passes `allow`.
///
/// `allow` receives the candidate's id and area, and exists for the one caller
/// that needs it: a fence looking for its own parent, which may not settle for
/// one of equal area.
fn smallest<'a>(
    pens: &[(&'a str, Rect, f32)],
    item: &Item,
    allow: impl Fn(&str, f32) -> bool,
) -> Option<&'a str> {
    let centre = point(item.x, item.y);
    let recorded = item.fence();
    let mut best: Option<(&str, f32)> = None;
    for (id, box_, area) in pens {
        if *id == item.id || !allow(id, *area) {
            continue;
        }
        // The slack applies only to the fence the file already named. Anywhere
        // else it would be a rule that a card near a line is inside it, which
        // is not the rule.
        let reach = if recorded == Some(*id) { box_.inflate(SLACK) } else { *box_ };
        if !reach.contains(centre) {
            continue;
        }
        if best.is_none_or(|(_, had)| *area < had) {
            best = Some((id, *area));
        }
    }
    best.map(|(id, _)| id)
}

/// Write the measurement onto the items, as the file wants it.
///
/// Called at the file boundary and nowhere else. The key is written only where
/// there is a fence to name — a `null` on every card would be noise, and the
/// absence already means "loose".
pub fn stamp(items: &mut [Item]) {
    let fences = Fences::measure(items);
    for item in items.iter_mut() {
        match fences.owner_of(&item.id) {
            Some(id) => {
                item.meta.insert("fence".into(), serde_json::Value::String(id.to_string()));
            }
            None => {
                item.meta.remove("fence");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(id: &str, kind: ItemType, x: f32, y: f32, w: f32, h: f32) -> Item {
        let mut item = Item::new(id, kind);
        item.x = x;
        item.y = y;
        item.w = w;
        item.h = h;
        item
    }

    fn pen(id: &str, x: f32, y: f32, w: f32, h: f32) -> Item {
        at(id, ItemType::Fence, x, y, w, h)
    }

    fn card(id: &str, x: f32, y: f32) -> Item {
        at(id, ItemType::Image, x, y, 40.0, 40.0)
    }

    #[test]
    fn a_card_belongs_to_the_fence_its_centre_is_in() {
        let items = vec![pen("f", 0.0, 0.0, 400.0, 400.0), card("a", 50.0, 50.0)];
        let fences = Fences::measure(&items);
        assert_eq!(fences.owner_of("a"), Some("f"));
    }

    #[test]
    fn a_card_hanging_over_the_edge_is_in_or_out_and_never_partly() {
        // Centre inside, most of the card outside: in.
        let mut items = vec![pen("f", 0.0, 0.0, 100.0, 100.0), card("a", 45.0, 0.0)];
        assert_eq!(Fences::measure(&items).owner_of("a"), Some("f"));
        // Nudge the centre past the line and it is out, even though most of the
        // card is still over the fence.
        items[1].x = 55.0;
        assert_eq!(Fences::measure(&items).owner_of("a"), None);
    }

    #[test]
    fn the_smallest_fence_holding_a_card_is_the_one_that_holds_it() {
        let items = vec![
            pen("big", 0.0, 0.0, 800.0, 800.0),
            pen("small", 0.0, 0.0, 200.0, 200.0),
            card("a", 10.0, 10.0),
        ];
        let fences = Fences::measure(&items);
        assert_eq!(fences.owner_of("a"), Some("small"));
        // And the inner fence is itself held by the outer one.
        assert_eq!(fences.owner_of("small"), Some("big"));
        assert_eq!(fences.chain("a"), vec!["small", "big"]);
    }

    #[test]
    fn two_fences_the_same_size_lying_on_each_other_do_not_hold_each_other() {
        // The strictly-greater-area rule, and the cycle it is there to prevent:
        // without it each of these is inside the other and `chain` never ends.
        let items = vec![pen("one", 0.0, 0.0, 300.0, 300.0), pen("two", 0.0, 0.0, 300.0, 300.0)];
        let fences = Fences::measure(&items);
        assert_eq!(fences.owner_of("one"), None);
        assert_eq!(fences.owner_of("two"), None);
        assert!(fences.chain("one").is_empty());
    }

    #[test]
    fn a_fence_takes_everything_under_it_and_not_just_its_own_members() {
        let items = vec![
            pen("outer", 0.0, 0.0, 800.0, 800.0),
            pen("inner", 100.0, 100.0, 200.0, 200.0),
            card("deep", 110.0, 110.0),
            card("shallow", -300.0, -300.0),
            card("away", 900.0, 900.0),
        ];
        let fences = Fences::measure(&items);
        let direct: Vec<&str> =
            fences.members_of("outer", &items).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(direct, vec!["inner", "shallow"]);
        let all: Vec<&str> =
            fences.contents("outer", &items).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(all, vec!["inner", "deep", "shallow"]);
    }

    #[test]
    fn a_grouping_survives_a_hair_of_drift_across_a_save() {
        // The centre came back a shade outside the fence it was saved inside.
        let mut a = card("a", 0.0, 100.4);
        a.meta.insert("fence".into(), serde_json::Value::String("f".into()));
        let items = vec![pen("f", 0.0, 0.0, 200.0, 200.0), a];
        assert_eq!(Fences::measure(&items).owner_of("a"), Some("f"));

        // But the record is not a claim. A card dragged properly out is out,
        // whatever the file remembers.
        let mut items = items;
        items[1].y = 400.0;
        assert_eq!(Fences::measure(&items).owner_of("a"), None);
    }

    #[test]
    fn the_record_only_rescues_the_fence_it_names() {
        // Slack near *another* fence would be a rule that a card beside a line
        // is inside it, and that is not the rule.
        let mut a = card("a", 0.0, 100.4);
        a.meta.insert("fence".into(), serde_json::Value::String("elsewhere".into()));
        let items = vec![pen("f", 0.0, 0.0, 200.0, 200.0), a];
        assert_eq!(Fences::measure(&items).owner_of("a"), None);
    }

    #[test]
    fn the_stamp_says_what_the_measurement_said() {
        let mut items = vec![
            pen("f", 0.0, 0.0, 400.0, 400.0),
            card("in", 10.0, 10.0),
            card("out", 900.0, 900.0),
        ];
        // A stale record on a card that is plainly outside is cleared rather
        // than believed — otherwise a fence deleted long ago haunts the file.
        items[2].meta.insert("fence".into(), serde_json::Value::String("ghost".into()));
        stamp(&mut items);
        assert_eq!(items[1].fence(), Some("f"));
        assert_eq!(items[2].fence(), None);
        assert!(!items[2].meta.contains_key("fence"));
    }

    #[test]
    fn a_board_with_no_fences_costs_nothing_to_ask_about() {
        let items = vec![card("a", 0.0, 0.0), card("b", 10.0, 10.0)];
        let fences = Fences::measure(&items);
        assert!(fences.is_empty());
        assert_eq!(fences.owner_of("a"), None);
    }
}
