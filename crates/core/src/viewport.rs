//! The camera: where you are looking, and how close.
//!
//! This is the one module in the crate that knows about screens, and it exists
//! so that nothing else has to. Two conversions and their inverses live here,
//! and the y-flip — world y up, screen y down — happens in exactly these four
//! functions and nowhere else.
//!
//! **`pan` is the world point under the centre of the view**, not a translation
//! offset. That is the original's convention (`canvas/viewport.ts`) and it is
//! what a `.mbrd` stores, so changing it here would silently move the camera on
//! every board ever saved. It is also the convention that makes `zoom_at` and
//! `fit` short: both are "put this world point in the middle".

use crate::geometry::{point, Point, Rect};

/// The scale the interface prints as "100%".
///
/// The original carries a `BASE_ZOOM` that the printed percentage divides by,
/// so that a board left at 100% could record something other than `1.0`. It has
/// been `1` for the life of the format; the constant stays because the file
/// stores a **raw scale**, and a reader that multiplies it by a hundred to get
/// the percentage is making an assumption that is only accidentally true.
pub const BASE_ZOOM: f32 = 1.0;
/// 10%, as printed.
pub const MIN_ZOOM: f32 = 0.1 * BASE_ZOOM;
/// 500%, as printed.
pub const MAX_ZOOM: f32 = 5.0 * BASE_ZOOM;

/// How big a view is, in screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    /// The world point under the centre of the view.
    pub pan: Point,
    /// The raw world-to-screen scale. Not a percentage.
    pub zoom: f32,
    /// The size of the surface being drawn to.
    pub size: ViewSize,
}

impl Default for Viewport {
    fn default() -> Self {
        Self { pan: Point::default(), zoom: BASE_ZOOM, size: ViewSize { width: 1.0, height: 1.0 } }
    }
}

impl Viewport {
    /// The centre of the surface, in screen pixels from its top-left.
    fn centre(&self) -> Point {
        point(self.size.width / 2.0, self.size.height / 2.0)
    }

    /// A world point, in screen pixels from the surface's top-left.
    pub fn to_screen(&self, world: Point) -> Point {
        let c = self.centre();
        point(
            (world.x - self.pan.x) * self.zoom + c.x,
            // The flip. World y is up, screen y is down.
            (self.pan.y - world.y) * self.zoom + c.y,
        )
    }

    /// A screen point, in world units.
    pub fn to_world(&self, screen: Point) -> Point {
        let c = self.centre();
        let (sx, sy) = (screen.x - c.x, screen.y - c.y);
        point(sx / self.zoom + self.pan.x, self.pan.y - sy / self.zoom)
    }

    /// The world rectangle currently on screen.
    pub fn visible(&self) -> Rect {
        let hw = self.size.width / 2.0 / self.zoom;
        let hh = self.size.height / 2.0 / self.zoom;
        Rect::new(self.pan.x - hw, self.pan.y - hh, self.pan.x + hw, self.pan.y + hh)
    }

    /// Drag the board by a screen-space delta.
    ///
    /// The y term adds where the x term subtracts, and that is the flip again
    /// rather than a typo: dragging the mouse *down* the screen should bring
    /// world content from above into view, which means the camera moves up.
    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan.x -= dx / self.zoom;
        self.pan.y += dy / self.zoom;
    }

    /// Zoom by a factor, keeping the world point under `anchor` where it is.
    ///
    /// This is what a wheel gesture wants: the thing under the cursor is the
    /// thing you are aiming at, so it must not slide out from under you. Read
    /// the world point first, apply the clamped zoom, then move the camera so
    /// that the same world point lands back on the same pixel.
    pub fn zoom_by(&mut self, factor: f32, anchor: Point) {
        let world = self.to_world(anchor);
        let z = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
        // A clamped-out zoom is not a no-op for `pan` unless we stop here: the
        // arithmetic below would still shift the camera by the rounding on a
        // factor that did nothing.
        if z == self.zoom {
            return;
        }
        self.zoom = z;
        let c = self.centre();
        let (sx, sy) = (anchor.x - c.x, anchor.y - c.y);
        self.pan.x = world.x - sx / z;
        self.pan.y = world.y + sy / z;
    }

    /// Put the origin back in the middle at 100%.
    pub fn home(&mut self) {
        self.pan = Point::default();
        self.zoom = BASE_ZOOM;
    }

    /// Frame a rectangle, leaving `pad` screen pixels around it.
    ///
    /// `max_zoom` caps how far in the fit may go. Passing `BASE_ZOOM` is what an
    /// opening view wants — a board smaller than the window should open at 100%
    /// rather than magnified until three cards fill the screen.
    pub fn fit(&mut self, bounds: Option<Rect>, pad: f32, max_zoom: f32) {
        let Some(b) = bounds else {
            // Nothing to fit is not a failure and not a zero-sized box: it is
            // the origin at 100%.
            self.home();
            return;
        };
        let avail_w = (self.size.width - pad * 2.0).max(1.0);
        let avail_h = (self.size.height - pad * 2.0).max(1.0);
        let z = (avail_w / b.width().max(1.0)).min(avail_h / b.height().max(1.0));
        self.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM.min(max_zoom));
        self.pan = b.centre();
    }

    /// The zoom the corner prints, as a percentage.
    pub fn percent(&self) -> f32 {
        self.zoom / BASE_ZOOM * 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport {
            pan: Point::default(),
            zoom: 1.0,
            size: ViewSize { width: 800.0, height: 600.0 },
        }
    }

    #[test]
    fn origin_sits_at_the_centre_of_the_view() {
        let s = vp().to_screen(point(0.0, 0.0));
        assert_eq!(s, point(400.0, 300.0));
    }

    #[test]
    fn world_y_points_up() {
        // A card above the origin draws nearer the top of the screen, which is
        // a *smaller* screen y. This is the assertion that catches a sign flip.
        let s = vp().to_screen(point(0.0, 100.0));
        assert!(s.y < 300.0, "world y=100 should be above centre, got {}", s.y);
    }

    #[test]
    fn screen_and_world_round_trip() {
        let mut v = vp();
        v.pan = point(37.0, -12.0);
        v.zoom = 2.5;
        let w = point(123.0, -456.0);
        let back = v.to_world(v.to_screen(w));
        assert!((back.x - w.x).abs() < 0.001 && (back.y - w.y).abs() < 0.001);
    }

    #[test]
    fn zooming_holds_the_point_under_the_cursor() {
        let mut v = vp();
        let anchor = point(650.0, 120.0);
        let before = v.to_world(anchor);
        v.zoom_by(1.4, anchor);
        let after = v.to_world(anchor);
        assert!((before.x - after.x).abs() < 0.001, "x slid: {before:?} -> {after:?}");
        assert!((before.y - after.y).abs() < 0.001, "y slid: {before:?} -> {after:?}");
    }

    #[test]
    fn zoom_is_clamped_at_both_ends() {
        let mut v = vp();
        for _ in 0..50 {
            v.zoom_by(2.0, point(400.0, 300.0));
        }
        assert_eq!(v.zoom, MAX_ZOOM);
        for _ in 0..100 {
            v.zoom_by(0.5, point(400.0, 300.0));
        }
        assert_eq!(v.zoom, MIN_ZOOM);
    }

    #[test]
    fn fitting_nothing_goes_home_rather_than_dividing_by_zero() {
        let mut v = vp();
        v.pan = point(900.0, 900.0);
        v.zoom = 3.0;
        v.fit(None, 80.0, MAX_ZOOM);
        assert_eq!(v.pan, point(0.0, 0.0));
        assert_eq!(v.zoom, BASE_ZOOM);
    }

    #[test]
    fn fit_centres_on_the_bounds_and_respects_its_ceiling() {
        let mut v = vp();
        // A small board, which without the ceiling would fit at well over 100%.
        v.fit(Some(Rect::new(-50.0, -50.0, 50.0, 50.0)), 80.0, BASE_ZOOM);
        assert_eq!(v.pan, point(0.0, 0.0));
        assert_eq!(v.zoom, BASE_ZOOM);
    }

    #[test]
    fn dragging_down_moves_the_camera_up() {
        let mut v = vp();
        v.pan_by(0.0, 40.0);
        assert!(v.pan.y > 0.0);
    }
}
