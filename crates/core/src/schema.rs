//! `board.json`, in both directions.
//!
//! This is the app's whole contract with a document it did not necessarily
//! write. What arrives has been parsed but not validated — it may have been
//! hand-edited, truncated, produced by a build that no longer exists, or
//! produced by something else entirely — so **every field in it is a claim
//! rather than a fact**.
//!
//! The rule that shapes this whole module: [`normalize`] **cannot fail**. It
//! returns a `Board`, never a `Result`, and it degrades to a default one field
//! at a time rather than giving up half-way through a load. There is deliberately
//! no error type in here to be tempted by. [`serialize`] is the same promise
//! pointing the other way: what it returns is already held to the rules the
//! reader will apply to it.
//!
//! The two exist as a pair, and a round trip through them is the cheapest test
//! this crate has — see the bottom of the file.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

use crate::model::*;
use crate::viewport::{MAX_ZOOM, MIN_ZOOM};

// ---------------------------------------------------------------------------
// Reading whatever arrived
// ---------------------------------------------------------------------------

/// A whole board, built from whatever arrived, with no way to fail.
pub fn normalize(data: &Value) -> Board {
    let src = record(data);
    let mut board = Board { title: clean_title(src.get("title")), ..Board::default() };

    if let Some(view) = src.get("view").map(record) {
        let pan = view.get("pan").map(record).unwrap_or_default();
        board.view = View {
            pan_x: num(pan.get("x"), 0.0),
            pan_y: num(pan.get("y"), 0.0),
            // Clamped, because a saved zoom outside what the app can draw would
            // otherwise open the board at a scale no gesture can get back from.
            zoom: num(view.get("zoom"), crate::viewport::BASE_ZOOM).clamp(MIN_ZOOM, MAX_ZOOM),
        };
    }

    let raw_settings = src.get("settings").map(record).unwrap_or_default();
    board.settings.desktop = normalize_settings(&raw_settings, LayoutMode::Desktop);
    board.settings.mobile = normalize_settings(&raw_settings, LayoutMode::Mobile);

    // One id space across the live board and the bin: a restored item must not
    // collide with a live one. The live board is filled first, so where the two
    // disagree it is the *binned* item that gets renamed.
    //
    // A set rather than a list, because the only question ever asked of it is
    // whether an id is in it, and asking that of a list once per card is a
    // board that takes longer to open the bigger it gets. See `normalize_item`.
    let mut ids: HashSet<String> = HashSet::new();

    board.items = array(src.get("items"))
        .iter()
        .take(MAX_ITEMS)
        .filter(|v| v.is_object())
        .map(|v| normalize_item(v, &mut ids))
        .collect();

    board.trash = array(src.get("trash"))
        .iter()
        .take(TRASH_LIMIT)
        .filter_map(|v| {
            let entry = record(v);
            let item = entry.get("item")?;
            if !item.is_object() {
                return None;
            }
            Some(TrashEntry {
                item: normalize_item(item, &mut ids),
                at: entry.get("at").and_then(Value::as_i64).unwrap_or(0),
            })
        })
        .collect();

    // The ids a connection or an ordering may name: live *or* binned, so that
    // restoring a card brings its lines and its place in the playlist back with
    // it. Narrower than "every id" for connections, which is applied below.
    let known: HashSet<&str> = ids.iter().map(String::as_str).collect();

    let layouts = src.get("layouts").map(record).unwrap_or_default();
    board.layouts.desktop = normalize_layout(layouts.get("desktop"), &board.items, true);
    board.layouts.mobile = normalize_layout(layouts.get("mobile"), &board.items, false);

    board.arrangements.desktop =
        string(src.get("arrangement")).filter(|s| !s.is_empty()).unwrap_or_else(|| "free".into());
    if let Some(mobile) = layouts.get("mobile").map(record) {
        if let Some(order) = string(mobile.get("arrangement")) {
            board.arrangements.mobile = normalize_mobile_order(&order);
        }
    }

    // The masthead moved from `settings` to the top level. A reader takes the
    // top-level value first and falls back to the old home, which is what makes
    // a file written before the move open with its typography intact.
    board.mobile_header = src
        .get("mobileHeader")
        .or_else(|| raw_settings.get("mobileHeader"))
        .map(normalize_mobile_header)
        .unwrap_or_default();

    board.title_hidden = truthy(src.get("titleHidden"));

    board.media_fit = match string(src.get("mediaFit")).as_deref() {
        Some("cover") => "cover".into(),
        // An absent or unrecognised value reads as `contain`, which is the
        // default rather than an error: a newer build's third fit mode should
        // letterbox here, not blank the card.
        _ => "contain".into(),
    };

    board.palette_sources = clean_palette_sources(src.get("paletteSources"));

    board.connections = normalize_connections(src.get("connections"), Some(&known));
    board.audio_order = normalize_id_list(src.get("audioOrder"), Some(&known));
    board.tour = normalize_id_list(src.get("tour"), Some(&known));

    board
}

/// One item, with a guaranteed-unique id.
///
/// `seen` is threaded through rather than deduped afterwards because the *order*
/// of the renames is load-bearing: whoever asks first keeps the name.
fn normalize_item(data: &Value, seen: &mut HashSet<String>) -> Item {
    let src = record(data);

    let mut id = string(src.get("id")).map(|s| clean_id(&s)).unwrap_or_default();
    if id.is_empty() {
        id = next_id(seen.len());
    }
    while seen.contains(&id) {
        id = format!("{}-{}", id, seen.len());
    }
    seen.insert(id.clone());

    let w = crate::geometry::clamp_size(num(src.get("w"), 320.0));
    let h = crate::geometry::clamp_size(num(src.get("h"), 240.0));

    Item {
        id,
        kind: string(src.get("type")).map(|s| ItemType::parse(&s)).unwrap_or(ItemType::Generic),
        x: num(src.get("x"), 0.0),
        y: num(src.get("y"), 0.0),
        w,
        h,
        rot: num(src.get("rot"), 0.0),
        z: num(src.get("z"), 0.0),
        name: string(src.get("name")).unwrap_or_default(),
        asset: normalize_asset(src.get("asset")),
        // Carried whole. Unknown keys ride along untouched — that is the
        // format's extension point, and filtering here would quietly strip a
        // newer build's work on every save.
        meta: src.get("meta").map(record).unwrap_or_default(),
    }
}

/// The asset reference, or nothing.
///
/// A bad hash is `None` rather than a kept-but-broken reference: a hash that
/// does not name 64 hex characters cannot name bytes in the store either, so
/// carrying it forward would only produce a card that never resolves.
fn normalize_asset(data: Option<&Value>) -> Option<ItemAsset> {
    let src = record(data?);
    if let Some(external) = src.get("external") {
        return Some(ItemAsset::External(external.clone()));
    }
    let hash = string(src.get("hash"))?;
    if !is_hash(&hash) {
        return None;
    }
    Some(ItemAsset::Embedded { hash, family: string(src.get("family")) })
}

/// The settings for one profile.
///
/// Only the fields that reach a stylesheet, a filename or the geometry are
/// re-validated; the flags are read as truthiness, exactly as the original
/// does. Being stricter here would refuse boards the app itself has written.
fn normalize_settings(raw: &Map<String, Value>, mode: LayoutMode) -> BoardSettings {
    let mut out = BoardSettings::default();

    // The per-layout record, where the file carries one, spread over the
    // top-level (Desktop) values. The record wins wherever it has an opinion.
    let profile = raw.get(mode.as_str()).map(record).unwrap_or_default();
    let get = |key: &str| profile.get(key).or_else(|| raw.get(key));

    if let Some(v) = get("grid") {
        out.grid = truthy(Some(v));
    }
    if let Some(v) = get("axes") {
        out.axes = truthy(Some(v));
    }
    if let Some(v) = get("snap") {
        out.snap = truthy(Some(v));
    }
    if let Some(v) = get("web") {
        out.web = truthy(Some(v));
    }
    if let Some(v) = get("hud") {
        out.hud = truthy(Some(v));
    }
    // Absent means on, which is why this is not `truthy(get(..))` like the
    // flags above it: every board written before guides existed has no key
    // here, and reading that as "off" would quietly turn the feature off for
    // every board anybody already has.
    if let Some(v) = get("guides") {
        out.guides = truthy(Some(v));
    }
    if let Some(v) = get("gridStyle").and_then(|v| v.as_str()) {
        out.grid_style = v.to_string();
    }

    out.grid_step = num(get("gridStep"), 64.0).clamp(1.0, 4096.0);
    out.scale = {
        let s = num(get("scale"), DEFAULT_SCALE);
        if s.is_finite() && s > 0.0 {
            s
        } else {
            DEFAULT_SCALE
        }
    };
    out.units = match string(get("units")).as_deref() {
        Some("imperial") => "imperial".into(),
        _ => "metric".into(),
    };
    // A paper id goes on to name a sheet in a catalogue. Anything unrecognised
    // is no sheet at all rather than a guess.
    out.paper = string(get("paper")).filter(|s| is_paper_id(s)).unwrap_or_default();
    out.paper_landscape = truthy(get("paperLandscape"));
    out.paper_resize = truthy(get("paperResize"));
    out.appearance = normalize_look(get("appearance"));
    out.fonts = normalize_fonts(get("fonts"));

    match mode {
        LayoutMode::Desktop => {
            out.spacing = num(get("spacing"), 12.0).clamp(0.0, 512.0);
            // Desktop's inert compatibility value.
            out.mobile_columns = 6;
        }
        LayoutMode::Mobile => {
            // Zero rather than Desktop's 12 where the file has no Mobile record
            // of its own: that is what boards written before Mobile had a gap
            // were actually saved looking like.
            out.spacing =
                profile.get("spacing").map(|v| num(Some(v), 0.0).clamp(0.0, 512.0)).unwrap_or(0.0);
            out.mobile_columns = match num(get("mobileColumns"), 6.0).round() as i64 {
                8 => 8,
                _ => 6,
            };
        }
    }

    out
}

/// The board's own look.
///
/// `vars` is the only part of a `.mbrd` that reaches a renderer as anything
/// like code, so it is filtered rather than carried: a key that is not a
/// `--custom-property` name, or a value carrying a brace or a semicolon, is
/// dropped. This is the format's one genuine injection surface and the filter
/// belongs at the door.
fn normalize_look(data: Option<&Value>) -> Look {
    let Some(src) = data.map(record) else {
        return Look::default();
    };
    let mut vars = Map::new();
    for (key, value) in src.get("vars").map(record).unwrap_or_default() {
        let Some(text) = value.as_str() else { continue };
        if !key.starts_with("--") || key.len() > 64 {
            continue;
        }
        if !key[2..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        if text.len() > 128 || text.contains(['{', '}', ';', '<', '>', '(', ')']) {
            continue;
        }
        vars.insert(key.clone(), Value::String(text.to_string()));
    }
    Look { palette: string(src.get("palette")).unwrap_or_default(), vars }
}

fn normalize_fonts(data: Option<&Value>) -> Vec<FontSpec> {
    array(data)
        .iter()
        .filter_map(|v| {
            let src = record(v);
            let hash = string(src.get("hash")).filter(|h| is_hash(h))?;
            let family = string(src.get("family")).filter(|f| is_family(f))?;
            let axes: Vec<FontAxis> = array(src.get("axes"))
                .iter()
                .take(32)
                .filter_map(|a| {
                    let a = record(a);
                    let tag = string(a.get("tag"))?;
                    if tag.chars().count() != 4
                        || !tag.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ')
                    {
                        return None;
                    }
                    let (min, default, max) = (
                        num(a.get("min"), f32::NAN),
                        num(a.get("default"), f32::NAN),
                        num(a.get("max"), f32::NAN),
                    );
                    // A record whose bounds are not finite, or whose max is not
                    // above its min, is a slider that cannot be drawn.
                    if !(min.is_finite() && default.is_finite() && max.is_finite()) || max <= min {
                        return None;
                    }
                    Some(FontAxis { tag, min, default, max })
                })
                .collect();
            // At most one of the two is ever written, and `axes` is the
            // stronger claim, so it wins where a file carries both.
            let variable = axes.is_empty() && truthy(src.get("variable"));
            Some(FontSpec { hash, family, axes, variable })
        })
        .take(MAX_FONTS)
        .collect()
}

/// How many of the board's pictures the dynamic palette reads.
fn clean_palette_sources(data: Option<&Value>) -> u32 {
    match data.and_then(Value::as_f64) {
        // Zero is the slider's stop *past the top* — every picture on the
        // board — and not a count below its bottom, so it is preserved rather
        // than clamped up to one.
        Some(0.0) => 0,
        Some(n) if n.is_finite() => (n.round() as i64).clamp(1, 24) as u32,
        _ => 12,
    }
}

fn normalize_mobile_header(data: &Value) -> MobileHeader {
    let src = record(data);
    let d = MobileHeader::default();
    MobileHeader {
        font: string(src.get("font")).unwrap_or(d.font),
        size: num(src.get("size"), d.size).clamp(1.0, 400.0),
        stretch: num(src.get("stretch"), d.stretch).clamp(1.0, 1000.0),
        leading: num(src.get("leading"), d.leading).clamp(1.0, 1000.0),
        weight: num(src.get("weight"), d.weight).clamp(1.0, 1000.0),
        offset: num(src.get("offset"), d.offset),
        italic: truthy(src.get("italic")),
        wrap: src.get("wrap").map(|v| truthy(Some(v))).unwrap_or(d.wrap),
        axes: src.get("axes").map(record).unwrap_or_default(),
    }
}

/// One layout's geometry, completed against the items the board actually has.
///
/// A file need not carry a `layouts` record at all — those written before the
/// two profiles existed do not — so this falls back to the top-level geometry
/// on each item, which is exactly what that duplication in `items` is for.
/// Missing per-item records are filled the same way rather than dropped: an
/// item with no place in a layout would otherwise vanish from it.
fn normalize_layout(data: Option<&Value>, items: &[Item], is_desktop: bool) -> Vec<Geometry> {
    // Both shapes are accepted: a bare array of geometry records, and the
    // object form that also carries the layout's `arrangement` and `settings`.
    //
    // Borrowed rather than copied out. A memo carries a record per card, so on
    // a full board this is twenty thousand JSON objects and there is nothing
    // here that needs its own copy of them.
    let list: &[Value] = data
        .and_then(Value::as_array)
        .or_else(|| {
            data.and_then(Value::as_object)
                .and_then(|src| src.get("items").or_else(|| src.get("geometry")))
                .and_then(Value::as_array)
        })
        .map(Vec::as_slice)
        .unwrap_or_default();

    // **Indexed once, rather than searched once per card.** This is the line
    // that used to decide how long a large board took to open: a scan of the
    // whole memo for every card is the board squared, and it copied each record
    // it looked at on the way past — twenty thousand cards against twenty
    // thousand records is four hundred million clones of a JSON object, which
    // is most of a minute of somebody watching a window that has stopped
    // answering.
    //
    // First entry wins, which is what the search this replaces did.
    let mut memo: HashMap<&str, &Map<String, Value>> = HashMap::with_capacity(list.len());
    for entry in list {
        let Some(record) = entry.as_object() else { continue };
        let Some(id) = record.get("id").and_then(Value::as_str) else { continue };
        memo.entry(id).or_insert(record);
    }

    let mut out: Vec<Geometry> = Vec::with_capacity(items.len());
    for item in items {
        // The title card is Desktop furniture and is never packed onto Mobile.
        if !is_desktop && item.kind == ItemType::Title {
            continue;
        }
        let found = memo.get(item.id.as_str()).copied();
        out.push(match found {
            Some(g) => Geometry {
                id: item.id.clone(),
                x: num(g.get("x"), item.x),
                y: num(g.get("y"), item.y),
                w: crate::geometry::clamp_size(num(g.get("w"), item.w)),
                h: crate::geometry::clamp_size(num(g.get("h"), item.h)),
                rot: num(g.get("rot"), item.rot),
                z: num(g.get("z"), item.z),
                // Checked before it is believed: a memo is a promise to put
                // a card back somewhere, and an unsound one is dropped rather
                // than repaired — there is no nearest sensible place to round
                // "put this card at infinity" to.
                presnap: g
                    .get("presnap")
                    .map(record)
                    .map(|p| PreSnap {
                        x: num(p.get("x"), 0.0),
                        y: num(p.get("y"), 0.0),
                        w: num(p.get("w"), MIN_SIZE),
                        h: num(p.get("h"), MIN_SIZE),
                    })
                    .filter(crate::snap::sound),
            },
            None => Geometry {
                id: item.id.clone(),
                x: item.x,
                y: item.y,
                w: item.w,
                h: item.h,
                rot: item.rot,
                z: item.z,
                presnap: None,
            },
        });
    }
    out
}

/// `layouts.mobile.arrangement` names an **order**, not a shape.
///
/// A Desktop shape stored here — which older files do carry — is read as the
/// nearest order rather than refused, and is deliberately not rewritten on load.
fn normalize_mobile_order(value: &str) -> String {
    match value {
        "fit" | "free" | "date" | "type" | "name" | "shuffle" => value.into(),
        "scatter" => "shuffle".into(),
        _ => "fit".into(),
    }
}

/// `known` is the ids a pair may name, or `None` for "take them as they come".
///
/// `None` is what the step ledger reads through: a recorded connection describes
/// a board that had cards this one may not, and pruning against the live board
/// would drop somebody's line the moment they undid the deletion that removed
/// its far end. The pruning happens once, at the file boundary, which is the
/// only place that knows what the file will actually carry.
fn normalize_connections(data: Option<&Value>, known: Option<&HashSet<&str>>) -> Vec<Connection> {
    let mut out: Vec<Connection> = Vec::new();
    // The pairs already kept, so that the duplicate check below is a lookup
    // rather than a walk of everything kept so far — and so that `key` is not
    // recomputed for every pair against every other pair.
    let mut kept: HashSet<(String, String)> = HashSet::new();
    for entry in array(data).iter() {
        let Some(pair) = entry.as_array() else { continue };
        let (Some(a), Some(b)) =
            (pair.first().and_then(Value::as_str), pair.get(1).and_then(Value::as_str))
        else {
            continue;
        };
        // A card joined to itself is not a connection.
        if a == b {
            continue;
        }
        // Both ends must name an item the file actually carries. A pair naming
        // nothing is dropped, since nothing could ever make it mean something.
        if known.map(|k| !k.contains(a) || !k.contains(b)).unwrap_or(false) {
            continue;
        }
        let conn = Connection {
            a: a.to_string(),
            b: b.to_string(),
            meta: pair.get(2).map(normalize_conn_meta).unwrap_or_default(),
        };
        // Duplicates collapse, in either order.
        let (first, second) = conn.key();
        if !kept.insert((first.to_string(), second.to_string())) {
            continue;
        }
        out.push(conn);
        if out.len() >= MAX_CONNECTIONS {
            break;
        }
    }
    out
}

/// How a line is drawn.
///
/// Unknown keys and unknown values are dropped rather than carried, and each
/// falls back to its default independently — so a `color` this build has never
/// heard of draws the ordinary grey line, which is the right answer to a board
/// written by a newer build.
fn normalize_conn_meta(data: &Value) -> ConnMeta {
    let src = record(data);
    // Generic rather than a closure: a closure would be monomorphic in its
    // return type, and these four are four different enums that happen to be
    // read the same way.
    fn pick<T: Default>(src: &Map<String, Value>, key: &str, parse: fn(&str) -> Option<T>) -> T {
        string(src.get(key)).and_then(|s| parse(&s)).unwrap_or_default()
    }
    ConnMeta {
        dir: pick(&src, "dir", ConnDir::parse),
        style: pick(&src, "style", ConnStyle::parse),
        color: pick(&src, "color", ConnColor::parse),
        weight: pick(&src, "weight", ConnWeight::parse),
        label: string(src.get("label")).map(|s| collapse_space(&s, 60)).filter(|s| !s.is_empty()),
        // Clamped rather than rejected, like everything else in here: a
        // fraction outside the line is a label at the end of the line, which
        // is somewhere, and refusing the connection over it would not be.
        label_at: num(src.get("labelAt"), crate::model::LABEL_MIDDLE).clamp(0.0, 1.0),
    }
}

/// A flat list of item ids — `audioOrder` and `tour` are the same shape.
///
/// Malformed entries are dropped one at a time rather than failing the load,
/// and an id that names nothing goes too: both lists are self-healing at the
/// far end, so a stale entry costs nothing to lose here.
fn normalize_id_list(data: Option<&Value>, known: Option<&HashSet<&str>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    // Both checks are membership questions, and both used to be walks: one over
    // every id on the board and one over everything kept so far. A tour as long
    // as the board is then the board squared. See `normalize_layout`.
    let list = array(data);
    let mut taken: HashSet<&str> = HashSet::new();
    for v in list.iter().take(MAX_ITEMS) {
        let Some(id) = v.as_str() else { continue };
        if known.map(|k| !k.contains(id)).unwrap_or(false) || !taken.insert(id) {
            continue;
        }
        out.push(id.to_string());
    }
    out
}

// ---------------------------------------------------------------------------
// Writing one back out
// ---------------------------------------------------------------------------

/// The board as `board.json` will hold it.
///
/// Coordinates and sizes are rounded to two decimals on the way out. A board is
/// a place things sit, not a measurement, and the third decimal of a drag is
/// noise that costs bytes in every item.
pub fn serialize(board: &Board) -> Value {
    let mut out = Map::new();
    out.insert("title".into(), json!(board.title));
    out.insert(
        "view".into(),
        json!({
            "pan": { "x": round2(board.view.pan_x), "y": round2(board.view.pan_y) },
            // Not `round2`, and this is the one field where that matters. Two
            // decimals is plenty for a world coordinate, where the unit is
            // about a pixel — and it is *everything* for a zoom, where the
            // whole bottom of the range lives below `0.01` and would be saved
            // as a flat zero, then read back as the floor. Significant digits
            // rather than decimal places is the shape this number wants.
            "zoom": significant(board.view.zoom),
        }),
    );
    out.insert("settings".into(), serialize_settings(board));
    out.insert("arrangement".into(), json!(board.arrangements.desktop));
    out.insert("mobileHeader".into(), serialize_mobile_header(&board.mobile_header));
    out.insert("titleHidden".into(), json!(board.title_hidden));
    out.insert("mediaFit".into(), json!(board.media_fit));
    out.insert("paletteSources".into(), json!(board.palette_sources));
    out.insert("items".into(), Value::Array(board.items.iter().map(serialize_item).collect()));
    out.insert(
        "layouts".into(),
        json!({
            "desktop": in_item_order(&board.layouts.desktop, &board.items),
            "mobile": {
                "arrangement": board.arrangements.mobile,
                "items": in_item_order(&board.layouts.mobile, &board.items),
            },
        }),
    );
    out.insert(
        "connections".into(),
        Value::Array(board.connections.iter().map(serialize_connection).collect()),
    );
    // Written even when empty: a tour that was built and then cleared must not
    // read as a board that never had one. Same for the playlist order.
    out.insert("audioOrder".into(), json!(board.audio_order));
    out.insert("tour".into(), json!(board.tour));
    out.insert(
        "trash".into(),
        Value::Array(
            board
                .trash
                .iter()
                .map(|t| json!({ "at": t.at, "item": serialize_item(&t.item) }))
                .collect(),
        ),
    );
    Value::Object(out)
}

fn serialize_item(item: &Item) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(item.id));
    out.insert("type".into(), json!(item.kind.as_str()));
    out.insert("x".into(), json!(round2(item.x)));
    out.insert("y".into(), json!(round2(item.y)));
    out.insert("w".into(), json!(round2(item.w)));
    out.insert("h".into(), json!(round2(item.h)));
    out.insert("rot".into(), json!(round2(item.rot)));
    out.insert("z".into(), json!(round2(item.z)));
    out.insert("name".into(), json!(item.name));
    out.insert(
        "asset".into(),
        match &item.asset {
            None => Value::Null,
            Some(ItemAsset::External(v)) => json!({ "external": v }),
            Some(ItemAsset::Embedded { hash, family }) => {
                let mut a = Map::new();
                a.insert("hash".into(), json!(hash));
                a.insert("embedded".into(), json!(true));
                if let Some(f) = family {
                    a.insert("family".into(), json!(f));
                }
                Value::Object(a)
            }
        },
    );
    out.insert("meta".into(), Value::Object(item.meta.clone()));
    Value::Object(out)
}

/// One layout's records, written in the item list's order.
///
/// A geometry list is **keyed by id**, so the order it happens to be held in is
/// not a fact about the board — [`normalize`] reads one back by completing it
/// against the items regardless of how it arrived. Writing it in an order the
/// board already has is what makes that true in both directions: a card taken
/// off the board and put back by an undo lands wherever the item list has it
/// rather than at the end of the layout, and two boards that are the same board
/// produce the same bytes.
///
/// A record naming no item is kept, at the back. Nothing in this crate produces
/// one, and dropping data on the floor to tidy an array would be a poor trade.
fn in_item_order(list: &[Geometry], items: &[Item]) -> Value {
    let mut by_id: Map<String, Value> = Map::new();
    for g in list {
        by_id.insert(g.id.clone(), serialize_geometry(g));
    }
    let mut out: Vec<Value> = Vec::with_capacity(by_id.len());
    for item in items {
        if let Some(g) = by_id.shift_remove(&item.id) {
            out.push(g);
        }
    }
    out.extend(by_id.into_iter().map(|(_, g)| g));
    Value::Array(out)
}

fn serialize_geometry(g: &Geometry) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(g.id));
    out.insert("x".into(), json!(round2(g.x)));
    out.insert("y".into(), json!(round2(g.y)));
    out.insert("w".into(), json!(round2(g.w)));
    out.insert("h".into(), json!(round2(g.h)));
    out.insert("rot".into(), json!(round2(g.rot)));
    out.insert("z".into(), json!(round2(g.z)));
    if let Some(p) = g.presnap {
        out.insert(
            "presnap".into(),
            json!({ "x": round2(p.x), "y": round2(p.y), "w": round2(p.w), "h": round2(p.h) }),
        );
    }
    Value::Object(out)
}

/// A connection at its defaults is a bare two-element array.
///
/// Defaults are omitted rather than written, so nothing about the common case
/// changed when the third element was added, and an older reader still sees the
/// pair it understands.
fn serialize_connection(c: &Connection) -> Value {
    if c.meta.is_default() {
        return json!([c.a, c.b]);
    }
    let mut m = Map::new();
    let d = ConnMeta::default();
    if c.meta.dir != d.dir {
        m.insert("dir".into(), json!(c.meta.dir.as_str()));
    }
    if c.meta.style != d.style {
        m.insert("style".into(), json!(c.meta.style.as_str()));
    }
    if c.meta.color != d.color {
        m.insert("color".into(), json!(c.meta.color.as_str()));
    }
    if c.meta.weight != d.weight {
        m.insert("weight".into(), json!(c.meta.weight.as_str()));
    }
    if let Some(label) = &c.meta.label {
        m.insert("label".into(), json!(label));
        // Only alongside the label, and only when it has been moved. Where
        // nothing is written there is nothing to place.
        if c.meta.label_at != crate::model::LABEL_MIDDLE {
            m.insert("labelAt".into(), json!(round2(c.meta.label_at)));
        }
    }
    json!([c.a, c.b, Value::Object(m)])
}

fn serialize_settings(board: &Board) -> Value {
    // Top-level `settings` describes Desktop, with the Mobile record nested
    // under it. That asymmetry is the format's, not a convenience: an older
    // reader that knows nothing of profiles finds the Desktop board where it
    // has always been.
    let mut out = settings_fields(&board.settings.desktop);
    out.insert("mobile".into(), Value::Object(settings_fields(&board.settings.mobile)));
    // Mirrored back into its old home so an older reader still finds it.
    out.insert("mobileHeader".into(), serialize_mobile_header(&board.mobile_header));
    Value::Object(out)
}

fn settings_fields(s: &BoardSettings) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("grid".into(), json!(s.grid));
    out.insert("axes".into(), json!(s.axes));
    out.insert("snap".into(), json!(s.snap));
    out.insert("web".into(), json!(s.web));
    out.insert("hud".into(), json!(s.hud));
    out.insert("guides".into(), json!(s.guides));
    out.insert("gridStyle".into(), json!(s.grid_style));
    out.insert("gridStep".into(), json!(round2(s.grid_step)));
    out.insert("mobileColumns".into(), json!(s.mobile_columns));
    out.insert("spacing".into(), json!(round2(s.spacing)));
    out.insert("scale".into(), json!(round2(s.scale)));
    out.insert("units".into(), json!(s.units));
    out.insert("paper".into(), json!(s.paper));
    out.insert("paperLandscape".into(), json!(s.paper_landscape));
    out.insert("paperResize".into(), json!(s.paper_resize));
    out.insert(
        "appearance".into(),
        json!({ "palette": s.appearance.palette, "vars": Value::Object(s.appearance.vars.clone()) }),
    );
    out.insert(
        "fonts".into(),
        Value::Array(
            s.fonts
                .iter()
                .map(|f| {
                    let mut m = Map::new();
                    m.insert("hash".into(), json!(f.hash));
                    m.insert("family".into(), json!(f.family));
                    // At most one of the two, and `axes` is the stronger claim.
                    if !f.axes.is_empty() {
                        m.insert(
                            "axes".into(),
                            Value::Array(
                                f.axes
                                    .iter()
                                    .map(|a| {
                                        json!({
                                            "tag": a.tag,
                                            "min": round2(a.min),
                                            "default": round2(a.default),
                                            "max": round2(a.max),
                                        })
                                    })
                                    .collect(),
                            ),
                        );
                    } else if f.variable {
                        m.insert("variable".into(), json!(true));
                    }
                    Value::Object(m)
                })
                .collect(),
        ),
    );
    out
}

fn serialize_mobile_header(h: &MobileHeader) -> Value {
    json!({
        "font": h.font,
        "size": round2(h.size),
        "stretch": round2(h.stretch),
        "leading": round2(h.leading),
        "weight": round2(h.weight),
        "offset": round2(h.offset),
        "italic": h.italic,
        "wrap": h.wrap,
        "axes": Value::Object(h.axes.clone()),
    })
}

// ---------------------------------------------------------------------------
// One field at a time, in both directions
// ---------------------------------------------------------------------------
//
// The step ledger records the board as *text* — one item, one geometry record,
// one bin entry, one whole small field — because that is what makes a step
// eighty bytes and comparable with `==`. Which bytes those are is this module's
// business rather than the ledger's, so the pairs below live here beside the
// whole-board pair they are built from. A field added to `normalize` and
// `serialize` and forgotten here is a field undo cannot take back.

/// One item, as `board.json` carries it.
pub fn item_value(item: &Item) -> Value {
    serialize_item(item)
}

/// And back. Nothing here can fail, for [`normalize`]'s reason.
///
/// Ids are taken as they arrive rather than made unique: what this reads was
/// written by the board it is going back onto, so renaming here would be
/// inventing a card the ledger never mentioned.
pub fn item_of_value(data: &Value) -> Item {
    normalize_item(data, &mut HashSet::new())
}

/// One item's place in one layout.
pub fn geometry_value(g: &Geometry) -> Value {
    serialize_geometry(g)
}

/// And back, standalone — unlike [`normalize`]'s layout reader, which completes
/// a whole list against the items it belongs to. A record with no usable id is
/// `None`, since a geometry that names no item has nowhere to go.
pub fn geometry_of_value(data: &Value) -> Option<Geometry> {
    let src = record(data);
    let id = string(src.get("id")).filter(|s| !s.is_empty())?;
    Some(Geometry {
        id,
        x: num(src.get("x"), 0.0),
        y: num(src.get("y"), 0.0),
        w: crate::geometry::clamp_size(num(src.get("w"), 320.0)),
        h: crate::geometry::clamp_size(num(src.get("h"), 240.0)),
        rot: num(src.get("rot"), 0.0),
        z: num(src.get("z"), 0.0),
        presnap: src.get("presnap").map(record).map(|p| PreSnap {
            x: num(p.get("x"), 0.0),
            y: num(p.get("y"), 0.0),
            w: num(p.get("w"), MIN_SIZE),
            h: num(p.get("h"), MIN_SIZE),
        }),
    })
}

/// One thing in the bin: the item as it was, and when it went in.
pub fn trash_value(entry: &TrashEntry) -> Value {
    json!({ "at": entry.at, "item": serialize_item(&entry.item) })
}

pub fn trash_of_value(data: &Value) -> Option<TrashEntry> {
    let src = record(data);
    let item = src.get("item").filter(|v| v.is_object())?;
    Some(TrashEntry {
        item: item_of_value(item),
        at: src.get("at").and_then(Value::as_i64).unwrap_or(0),
    })
}

/// The board's small fields, by the name a step records them under.
///
/// Deliberately **not** the whole of `board.json`: `items`, `layouts` and
/// `trash` are the keyed sections and are recorded per id, which is the entire
/// reason a step is small.
///
/// `view` is deliberately absent, and it is the one place this list diverges
/// from the original's. The camera is where you are looking rather than
/// something you did, and a board is saved with wherever the view was left — so
/// recording it would put a pair on every step taken after a save, and undoing
/// a nudge would throw the camera somewhere the person undoing it never asked
/// to go. A step written by a build that does record it is read past, not
/// refused.
pub const REST_FIELDS: [&str; 10] = [
    "title",
    "layoutSettings",
    "arrangements",
    "mobileHeader",
    "titleHidden",
    "mediaFit",
    "paletteSources",
    "connections",
    "audioOrder",
    "tour",
];

/// Whether one of [`REST_FIELDS`] differs between two boards, answered
/// structurally and without serialising either side.
///
/// [`rest_value`] builds a full `serde_json::Value` — for `connections` that is
/// an array of every wire on the board — and `changes` runs over every field on
/// every edit, so building both sides just to learn they are equal made a nudge
/// cost the board rather than the nudge. Serialisation is a pure function of
/// the field, so structural equality is enough to skip it; a field that *does*
/// differ still goes through the serialised compare, which is what lets a
/// difference the file cannot represent round away. A name not on the list
/// answers `true` and is sorted out by `rest_value` returning `None`.
pub fn rest_differs(before: &Board, after: &Board, field: &str) -> bool {
    match field {
        "title" => before.title != after.title,
        "layoutSettings" => before.settings != after.settings,
        "arrangements" => before.arrangements != after.arrangements,
        "mobileHeader" => before.mobile_header != after.mobile_header,
        "titleHidden" => before.title_hidden != after.title_hidden,
        "mediaFit" => before.media_fit != after.media_fit,
        "paletteSources" => before.palette_sources != after.palette_sources,
        "connections" => before.connections != after.connections,
        "audioOrder" => before.audio_order != after.audio_order,
        "tour" => before.tour != after.tour,
        _ => true,
    }
}

/// One of [`REST_FIELDS`], as a step records it. `None` for a name not on that
/// list, which is what a step written by another build is read through.
pub fn rest_value(board: &Board, field: &str) -> Option<Value> {
    Some(match field {
        "title" => json!(board.title),
        "layoutSettings" => json!({
            "desktop": Value::Object(settings_fields(&board.settings.desktop)),
            "mobile": Value::Object(settings_fields(&board.settings.mobile)),
        }),
        "arrangements" => json!({
            "desktop": board.arrangements.desktop,
            "mobile": board.arrangements.mobile,
        }),
        "mobileHeader" => serialize_mobile_header(&board.mobile_header),
        "titleHidden" => json!(board.title_hidden),
        "mediaFit" => json!(board.media_fit),
        "paletteSources" => json!(board.palette_sources),
        "connections" => Value::Array(board.connections.iter().map(serialize_connection).collect()),
        "audioOrder" => json!(board.audio_order),
        "tour" => json!(board.tour),
        _ => return None,
    })
}

/// Put one of [`REST_FIELDS`] back on the board.
///
/// Held to exactly the rules [`normalize`] applies to the same field, because a
/// step is a record of a board somebody else may have written and is no more
/// trustworthy than a file is. A name this build does not know is a no-op rather
/// than an error.
pub fn rest_apply(board: &mut Board, field: &str, data: &Value) {
    match field {
        "title" => board.title = clean_title(Some(data)),
        "layoutSettings" => {
            let src = record(data);
            let desktop = src.get("desktop").map(record).unwrap_or_default();
            board.settings.desktop = normalize_settings(&desktop, LayoutMode::Desktop);
            // The Mobile reader looks for its record *under* `mobile`, since
            // that is where a file keeps it — so the value is handed to it in
            // the shape it expects rather than flattened.
            let mut wrapped = Map::new();
            wrapped.insert("mobile".into(), src.get("mobile").cloned().unwrap_or(Value::Null));
            board.settings.mobile = normalize_settings(&wrapped, LayoutMode::Mobile);
        }
        "arrangements" => {
            let src = record(data);
            board.arrangements.desktop = string(src.get("desktop"))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "free".into());
            board.arrangements.mobile = string(src.get("mobile"))
                .map(|s| normalize_mobile_order(&s))
                .unwrap_or_else(|| "fit".into());
        }
        "mobileHeader" => board.mobile_header = normalize_mobile_header(data),
        "titleHidden" => board.title_hidden = truthy(Some(data)),
        "mediaFit" => {
            board.media_fit = match data.as_str() {
                Some("cover") => "cover".into(),
                _ => "contain".into(),
            }
        }
        "paletteSources" => board.palette_sources = clean_palette_sources(Some(data)),
        "connections" => board.connections = normalize_connections(Some(data), None),
        "audioOrder" => board.audio_order = normalize_id_list(Some(data), None),
        "tour" => board.tour = normalize_id_list(Some(data), None),
        _ => {}
    }
}

/// A hash of the board **as a file carries it**, so a stale ledger cannot lie.
///
/// Taken over the stored arrays rather than over the live board, and the
/// distinction is the whole of why this exists. A board written out and read
/// back is not the same object: coordinates round to two places, absent keys
/// take their defaults, both layouts are recomputed. A hash of the live board
/// would therefore disagree with itself across an ordinary save and load, and
/// every reopened board would be declared stale — which is worse than not
/// checking, because a warning that is always wrong is a warning nobody reads.
///
/// `items` and `trash` alone, because those are what a step describes. The
/// failure being guarded is specific: a build that does not understand
/// `timeline` opens the file, drops the key, and writes the board back edited.
/// Hashing the item list catches exactly that, and does not fire on a view that
/// panned or a setting that moved, neither of which any step's correctness
/// depends on.
pub fn doc_fingerprint(items: Option<&Value>, trash: Option<&Value>) -> String {
    let text = |v: Option<&Value>| {
        serde_json::to_string(v.unwrap_or(&Value::Array(Vec::new()))).unwrap_or_default()
    };
    crate::history::fnv1a([text(items), text(trash)])
}

// ---------------------------------------------------------------------------
// The small readers everything above is built from
// ---------------------------------------------------------------------------

fn record(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

fn array(v: Option<&Value>) -> Vec<Value> {
    v.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_string)
}

/// A number, or the default. Non-finite counts as absent: an `Infinity` that
/// reached the geometry would put a card somewhere no pan could reach.
fn num(v: Option<&Value>, default: f32) -> f32 {
    match v.and_then(Value::as_f64) {
        Some(n) if n.is_finite() => n as f32,
        _ => default,
    }
}

/// JavaScript truthiness, because that is what wrote the file. A flag saved as
/// `1` or `"yes"` by a hand edit means the same thing it meant in the original.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0 && f.is_finite()).unwrap_or(false),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn round2(v: f32) -> f64 {
    if !v.is_finite() {
        return 0.0;
    }
    ((v as f64) * 100.0).round() / 100.0
}

/// Six significant digits, wherever the number happens to sit.
///
/// For a value whose *scale* is the thing being stored rather than its
/// distance from something — see the zoom in [`serialize`]. Six is the width
/// of an `f32`'s mantissa in decimal, so this is a shorter spelling of the
/// same number rather than a lossy one.
fn significant(v: f32) -> f64 {
    if !v.is_finite() || v == 0.0 {
        return 0.0;
    }
    let v = v as f64;
    let places = 6 - 1 - v.abs().log10().floor() as i32;
    let scale = 10f64.powi(places.clamp(0, 15));
    (v * scale).round() / scale
}

/// 64 lowercase hex characters. Enforced in both directions.
pub fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn is_family(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.contains(['"', '\'', ';', '{', '}', '<', '>', '(', ')'])
}

fn is_paper_id(s: &str) -> bool {
    matches!(s, "a3" | "a4" | "a5" | "a6" | "letter" | "legal" | "tabloid")
}

/// `[A-Za-z0-9_-]{1,64}`, with anything else stripped rather than the whole id
/// refused: an id is only ever compared to other ids, so a repaired one still
/// does its job.
fn clean_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').take(64).collect()
}

fn next_id(n: usize) -> String {
    format!("i{n:06}")
}

pub(crate) fn collapse_space(s: &str, max: usize) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// A title as typed, held to the same rules as one arriving in a file.
///
/// The public face of [`clean_title`], for the window: a name typed into the
/// app goes through the exact wash a `board.json`'s would, so the two can
/// never disagree about what a title may hold.
pub fn titled(text: &str) -> String {
    clean_title(Some(&Value::String(text.to_string())))
}

/// A board's title, held to what is safe in a file picker.
fn clean_title(v: Option<&Value>) -> String {
    let raw = string(v).unwrap_or_default();
    raw.chars()
        .filter(|c| {
            !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') && !c.is_control()
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(BOARD_TITLE_MAX)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_typed_title_is_washed_the_way_a_filed_one_is() {
        // The characters a file picker chokes on go, runs of space collapse,
        // and the length stops where the format says titles stop.
        assert_eq!(titled("  Kitchen   ideas  "), "Kitchen ideas");
        assert_eq!(titled("a/b:c*d"), "abcd");
        assert_eq!(titled(&"x".repeat(BOARD_TITLE_MAX + 9)), "x".repeat(BOARD_TITLE_MAX));
        assert_eq!(titled("***"), "");
    }

    #[test]
    fn nothing_at_all_is_a_default_board() {
        // The load-bearing property of this whole module: rubbish in, board out.
        for junk in [json!(null), json!(7), json!("board"), json!([]), json!({})] {
            let b = normalize(&junk);
            assert!(b.items.is_empty());
            assert_eq!(b.media_fit, "contain");
            assert_eq!(b.palette_sources, 12);
        }
    }

    #[test]
    fn a_board_written_before_guides_existed_still_has_them() {
        // The one flag on the settings whose absence does not mean `false`.
        // Every board anybody already has is missing this key, and reading a
        // missing key as "off" would turn the feature off for all of them —
        // silently, and in a way that looks like the feature never shipped.
        let old = json!({ "settings": { "grid": true } });
        assert!(normalize(&old).settings.desktop.guides);

        // Said explicitly, though, it is believed either way.
        let off = json!({ "settings": { "guides": false } });
        assert!(!normalize(&off).settings.desktop.guides);

        // And the same on the ledger's side of the fence, where the record is
        // named `layoutSettings` and keeps Desktop under its own key rather
        // than at the top. A step that predates the flag has to read as on for
        // the same reason a file does.
        let mut board = Board::default();
        rest_apply(&mut board, "layoutSettings", &json!({ "desktop": { "grid": true } }));
        assert!(board.settings.desktop.guides);
        rest_apply(&mut board, "layoutSettings", &json!({ "desktop": { "guides": false } }));
        assert!(!board.settings.desktop.guides);
    }

    #[test]
    fn whether_guides_are_on_survives_a_round_trip() {
        // A setting that cannot be turned off across a save is a setting that
        // is not off. See `settings_fields`, which is the other half.
        let mut board = Board::default();
        board.settings.desktop.guides = false;
        let there_and_back = normalize(&serialize(&board));
        assert!(!there_and_back.settings.desktop.guides);
    }

    #[test]
    fn a_truncated_item_degrades_field_by_field() {
        let b = normalize(&json!({ "items": [{ "id": "a1", "type": "image" }, "nonsense", 42] }));
        assert_eq!(b.items.len(), 1, "non-object entries drop, they do not fail the load");
        let it = &b.items[0];
        assert_eq!(it.id, "a1");
        assert_eq!(it.kind, ItemType::Image);
        assert_eq!(it.x, 0.0);
        assert!(it.w >= MIN_SIZE);
    }

    #[test]
    fn an_unknown_type_survives_a_round_trip() {
        let b = normalize(&json!({ "items": [{ "id": "a1", "type": "hologram" }] }));
        assert_eq!(b.items[0].kind, ItemType::Other("hologram".into()));
        let out = serialize(&b);
        assert_eq!(out["items"][0]["type"], json!("hologram"));
    }

    #[test]
    fn unknown_meta_keys_ride_along_untouched() {
        let b = normalize(&json!({
            "items": [{ "id": "a1", "type": "note", "meta": { "text": "hi", "flavour": "peach" } }]
        }));
        let out = serialize(&b);
        assert_eq!(out["items"][0]["meta"]["flavour"], json!("peach"));
    }

    #[test]
    fn colliding_ids_are_renamed_and_the_first_one_keeps_its_name() {
        let b = normalize(&json!({
            "items": [{ "id": "dup" }, { "id": "dup" }],
        }));
        assert_eq!(b.items[0].id, "dup");
        assert_ne!(b.items[1].id, "dup");
    }

    #[test]
    fn a_binned_item_never_collides_with_a_live_one() {
        let b = normalize(&json!({
            "items": [{ "id": "x" }],
            "trash": [{ "at": 1, "item": { "id": "x" } }],
        }));
        assert_eq!(b.items[0].id, "x", "the live board fills the id space first");
        assert_ne!(b.trash[0].item.id, "x");
    }

    #[test]
    fn connections_are_pruned_collapsed_and_kept_unordered() {
        let b = normalize(&json!({
            "items": [{ "id": "a" }, { "id": "b" }],
            "connections": [
                ["a", "b"],
                ["b", "a"],          // the same one, reversed
                ["a", "a"],          // a card joined to itself
                ["a", "ghost"],      // names nothing
                ["a"],               // malformed
                "nonsense",
            ],
        }));
        assert_eq!(b.connections.len(), 1);
        assert_eq!((b.connections[0].a.as_str(), b.connections[0].b.as_str()), ("a", "b"));
    }

    #[test]
    fn a_connection_to_a_binned_card_is_kept() {
        // Restoring that card has to bring its lines back with it.
        let b = normalize(&json!({
            "items": [{ "id": "a" }],
            "trash": [{ "at": 1, "item": { "id": "b" } }],
            "connections": [["a", "b"]],
        }));
        assert_eq!(b.connections.len(), 1);
    }

    #[test]
    fn a_plain_connection_writes_as_two_elements() {
        let b = normalize(&json!({
            "items": [{ "id": "a" }, { "id": "b" }],
            "connections": [["a", "b"]],
        }));
        assert_eq!(serialize(&b)["connections"][0], json!(["a", "b"]));
    }

    #[test]
    fn an_unknown_connection_colour_falls_back_to_the_plain_line() {
        let b = normalize(&json!({
            "items": [{ "id": "a" }, { "id": "b" }],
            "connections": [["a", "b", { "color": "chartreuse", "style": "dashed" }]],
        }));
        assert_eq!(b.connections[0].meta.color, ConnColor::Line);
        assert_eq!(b.connections[0].meta.style, ConnStyle::Dashed);
    }

    #[test]
    fn palette_sources_keeps_zero_and_clamps_the_top() {
        assert_eq!(normalize(&json!({ "paletteSources": 0 })).palette_sources, 0);
        assert_eq!(normalize(&json!({ "paletteSources": 99 })).palette_sources, 24);
        assert_eq!(normalize(&json!({ "paletteSources": "x" })).palette_sources, 12);
    }

    #[test]
    fn appearance_vars_that_could_reach_a_stylesheet_are_dropped() {
        let b = normalize(&json!({
            "settings": { "appearance": { "palette": "papyrus", "vars": {
                "--accent": "#b4553a",
                "--evil": "red; } body { display: none",
                "notAVar": "#fff",
            } } }
        }));
        let vars = &b.settings.desktop.appearance.vars;
        assert_eq!(vars.get("--accent"), Some(&json!("#b4553a")));
        assert!(vars.get("--evil").is_none());
        assert!(vars.get("notAVar").is_none());
    }

    #[test]
    fn mobile_spacing_defaults_to_zero_rather_than_inheriting_desktop() {
        let b = normalize(&json!({ "settings": { "spacing": 32 } }));
        assert_eq!(b.settings.desktop.spacing, 32.0);
        assert_eq!(b.settings.mobile.spacing, 0.0);
    }

    #[test]
    fn a_file_with_no_layouts_gets_one_from_the_item_geometry() {
        let b = normalize(&json!({
            "items": [{ "id": "a", "x": 120, "y": -40, "w": 320, "h": 240 }],
        }));
        assert_eq!(b.layouts.desktop.len(), 1);
        assert_eq!(b.layouts.desktop[0].x, 120.0);
        assert_eq!(b.layouts.desktop[0].y, -40.0);
    }

    #[test]
    fn a_desktop_shape_stored_as_a_mobile_order_reads_as_the_nearest_order() {
        let b = normalize(&json!({ "layouts": { "mobile": { "arrangement": "scatter" } } }));
        assert_eq!(b.arrangements.mobile, "shuffle");
        let b = normalize(&json!({ "layouts": { "mobile": { "arrangement": "spiral" } } }));
        assert_eq!(b.arrangements.mobile, "fit");
    }

    #[test]
    fn a_bad_asset_hash_is_no_asset_rather_than_a_broken_one() {
        let b = normalize(&json!({
            "items": [{ "id": "a", "asset": { "hash": "NOTAHASH", "embedded": true } }],
        }));
        assert!(b.items[0].asset.is_none());
    }

    #[test]
    fn the_view_is_clamped_to_what_the_app_can_draw() {
        let b = normalize(&json!({ "view": { "pan": { "x": 5, "y": 6 }, "zoom": 900 } }));
        assert_eq!(b.view.zoom, MAX_ZOOM);
        assert_eq!(b.view.pan_x, 5.0);
    }

    #[test]
    fn a_label_slid_along_a_line_stays_where_it_was_put() {
        let board = Board {
            items: vec![Item::new("a", ItemType::Note), Item::new("b", ItemType::Note)],
            connections: vec![Connection {
                a: "a".into(),
                b: "b".into(),
                meta: ConnMeta {
                    label: Some("goes with".into()),
                    label_at: 0.18,
                    ..ConnMeta::default()
                },
            }],
            ..Board::default()
        };
        let back = normalize(&serialize(&board));
        assert_eq!(back.connections[0].meta.label.as_deref(), Some("goes with"));
        assert!((back.connections[0].meta.label_at - 0.18).abs() < 0.01);
    }

    #[test]
    fn a_line_with_no_label_has_nowhere_to_put_one() {
        // A position with no words at it is not a connection worth writing a
        // third element for, however far somebody once slid it.
        let mut meta = ConnMeta { label_at: 0.9, ..ConnMeta::default() };
        assert!(meta.is_default(), "an unlabelled line should still be a plain one");
        let out =
            serialize_connection(&Connection { a: "a".into(), b: "b".into(), meta: meta.clone() });
        assert_eq!(out, json!(["a", "b"]), "it grew a third element for nothing");
        meta.label = Some("here".into());
        assert!(!meta.is_default());
    }

    #[test]
    fn a_label_position_off_the_end_of_the_line_is_pulled_back_onto_it() {
        let meta = normalize_conn_meta(&json!({ "label": "x", "labelAt": 4.5 }));
        assert_eq!(meta.label_at, 1.0);
        let meta = normalize_conn_meta(&json!({ "label": "x", "labelAt": -2.0 }));
        assert_eq!(meta.label_at, 0.0);
        let meta = normalize_conn_meta(&json!({ "label": "x" }));
        assert_eq!(meta.label_at, LABEL_MIDDLE, "an absent position is the middle");
    }

    #[test]
    fn a_zoom_from_the_bottom_of_the_range_survives_being_saved() {
        // The whole floor of the range sits below two decimal places, so
        // rounding it the way a coordinate is rounded would save every one of
        // these as zero and read them all back as the floor.
        let mut board = Board::default();
        for zoom in [MIN_ZOOM, 0.004, 0.0375, MAX_ZOOM] {
            board.view.zoom = zoom;
            let back = normalize(&serialize(&board));
            let off = (back.view.zoom - zoom).abs() / zoom;
            assert!(off < 1e-5, "{zoom} came back as {}", back.view.zoom);
        }
    }

    #[test]
    fn a_board_survives_a_round_trip_unchanged() {
        let source = json!({
            "title": "Kitchen",
            "view": { "pan": { "x": 12.5, "y": -8.25 }, "zoom": 0.8 },
            "settings": { "grid": true, "snap": true, "gridStep": 64, "spacing": 32 },
            "arrangement": "spiral",
            "mediaFit": "cover",
            "paletteSources": 6,
            "items": [
                { "id": "k3f9a2", "type": "image", "x": 120, "y": -40, "w": 320, "h": 240,
                  "rot": 0, "z": 7, "name": "kitchen-window.jpg",
                  "asset": { "hash": "a".repeat(64), "embedded": true }, "meta": { "fit": "cover" } },
                { "id": "p81m4x", "type": "note", "x": -60, "y": 90, "w": 200, "h": 200,
                  "rot": 3, "z": 8, "name": "buy the smaller one", "asset": null,
                  "meta": { "text": "the big one does not fit", "tint": 2 } }
            ],
            "connections": [["k3f9a2", "p81m4x", { "dir": "fwd", "label": "  goes   with " }]],
            "tour": ["k3f9a2"],
        });
        // `"a".repeat(64)` is not a thing json! can do, so patch it in.
        let mut source = source;
        source["items"][0]["asset"]["hash"] = json!("a".repeat(64));

        let once = normalize(&source);
        let twice = normalize(&serialize(&once));

        assert_eq!(twice.title, "Kitchen");
        assert_eq!(twice.items.len(), 2);
        assert_eq!(twice.items[0].name, "kitchen-window.jpg");
        assert_eq!(twice.items[1].kind, ItemType::Note);
        assert_eq!(twice.items[1].note_text(), Some("the big one does not fit"));
        assert_eq!(twice.media_fit, "cover");
        assert_eq!(twice.palette_sources, 6);
        assert_eq!(twice.arrangements.desktop, "spiral");
        assert_eq!(twice.connections.len(), 1);
        assert_eq!(twice.connections[0].meta.dir, ConnDir::Fwd);
        assert_eq!(twice.connections[0].meta.label.as_deref(), Some("goes with"));
        assert_eq!(twice.tour, vec!["k3f9a2"]);
        // Serialising twice must be a fixed point, or every save churns the file.
        assert_eq!(serialize(&once), serialize(&twice));
    }
}
