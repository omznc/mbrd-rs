//! mbrd's board model and file format, with no UI attached.
//!
//! This crate is deliberately the bottom of the stack and knows nothing about
//! the window it will eventually be drawn in. That is not tidiness for its own
//! sake — it is what makes the format testable. Feeding [`schema::normalize`] a
//! broken object and asserting the board it returns needs no window, no event
//! loop and no GPU, so the tests here run in a second and run in CI.
//!
//! The layering the original enforces with a test is kept here by construction:
//!
//! ```text
//! geometry, guides, history, motion <- model <- {schema, viewport, naming} <- state <- mbrd <- {preview, facts}
//! ```
//!
//! and the UI crate sits above all of it. **Nothing in here may depend on the
//! UI crate**, which in Cargo is not a convention but a fact — a cycle would
//! not build.
//!
//! ## Where a change goes
//!
//! - **A new field in the file** → [`schema::normalize`] *and* [`schema::serialize`],
//!   in the same commit. A field read but not written is lost on the next save.
//! - **A new item type** → a variant in [`model::ItemType`], and nothing else is
//!   required: unknown types already round-trip.
//! - **A new per-type extra** → nothing. `meta` carries unknown keys through
//!   untouched, which is the whole point of it being a map.
//! - **A new small field of the board** → [`schema::normalize`], [`schema::serialize`]
//!   *and* [`schema::REST_FIELDS`], in the same commit. A field the ledger does
//!   not record is a field undo cannot take back.
//! - **A new way to change the board** → nothing beyond going through
//!   [`state::BoardState::edit`]. There is no second place to register it, which
//!   is the whole point of there being one door.
//! - **A new top-level directory in the archive** → [`mbrd::read`] and
//!   [`mbrd::write`], and it is free to add: a reader that finds a directory it
//!   does not know walks past it.

pub mod align;
pub mod arrange;
pub mod facts;
pub mod fence;
pub mod geometry;
pub mod guides;
pub mod history;
pub mod index;
pub mod markdown;
pub mod mbrd;
pub mod media;
pub mod mesh;
pub mod model;
pub mod motion;
pub mod naming;
pub mod paper;
pub mod peaks;
pub mod preview;
pub mod rope;
pub mod route;
pub mod schema;
pub mod snap;
pub mod sound;
pub mod state;
pub mod viewport;

pub use history::Timeline;
pub use mbrd::{Document, Manifest};
pub use model::{Board, Item, ItemType, LayoutMode};
pub use state::BoardState;
pub use viewport::Viewport;
