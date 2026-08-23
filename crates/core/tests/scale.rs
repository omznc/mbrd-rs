//! What the index costs, measured rather than assumed.
//!
//! Not a benchmark harness — a floor. The numbers printed here are what the
//! module docs claim, and the assertions are loose enough that a slow machine
//! or a debug build does not fail the suite, while still catching the thing
//! worth catching: a change that puts the whole board back inside a query.

use std::time::{Duration, Instant};

use mbrd_core::geometry::{point, Rect};
use mbrd_core::index::Grid;
use mbrd_core::model::{Item, ItemType, MAX_ITEMS};

fn a_full_board() -> Vec<Item> {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32
    };
    (0..MAX_ITEMS)
        .map(|i| {
            let mut item = Item::new(format!("i{i}"), ItemType::Image);
            item.x = next() * 30_000.0 - 15_000.0;
            item.y = next() * 30_000.0 - 15_000.0;
            item.w = 120.0 + next() * 400.0;
            item.h = 120.0 + next() * 400.0;
            item
        })
        .collect()
}

#[test]
fn a_screenful_of_a_full_board_costs_the_screenful() {
    let items = a_full_board();

    let built = Instant::now();
    let grid = Grid::build(&items);
    let build = built.elapsed();

    // A 1600x900 window at 100%, panned across the board. This is the query
    // that happens on every frame of every pan, so it is the one that matters.
    let mut out = Vec::new();
    let started = Instant::now();
    let rounds = 200;
    let mut seen = 0usize;
    for i in 0..rounds {
        let cx = -14_000.0 + i as f32 * 140.0;
        grid.in_rect(Rect::centred(cx, 0.0, 1_600.0, 900.0), &mut out);
        seen += out.len();
    }
    let per_query = started.elapsed() / rounds;

    // And the same question asked the way it used to be.
    let started = Instant::now();
    let mut scanned = 0usize;
    for i in 0..rounds {
        let cx = -14_000.0 + i as f32 * 140.0;
        let window = Rect::centred(cx, 0.0, 1_600.0, 900.0);
        scanned += items.iter().filter(|it| Rect::of_item(it).intersects(&window)).count();
    }
    let per_scan = started.elapsed() / rounds;

    println!(
        "{} items · build {build:?} · query {per_query:?} · scan {per_scan:?} · {} found",
        items.len(),
        seen / rounds as usize,
    );
    assert_eq!(seen, scanned, "the index and the scan disagree about the board");
    assert!(
        per_query * 4 < per_scan,
        "a windowed query ({per_query:?}) should be far under a whole-board scan ({per_scan:?})"
    );
}

#[test]
fn a_press_on_a_full_board_costs_the_pointer() {
    let items = a_full_board();
    let grid = Grid::build(&items);
    let mut out = Vec::new();

    let started = Instant::now();
    let rounds = 2_000;
    for i in 0..rounds {
        grid.at(point(-14_000.0 + i as f32 * 14.0, i as f32 * 13.0), &mut out);
    }
    let per_press = started.elapsed() / rounds;
    println!("{} items · press {per_press:?}", items.len());
    // Generous by three orders of magnitude against a scan of twenty thousand,
    // and still nowhere near a frame.
    assert!(per_press.as_micros() < 200, "a press took {per_press:?}");
}

/// A screenful of connections has to settle inside a frame, or the rule that
/// nothing is routed while anything is moving buys nothing — the pause when the
/// hand comes off would be the thing people noticed instead.
///
/// Routed against a busy board rather than an empty one, because an empty board
/// is the case the string-pull collapses to one segment and measures nothing.
#[test]
fn a_screenful_of_lines_settles_inside_a_frame() {
    use mbrd_core::geometry::Rect;
    use mbrd_core::route::{ends, route, Ask, Link};

    // Sixty cards in a loose grid, and forty lines across them: more than a
    // moodboard usually has in one window, which is the point of a ceiling.
    let cards: Vec<Rect> = (0..60)
        .map(|n| Rect::centred((n % 10) as f32 * 260.0, (n / 10) as f32 * 220.0, 180.0, 140.0))
        .collect();
    let links: Vec<Link> = (0..40).map(|n| Link { a: n % 60, b: (n * 7 + 13) % 60 }).collect();
    let pairs = ends(&cards, &links);

    let start = Instant::now();
    let mut settled: Vec<Vec<mbrd_core::geometry::Point>> = Vec::new();
    for (n, link) in links.iter().enumerate() {
        let walls: Vec<Rect> = cards
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != link.a && *i != link.b)
            .map(|(_, r)| *r)
            .collect();
        let ask = Ask::new(&walls, &settled);
        let (from, to) = pairs[n];
        settled.push(route(&cards[link.a], &cards[link.b], from, to, &ask));
    }
    let took = start.elapsed();

    assert_eq!(settled.len(), 40);
    println!("forty routes over sixty cards: {took:?}");
    assert!(
        took < Duration::from_millis(120),
        "routing a screenful took {took:?}, which is a visible pause when the hand comes off"
    );
}
