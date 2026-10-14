//! The headset browser panel: a list of servers, shares, folders or videos,
//! drawn into a canvas that is shown on an OpenXR quad layer.

use super::canvas::{Canvas, Fonts, Rgb};
use super::form::{self, Form};
use crate::media::Thumb;
use crate::playability::Verdict;
use crate::vr::{Layout, Projection, Stereo};
use std::sync::Arc;

pub const WIDTH: u32 = 1600;
/// Two header rows (path, then tools) on top of the 1600x1000 list area.
pub const HEIGHT: u32 = 1070;
const HEADER: f32 = 190.0;
/// Top of the tool buttons: the header's second row. The first holds the path.
const TOOL_Y: f32 = 106.0;
/// Top and height of a breadcrumb's hit and hover box, in the first row.
const CRUMB_Y: f32 = 36.0;
const CRUMB_H: f32 = 62.0;
const BOTTOM: f32 = 24.0;
/// Row heights: plain, and with a thumbnail slot.
const ROW: f32 = 88.0;
const ROW_THUMB: f32 = 140.0;
/// The thumbnail slot at the left of a tall row, and its corner radius.
const SLOT_W: f32 = crate::thumb_cache::THUMB_W as f32;
const SLOT_H: f32 = crate::thumb_cache::THUMB_H as f32;
const SLOT_R: f32 = 10.0;
const PAD: f32 = 32.0;
const CRUMB_SIZE: f32 = 40.0;
const CRUMB_SEP: &str = "  ›  ";
/// Row actions, from the right edge of a row.
const ACTION_W: f32 = 150.0;
/// The scrollbar's grab zone at the right edge of the list.
const SCROLL_W: f32 = 64.0;
/// Header tool buttons (e.g. "Cancel", "Delete 3").
/// Wide enough for "Sort: Unwatched".
const TOOL_W: f32 = 264.0;
const TOOL_GAP: f32 = 12.0;
/// A square icon button in the header.
const ICON_TOOL_W: f32 = 84.0;
/// The watched-state mark: its radius, ring width, and the room it takes in a row.
const MARK_R: f32 = 12.0;
const MARK_RING: f32 = 3.5;
const MARK_W: f32 = 40.0;
/// Shown in place of the crumbs the path has no room for.
const ELLIPSIS: &str = "…";
/// Gap between the path and the build label.
const BUILD_GAP: f32 = 40.0;

/// Shown top right so installs can be told apart.
pub const BUILD: &str = env!("VIDEO_BOX_BUILD");

const BG: Rgb = [0x15, 0x17, 0x1c];
const ROW_BG: Rgb = [0x1d, 0x21, 0x28];
/// Behind the icon on a thumbnail, and in the slot until one arrives.
const SLOT_BG: Rgb = [0x11, 0x13, 0x17];
const HOVER: Rgb = [0x2c, 0x33, 0x40];
const TEXT: Rgb = [0xe8, 0xea, 0xed];
const SUBTLE: Rgb = [0x9a, 0xa0, 0xa6];
const FAINT: Rgb = [0x5f, 0x63, 0x68];
const ACCENT: Rgb = [0x4f, 0x8c, 0xff];
const GREEN: Rgb = [0x34, 0xc7, 0x59];
const YELLOW: Rgb = [0xff, 0xcc, 0x00];
const ORANGE: Rgb = [0xff, 0x95, 0x00];
const RED: Rgb = [0xff, 0x45, 0x3a];
const GREY: Rgb = [0x5f, 0x63, 0x68];

#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    Server,
    Share,
    Folder,
    /// A video; `None` while its playability is still being checked.
    Video(Option<Verdict>),
    /// A 3D video for a flat screen (side by side or top/bottom): glasses.
    Video3d(Option<Verdict>),
    /// A VR180, VR360 or fisheye video: a headset.
    VideoVr(Option<Verdict>),
    /// A file that could not be read.
    Broken,
    /// A file that isn't a video.
    File,
    Add,
    /// A gear: the Settings screen.
    Settings,
    /// A slider: one setting.
    Slider,
    /// Close the app.
    Exit,
}

impl Icon {
    /// A video's mark: its shape says flat, 3D or VR, its colour how well it plays.
    pub fn video(verdict: Option<Verdict>, layout: Option<&Layout>) -> Icon {
        match layout {
            Some(l) if l.projection != Projection::Flat => Icon::VideoVr(verdict),
            Some(l) if l.stereo != Stereo::Mono => Icon::Video3d(verdict),
            _ => Icon::Video(verdict),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// A server's address, login and name.
    Edit,
    /// Delete a saved server.
    Delete,
}

impl Action {
    fn width(self) -> f32 {
        ACTION_W
    }

    fn label(self) -> &'static str {
        match self {
            Action::Edit => "Edit",
            Action::Delete => "Delete",
        }
    }
}

/// How much of a video has been watched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Watch {
    /// Never played: a filled circle.
    Unwatched,
    /// Left part way, to resume: a ring, half filled.
    Partial,
    /// Played to the end: an empty ring.
    Watched,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub icon: Icon,
    pub label: String,
    pub detail: String,
    pub right: String,
    /// Buttons at the right end, left to right.
    pub actions: Vec<Action>,
    /// A checkbox in place of the icon (selecting what to delete).
    pub checked: Option<bool>,
    /// Greyed out: listed, but not something to open (non-video files).
    pub dimmed: bool,
    /// Outlined: the entry just come back out of.
    pub outlined: bool,
    /// A video's picture (for views with [`View::thumbnails`]).
    pub thumbnail: Option<Arc<Thumb>>,
    /// How much of a video was watched (a line along the row's foot).
    pub progress: Option<f32>,
    /// A video's watched state, drawn as a mark (`None`: not a video).
    pub watch: Option<Watch>,
}

impl Row {
    pub fn new(icon: Icon, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
            detail: String::new(),
            right: String::new(),
            actions: Vec::new(),
            checked: None,
            dimmed: false,
            outlined: false,
            thumbnail: None,
            progress: None,
            watch: None,
        }
    }

    /// A video (or one that can't be read): what gets a thumbnail.
    fn is_video(&self) -> bool {
        matches!(
            self.icon,
            Icon::Video(_) | Icon::Video3d(_) | Icon::VideoVr(_) | Icon::Broken
        )
    }
}

/// A button in the header, right-aligned.
#[derive(Clone, Debug, PartialEq)]
pub struct Tool {
    pub label: String,
    pub danger: bool,
    /// Drawn as a square icon button instead of the label.
    pub icon: Option<ToolIcon>,
    /// Toggled on (e.g. thumbnails).
    pub active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolIcon {
    /// Thumbnails in the video list.
    Thumbnails,
}

impl Tool {
    pub fn text(label: impl Into<String>, danger: bool) -> Self {
        Self {
            label: label.into(),
            danger,
            icon: None,
            active: false,
        }
    }

    pub fn icon(icon: ToolIcon, active: bool) -> Self {
        Self {
            label: String::new(),
            danger: false,
            icon: Some(icon),
            active,
        }
    }

    fn width(&self) -> f32 {
        if self.icon.is_some() {
            ICON_TOOL_W
        } else {
            TOOL_W
        }
    }
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub title: String,
    pub body: Vec<String>,
    /// Left to right; the last one is the primary action.
    pub buttons: Vec<String>,
    /// The primary button is destructive (drawn red).
    pub danger: bool,
}

#[derive(Clone, Debug, Default)]
pub struct View {
    /// Breadcrumb path; the last entry is the current place.
    pub crumbs: Vec<String>,
    /// Shown in place of the breadcrumbs (e.g. "2 selected" while the
    /// header holds the selection's tools).
    pub heading: Option<String>,
    pub rows: Vec<Row>,
    /// Shown instead of rows (loading, errors, empty folders).
    pub status: Option<String>,
    /// Small line at the bottom (e.g. "Opening …").
    pub notice: Option<String>,
    pub dialog: Option<Dialog>,
    pub form: Option<Form>,
    pub tools: Vec<Tool>,
    /// First visible row (fractional while scrolling).
    pub scroll: f32,
    /// Tall rows with a thumbnail slot (a folder of videos, thumbnails on).
    pub thumbnails: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Row(usize),
    Crumb(usize),
    RowAction(usize, Action),
    Tool(usize),
    /// The scrollbar: scroll with [`scroll_at`].
    ScrollBar,
    DialogButton(usize),
    Form(form::Hit),
    Nothing,
}

/// The thin progress bar under the header line, from `from` to `to`
/// (fractions of its full width): (x, y, w, h) in canvas pixels. The app
/// shows it as its own layer, so it can move without redrawing the panel.
pub fn progress_rect(from: f32, to: f32) -> (f32, f32, f32, f32) {
    let full = WIDTH as f32 - 2.0 * PAD;
    let (from, to) = (from.clamp(0.0, 1.0), to.clamp(0.0, 1.0));
    (PAD + full * from, HEADER - 8.0, full * (to - from), 3.0)
}

pub fn list_top() -> f32 {
    HEADER
}

impl View {
    /// Height of one row, in canvas pixels.
    pub fn row_h(&self) -> f32 {
        if self.thumbnails { ROW_THUMB } else { ROW }
    }

    /// How many rows fit in the list (fractional).
    pub fn visible_rows(&self) -> f32 {
        (HEIGHT as f32 - HEADER - BOTTOM) / self.row_h()
    }

    pub fn max_scroll(&self) -> f32 {
        (self.rows.len() as f32 - self.visible_rows()).max(0.0)
    }

    pub fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }
}

pub type Rect = (f32, f32, f32, f32);

fn inside((x, y, w, h): Rect, px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

fn dialog_buttons(n: usize) -> Vec<Rect> {
    let (w, h, gap) = (280.0, 80.0, 24.0);
    let total = n as f32 * w + (n.saturating_sub(1)) as f32 * gap;
    let x0 = WIDTH as f32 / 2.0 - total / 2.0;
    (0..n)
        .map(|i| (x0 + i as f32 * (w + gap), HEIGHT as f32 - 250.0, w, h))
        .collect()
}

fn tool_rect(view: &View, k: usize) -> Rect {
    let right: f32 = view.tools[k..].iter().map(|t| t.width() + TOOL_GAP).sum();
    (
        WIDTH as f32 - PAD - right + TOOL_GAP,
        TOOL_Y,
        view.tools[k].width(),
        62.0,
    )
}

fn scroll_track() -> (f32, f32) {
    (HEADER, HEIGHT as f32 - BOTTOM - HEADER)
}

fn thumb_height(view: &View) -> f32 {
    (view.visible_rows() / view.rows.len().max(1) as f32 * scroll_track().1).max(60.0)
}

/// The scroll position that puts the scrollbar's thumb under canvas `y`.
pub fn scroll_at(view: &View, y: f32) -> f32 {
    let (top, track) = scroll_track();
    let thumb = thumb_height(view);
    let f = ((y - top - thumb / 2.0) / (track - thumb).max(1.0)).clamp(0.0, 1.0);
    f * view.max_scroll()
}

/// Rows moved by a vertical drag of `dy` canvas pixels.
pub fn rows_for_drag(view: &View, dy: f32) -> f32 {
    dy / view.row_h()
}

/// One breadcrumb as drawn: the crumb it stands for and its horizontal extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrumbSpan {
    /// Index into [`View::crumbs`]; for the "…" that stands in for dropped
    /// crumbs, the nearest dropped one (the parent of the first shown after it).
    pub index: usize,
    pub elided: bool,
    pub x0: f32,
    pub x1: f32,
}

/// Width the path may use: the first row, left of the build label.
fn path_room(fonts: &mut Fonts) -> f32 {
    WIDTH as f32 - 2.0 * PAD - fonts.measure(BUILD, 20.0) - BUILD_GAP
}

/// The breadcrumbs as drawn, left to right. The path gets the whole first row
/// (less the build label); when it does not fit, the oldest crumbs after the
/// first are replaced by one "…" and the last is shortened if it still must.
pub fn crumb_spans(view: &View, fonts: &mut Fonts) -> Vec<CrumbSpan> {
    let n = view.crumbs.len();
    if n == 0 {
        return Vec::new();
    }
    let room = path_room(fonts);
    let sep = fonts.measure(CRUMB_SEP, CRUMB_SIZE);
    let widths: Vec<f32> = view
        .crumbs
        .iter()
        .map(|c| fonts.measure(c, CRUMB_SIZE))
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + sep * (n - 1) as f32;
    // Which crumbs show: all, or the first, an ellipsis, and the newest that fit.
    let (first_shown, elide) = if total <= room || n <= 2 {
        (1, false)
    } else {
        let dots = fonts.measure(ELLIPSIS, CRUMB_SIZE);
        let mut used = widths[0] + sep + dots + sep + widths[n - 1];
        let mut from = n - 1;
        while from > 1 && used + sep + widths[from - 1] <= room {
            used += sep + widths[from - 1];
            from -= 1;
        }
        (from, true)
    };
    let mut items: Vec<(usize, bool, f32)> = vec![(0, false, widths[0])];
    if elide {
        let dots = fonts.measure(ELLIPSIS, CRUMB_SIZE);
        items.push((first_shown - 1, true, dots));
    }
    items.extend((first_shown..n).map(|i| (i, false, widths[i])));
    // Whatever is still too wide (a very long last name) is clipped.
    let fixed: f32 = items.iter().map(|i| i.2).sum::<f32>() + sep * (items.len() - 1) as f32;
    let last = items.len() - 1;
    if fixed > room && items.len() > 1 {
        let others = fixed - items[last].2;
        items[last].2 = (room - others).max(120.0);
    }
    let mut x = PAD;
    items
        .into_iter()
        .map(|(index, elided, w)| {
            let span = CrumbSpan {
                index,
                elided,
                x0: x,
                x1: x + w,
            };
            x += w + sep;
            span
        })
        .collect()
}

fn row_rect(view: &View, i: usize) -> Rect {
    let row_h = view.row_h();
    let y = HEADER + (i as f32 - view.scroll) * row_h;
    (
        PAD,
        y + 4.0,
        WIDTH as f32 - 2.0 * PAD - SCROLL_W,
        row_h - 8.0,
    )
}

/// The `k`th action button of row `i`.
fn action_rect(view: &View, i: usize, k: usize) -> Rect {
    let (x, y, w, h) = row_rect(view, i);
    let right = x + w - 8.0;
    let actions = &view.rows[i].actions;
    let from_right: f32 = actions[k..].iter().map(|a| a.width() + 10.0).sum();
    (
        right - from_right + 10.0,
        y + 6.0,
        actions[k].width(),
        h - 12.0,
    )
}

/// What the D-pad moves between besides the list's rows, with their
/// rectangles: a form's fields and keys, a dialog's buttons (left to right),
/// or the header's breadcrumbs and tools (left to right).
pub fn fixed_targets(view: &View, fonts: &mut Fonts) -> Vec<(Hit, Rect)> {
    if let Some(f) = &view.form {
        return form::targets(f, WIDTH as f32)
            .into_iter()
            .map(|(h, r)| (Hit::Form(h), r))
            .collect();
    }
    if let Some(dialog) = &view.dialog {
        return dialog_buttons(dialog.buttons.len())
            .into_iter()
            .enumerate()
            .map(|(i, r)| (Hit::DialogButton(i), r))
            .collect();
    }
    // The last crumb is where we are: not a link. A heading replaces the path.
    let last = view.crumbs.len().saturating_sub(1);
    let crumbs: Vec<_> = if view.heading.is_some() {
        Vec::new()
    } else {
        crumb_spans(view, fonts)
            .into_iter()
            .filter(|c| c.index < last)
            .map(|c| {
                (
                    Hit::Crumb(c.index),
                    (c.x0 - 8.0, CRUMB_Y, c.x1 - c.x0 + 16.0, CRUMB_H),
                )
            })
            .collect()
    };
    let tools = (0..view.tools.len()).map(|k| (Hit::Tool(k), tool_rect(view, k)));
    crumbs.into_iter().chain(tools).collect()
}

/// Row `i`'s targets, left to right: the row itself, then its actions.
pub fn row_targets(view: &View, i: usize) -> Vec<(Hit, Rect)> {
    let Some(row) = view.rows.get(i) else {
        return Vec::new();
    };
    let (x, y, _, h) = row_rect(view, i);
    // The row: its label, left of any buttons.
    let mut out = vec![(Hit::Row(i), (x, y, 400.0, h))];
    for (k, action) in row.actions.iter().enumerate() {
        out.push((Hit::RowAction(i, *action), action_rect(view, i, k)));
    }
    out
}

/// What the pointer at canvas pixel (x, y) would activate.
pub fn hit(view: &View, fonts: &mut Fonts, x: f32, y: f32) -> Hit {
    if let Some(f) = &view.form {
        return Hit::Form(form::hit(f, fonts, WIDTH as f32, x, y));
    }
    let crumbs = crumb_spans(view, fonts);
    if let Some(dialog) = &view.dialog {
        return dialog_buttons(dialog.buttons.len())
            .into_iter()
            .position(|r| inside(r, x, y))
            .map_or(Hit::Nothing, Hit::DialogButton);
    }
    if y < HEADER - 10.0 {
        if let Some(k) = (0..view.tools.len()).find(|&k| inside(tool_rect(view, k), x, y)) {
            return Hit::Tool(k);
        }
        if view.heading.is_some() {
            return Hit::Nothing;
        }
        // The last crumb is where we are: not a link. They sit in the first
        // row; the tools (above) in the second.
        let last = view.crumbs.len().saturating_sub(1);
        return crumbs
            .iter()
            .find(|c| {
                x >= c.x0 - 8.0 && x <= c.x1 + 8.0 && y > CRUMB_Y - 6.0 && y < CRUMB_Y + CRUMB_H
            })
            .filter(|c| c.index < last)
            .map_or(Hit::Nothing, |c| Hit::Crumb(c.index));
    }
    if view.status.is_some() || y > HEIGHT as f32 - BOTTOM || x < PAD || x > WIDTH as f32 - PAD {
        return Hit::Nothing;
    }
    if x > WIDTH as f32 - PAD - SCROLL_W + 8.0 {
        return if view.max_scroll() > 0.0 {
            Hit::ScrollBar
        } else {
            Hit::Nothing
        };
    }
    let index = ((y - HEADER) / view.row_h() + view.scroll).floor();
    if index < 0.0 || index as usize >= view.rows.len() {
        return Hit::Nothing;
    }
    let i = index as usize;
    let row = &view.rows[i];
    for (k, action) in row.actions.iter().enumerate() {
        if inside(action_rect(view, i, k), x, y) {
            return Hit::RowAction(i, *action);
        }
    }
    Hit::Row(i)
}

/// `bg` is the row behind the icon, for cut-outs.
fn draw_icon(canvas: &mut Canvas, icon: &Icon, cx: f32, cy: f32, bg: Rgb) {
    draw_icon_scaled(canvas, icon, cx, cy, 1.0, bg);
}

/// [`draw_icon`] at `scale` times its size.
fn draw_icon_scaled(canvas: &mut Canvas, icon: &Icon, cx: f32, cy: f32, scale: f32, bg: Rgb) {
    let mut pen = Pen {
        canvas,
        cx,
        cy,
        scale,
    };
    match icon {
        Icon::Server => {
            pen.rect(-22.0, -20.0, 44.0, 16.0, 4.0, ACCENT);
            pen.rect(-22.0, 2.0, 44.0, 16.0, 4.0, ACCENT);
            pen.circle(13.0, -12.0, 3.0, BG);
            pen.circle(13.0, 10.0, 3.0, BG);
        }
        Icon::Share | Icon::Folder => {
            let color = if *icon == Icon::Share {
                ACCENT
            } else {
                [0x8a, 0xb4, 0xf8]
            };
            pen.rect(-24.0, -18.0, 20.0, 10.0, 3.0, color);
            pen.rect(-24.0, -12.0, 48.0, 32.0, 4.0, color);
        }
        Icon::Video(verdict) => pen.circle(0.0, 0.0, 16.0, verdict_color(*verdict)),
        Icon::Video3d(verdict) => {
            // Glasses: two lenses on a bar, with short arms.
            let color = verdict_color(*verdict);
            pen.rect(-26.0, -10.0, 52.0, 5.0, 2.0, color);
            pen.rect(-24.0, -10.0, 21.0, 19.0, 6.0, color);
            pen.rect(3.0, -10.0, 21.0, 19.0, 6.0, color);
        }
        Icon::VideoVr(verdict) => {
            // A headset from the front: visor with two lenses and a nose gap,
            // and the strap at the sides.
            let color = verdict_color(*verdict);
            pen.rect(-29.0, -6.0, 58.0, 8.0, 3.0, color);
            pen.rect(-24.0, -17.0, 48.0, 33.0, 10.0, color);
            pen.circle(-11.0, -2.0, 7.0, bg);
            pen.circle(11.0, -2.0, 7.0, bg);
            pen.circle(0.0, 17.0, 7.0, bg);
        }
        Icon::Broken => {
            pen.circle(0.0, 0.0, 16.0, RED);
            pen.rect(-9.0, -3.0, 18.0, 6.0, 2.0, BG);
        }
        Icon::File => {
            // A page with a folded top-right corner.
            pen.rect(-16.0, -20.0, 32.0, 40.0, 4.0, FAINT);
            pen.rect(-12.0, -16.0, 24.0, 32.0, 2.0, ROW_BG);
            pen.rect(2.0, -21.0, 15.0, 15.0, 0.0, ROW_BG);
            pen.rect(2.0, -20.0, 4.0, 14.0, 1.0, FAINT);
            pen.rect(2.0, -10.0, 14.0, 4.0, 1.0, FAINT);
        }
        Icon::Add => {
            pen.rect(-3.0, -18.0, 6.0, 36.0, 3.0, ACCENT);
            pen.rect(-18.0, -3.0, 36.0, 6.0, 3.0, ACCENT);
        }
        Icon::Settings => {
            // Eight teeth around a wheel with a hole.
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::FRAC_PI_4;
                let (x, y) = (16.0 * a.cos(), 16.0 * a.sin());
                pen.rect(x - 5.0, y - 5.0, 10.0, 10.0, 2.0, SUBTLE);
            }
            pen.circle(0.0, 0.0, 15.0, SUBTLE);
            pen.circle(0.0, 0.0, 6.0, bg);
        }
        Icon::Exit => {
            // A power symbol: a ring open at the top with a bar through the gap.
            pen.circle(0.0, 2.0, 17.0, FAINT);
            pen.circle(0.0, 2.0, 11.0, ROW_BG);
            pen.rect(-7.0, -18.0, 14.0, 14.0, 0.0, ROW_BG);
            pen.rect(-3.0, -20.0, 6.0, 22.0, 3.0, FAINT);
        }
        Icon::Slider => {
            pen.rect(-20.0, -2.0, 40.0, 4.0, 2.0, FAINT);
            pen.rect(-20.0, -2.0, 22.0, 4.0, 2.0, ACCENT);
            pen.circle(2.0, 0.0, 8.0, ACCENT);
        }
    }
}

/// Draws relative to a centre, at a scale.
struct Pen<'a> {
    canvas: &'a mut Canvas,
    cx: f32,
    cy: f32,
    scale: f32,
}

impl Pen<'_> {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgb) {
        let s = self.scale;
        let (x, y) = (self.cx + x * s, self.cy + y * s);
        self.canvas.rect(x, y, w * s, h * s, radius * s, color);
    }

    fn circle(&mut self, x: f32, y: f32, radius: f32, color: Rgb) {
        let s = self.scale;
        let (x, y) = (self.cx + x * s, self.cy + y * s);
        self.canvas.circle(x, y, radius * s, color);
    }

    fn ring(&mut self, x: f32, y: f32, radius: f32, thickness: f32, color: Rgb) {
        let s = self.scale;
        let (x, y) = (self.cx + x * s, self.cy + y * s);
        self.canvas.ring(x, y, radius * s, thickness * s, color);
    }

    /// The left half of a disc.
    fn half_disc(&mut self, x: f32, y: f32, radius: f32, color: Rgb) {
        let s = self.scale;
        let (x, y) = (self.cx + x * s, self.cy + y * s);
        self.canvas.half_disc(x, y, radius * s, color);
    }
}

/// A video's watched state, centred on (cx, cy): filled when unwatched, a ring
/// half filled when left part way, an empty ring when watched. Blue, so it is
/// not mistaken for the verdict colours of the video's own mark.
fn draw_watch(canvas: &mut Canvas, watch: Watch, cx: f32, cy: f32) {
    let mut pen = Pen {
        canvas,
        cx,
        cy,
        scale: 1.0,
    };
    match watch {
        Watch::Unwatched => pen.circle(0.0, 0.0, MARK_R, ACCENT),
        Watch::Partial => {
            pen.ring(0.0, 0.0, MARK_R, MARK_RING, ACCENT);
            // Runs under the ring's inner edge, so there is no seam.
            pen.half_disc(0.0, 0.0, MARK_R - MARK_RING + 1.0, ACCENT);
        }
        Watch::Watched => pen.ring(0.0, 0.0, MARK_R, MARK_RING, ACCENT),
    }
}

/// How well a video plays; grey while that is being checked.
fn verdict_color(verdict: Option<Verdict>) -> Rgb {
    match verdict {
        Some(Verdict::Hardware) => GREEN,
        Some(Verdict::Software) => YELLOW,
        Some(Verdict::SoftwareMarginal) => ORANGE,
        Some(Verdict::Unplayable) => RED,
        None => GREY,
    }
}

/// A checkbox, `fill` when ticked, cut out of `background` when not.
pub(super) fn draw_checkbox(
    canvas: &mut Canvas,
    cx: f32,
    cy: f32,
    checked: bool,
    fill: Rgb,
    background: Rgb,
) {
    if checked {
        canvas.rect(cx - 18.0, cy - 18.0, 36.0, 36.0, 7.0, fill);
        // A tick from two strokes of small squares.
        for i in 0..8 {
            let t = i as f32;
            canvas.rect(cx - 11.0 + t, cy - 1.0 + t, 5.0, 5.0, 1.0, TEXT);
        }
        for i in 0..14 {
            let t = i as f32;
            canvas.rect(cx - 4.0 + t, cy + 6.0 - t * 1.2, 5.0, 5.0, 1.0, TEXT);
        }
    } else {
        canvas.rect(cx - 18.0, cy - 18.0, 36.0, 36.0, 7.0, SUBTLE);
        canvas.rect(cx - 14.0, cy - 14.0, 28.0, 28.0, 5.0, background);
    }
}

/// The left slot of a tall row: a video's thumbnail (or a dark placeholder)
/// with its mark as a badge in the bottom-left corner and its watched state
/// in the top-right, or another entry's icon centred. `slot` is its top-left,
/// `cy` the row's vertical centre.
fn draw_slot(canvas: &mut Canvas, row: &Row, slot: (f32, f32), cy: f32, bg: Rgb) {
    let (sx, sy) = slot;
    let cx = sx + SLOT_W / 2.0;
    if !row.is_video() {
        match row.checked {
            Some(checked) => draw_checkbox(canvas, cx, cy, checked, RED, bg),
            None => draw_icon(canvas, &row.icon, cx, cy, bg),
        }
        return;
    }
    match &row.thumbnail {
        Some(thumb) => {
            // Centred, should it ever differ from the slot's size.
            let x = sx + (SLOT_W - thumb.width as f32) / 2.0;
            let y = sy + (SLOT_H - thumb.height as f32) / 2.0;
            canvas.image(x.round() as i32, y.round() as i32, thumb, SLOT_R);
            // The mark on a dark backing, so it reads over any picture.
            let (bw, bh) = (62.0, 40.0);
            let (bx, by) = (sx + 6.0, sy + SLOT_H - 6.0 - bh);
            canvas.rect(bx, by, bw, bh, 10.0, SLOT_BG);
            let (ix, iy) = (bx + bw / 2.0, by + bh / 2.0);
            draw_icon_scaled(canvas, &row.icon, ix, iy, 0.8, SLOT_BG);
            if let Some(checked) = row.checked {
                // Top-left, on its own backing.
                canvas.rect(sx + 6.0, sy + 6.0, 48.0, 48.0, 10.0, SLOT_BG);
                draw_checkbox(canvas, sx + 30.0, sy + 30.0, checked, RED, SLOT_BG);
            }
        }
        None => {
            canvas.rect(sx, sy, SLOT_W, SLOT_H, SLOT_R, SLOT_BG);
            match row.checked {
                Some(checked) => draw_checkbox(canvas, cx, cy, checked, RED, SLOT_BG),
                None => draw_icon(canvas, &row.icon, cx, cy, SLOT_BG),
            }
        }
    }
    if let Some(watch) = row.watch {
        // Top-right, on its own backing like the badge: it reads over any picture.
        let side = 2.0 * MARK_R + 16.0;
        let (bx, by) = (sx + SLOT_W - 6.0 - side, sy + 6.0);
        canvas.rect(bx, by, side, side, 10.0, SLOT_BG);
        draw_watch(canvas, watch, bx + side / 2.0, by + side / 2.0);
    }
}

/// Renders the panel; `pointer` highlights what it hovers and, with
/// `draw_cursor`, marks its position (previews; the headset has a cursor layer).
pub fn render(
    view: &View,
    fonts: &mut Fonts,
    pointer: Option<(f32, f32)>,
    draw_cursor: bool,
) -> Canvas {
    render_into(
        Canvas::new(WIDTH, HEIGHT),
        view,
        fonts,
        pointer,
        draw_cursor,
    )
}

/// [`render`] over a canvas from an earlier render (any content), which saves
/// allocating and faulting in 6 MB for every repaint.
pub fn render_into(
    mut canvas: Canvas,
    view: &View,
    fonts: &mut Fonts,
    pointer: Option<(f32, f32)>,
    draw_cursor: bool,
) -> Canvas {
    debug_assert_eq!((canvas.width, canvas.height), (WIDTH, HEIGHT));
    canvas.clear(BG);
    let w = WIDTH as f32;
    let crumbs = crumb_spans(view, fonts);
    let hover = pointer.map(|(x, y)| hit(view, fonts, x, y));
    let bottom = HEIGHT as f32 - BOTTOM;

    if let Some(f) = &view.form {
        let form_hover = match hover {
            Some(Hit::Form(h)) => h,
            _ => form::Hit::Nothing,
        };
        fonts.draw(&mut canvas, &f.title, PAD, 78.0, 46.0, TEXT, w - 2.0 * PAD);
        form::render(&mut canvas, fonts, f, form_hover);
    } else if let Some(status) = &view.status {
        let lines = fonts.wrap(status, 36.0, w - 4.0 * PAD);
        for (i, line) in lines.iter().enumerate() {
            fonts.draw(
                &mut canvas,
                line,
                2.0 * PAD,
                HEADER + 90.0 + i as f32 * 52.0,
                36.0,
                SUBTLE,
                w - 4.0 * PAD,
            );
        }
    } else {
        let first = view.scroll.floor() as usize;
        for (i, row) in view.rows.iter().enumerate().skip(first) {
            let (rx, ry, rw, rh) = row_rect(view, i);
            if ry > bottom {
                break;
            }
            // A dimmed row opens nothing, so it only lights up while selecting.
            let hovered = matches!(hover, Some(Hit::Row(r)) if r == i)
                && (!row.dimmed || row.checked.is_some());
            let label_color = if row.dimmed { FAINT } else { TEXT };
            let detail_color = if row.dimmed { FAINT } else { SUBTLE };
            if row.outlined {
                canvas.rect(rx - 3.0, ry - 3.0, rw + 6.0, rh + 6.0, 17.0, ACCENT);
            }
            let bg = if hovered { HOVER } else { ROW_BG };
            canvas.rect(rx, ry, rw, rh, 14.0, bg);
            let cy = ry + rh / 2.0;
            let text_x = if view.thumbnails {
                let slot = (rx + 4.0, ry + (rh - SLOT_H) / 2.0);
                draw_slot(&mut canvas, row, slot, cy, bg);
                slot.0 + SLOT_W + 24.0
            } else {
                let icon_x = PAD + 48.0;
                match row.checked {
                    Some(checked) => draw_checkbox(&mut canvas, icon_x, cy, checked, RED, ROW_BG),
                    None => draw_icon(&mut canvas, &row.icon, icon_x, cy, bg),
                }
                PAD + 96.0
            };
            let mut right_edge = rx + rw - 24.0;
            for (k, action) in row.actions.iter().enumerate() {
                let (ax, ay, aw, ah) = action_rect(view, i, k);
                let color = if *action == Action::Delete {
                    RED
                } else {
                    ACCENT
                };
                let hovered = hover == Some(Hit::RowAction(i, *action));
                let fill = if hovered {
                    color
                } else {
                    [color[0] / 3, color[1] / 3, color[2] / 3]
                };
                canvas.rect(ax, ay, aw, ah, 12.0, fill);
                let label = action.label();
                let lw = fonts.measure(label, 30.0);
                let lx = ax + (aw - lw) / 2.0;
                fonts.draw(&mut canvas, label, lx, ay + ah / 2.0 + 11.0, 30.0, TEXT, aw);
                if k == 0 {
                    right_edge = ax - 16.0;
                }
            }
            // The watched mark takes the right end; the size text moves left of it.
            let mark = row
                .watch
                .filter(|_| row.actions.is_empty() && !view.thumbnails);
            if let Some(watch) = mark {
                draw_watch(&mut canvas, watch, right_edge - MARK_R, cy);
            }
            let progress_edge = right_edge;
            let right_edge = if mark.is_some() {
                right_edge - MARK_W
            } else {
                right_edge
            };
            let right_w = if row.right.is_empty() || !row.actions.is_empty() {
                0.0
            } else {
                fonts.measure(&row.right, 28.0) + 24.0
            };
            let text_w = right_edge - right_w - text_x;
            if row.detail.is_empty() {
                fonts.draw(
                    &mut canvas,
                    &row.label,
                    text_x,
                    cy + 12.0,
                    36.0,
                    label_color,
                    text_w,
                );
            } else {
                fonts.draw(
                    &mut canvas,
                    &row.label,
                    text_x,
                    cy - 2.0,
                    34.0,
                    label_color,
                    text_w,
                );
                fonts.draw(
                    &mut canvas,
                    &row.detail,
                    text_x,
                    cy + 30.0,
                    24.0,
                    detail_color,
                    text_w,
                );
            }
            if right_w > 0.0 {
                fonts.draw(
                    &mut canvas,
                    &row.right,
                    right_edge - right_w + 12.0,
                    cy + 12.0,
                    28.0,
                    detail_color,
                    right_w,
                );
            }
            // Where a video was left: a line along the row's foot.
            if let Some(progress) = row.progress {
                let (px, pw) = (PAD + 96.0, progress_edge - (PAD + 96.0));
                canvas.rect(px, ry + rh - 5.0, pw, 3.0, 1.5, [0x3a, 0x42, 0x50]);
                canvas.rect(
                    px,
                    ry + rh - 5.0,
                    pw * progress.clamp(0.0, 1.0),
                    3.0,
                    1.5,
                    ACCENT,
                );
            }
        }
        // Cover rows that scrolled under the header or off the bottom.
        canvas.rect(0.0, 0.0, w, HEADER - 8.0, 0.0, BG);
        canvas.rect(0.0, bottom, w, BOTTOM, 0.0, BG);
        let max = view.max_scroll();
        if max > 0.0 {
            let (track_top, track) = scroll_track();
            let thumb = thumb_height(view);
            let top = track_top + view.scroll / max * (track - thumb);
            let hovered = hover == Some(Hit::ScrollBar);
            let (bar_w, color) = if hovered {
                (18.0, TEXT)
            } else {
                (10.0, SUBTLE)
            };
            let x = w - PAD - SCROLL_W / 2.0 + 4.0 - bar_w / 2.0;
            canvas.rect(x, track_top, bar_w, track, bar_w / 2.0, [0x2a, 0x2f, 0x38]);
            canvas.rect(x, top, bar_w, thumb, bar_w / 2.0, color);
        }
    }

    // Header: breadcrumbs (links except the last) and the build number.
    if view.form.is_none() {
        for (k, tool) in view.tools.iter().enumerate() {
            let (x, y, tw, th) = tool_rect(view, k);
            let hovered = hover == Some(Hit::Tool(k));
            let fill = match (tool.danger, hovered) {
                (true, true) => [0xff, 0x6b, 0x60],
                (true, false) => RED,
                (false, true) => HOVER,
                (false, false) => [0x2a, 0x2f, 0x38],
            };
            canvas.rect(x, y, tw, th, 14.0, fill);
            let (cx, cy) = (x + tw / 2.0, y + th / 2.0);
            match tool.icon {
                Some(ToolIcon::Thumbnails) => {
                    // A picture: frame, sun and a mountain.
                    let ink = if tool.active {
                        fill
                    } else {
                        [0x2a, 0x2f, 0x38]
                    };
                    canvas.rect(cx - 22.0, cy - 17.0, 44.0, 34.0, 6.0, TEXT);
                    canvas.rect(cx - 18.0, cy - 13.0, 36.0, 26.0, 3.0, ink);
                    canvas.circle(cx + 8.0, cy - 5.0, 4.0, TEXT);
                    for i in 0..12 {
                        let t = i as f32;
                        canvas.rect(
                            cx - 17.0 + t * 1.2,
                            cy + 13.0 - 3.0 - t,
                            2.4,
                            3.0 + t,
                            0.0,
                            TEXT,
                        );
                    }
                    for i in 0..8 {
                        let t = i as f32;
                        canvas.rect(
                            cx - 3.0 + t * 1.5,
                            cy + 13.0 - 3.0 - t * 0.9,
                            1.6,
                            3.0 + t * 0.9,
                            0.0,
                            TEXT,
                        );
                    }
                }
                None => {
                    let lw = fonts.measure(&tool.label, 30.0);
                    fonts.draw(
                        &mut canvas,
                        &tool.label,
                        cx - lw / 2.0,
                        cy + 11.0,
                        30.0,
                        TEXT,
                        tw,
                    );
                }
            }
        }
        let last = view.crumbs.len().saturating_sub(1);
        if let Some(heading) = &view.heading {
            // The selection's tools need the header: the crumbs make way.
            let room = path_room(fonts);
            fonts.draw(&mut canvas, heading, PAD, 78.0, CRUMB_SIZE, TEXT, room);
        } else {
            for span in &crumbs {
                let (x0, x1) = (span.x0, span.x1);
                let hovered = hover == Some(Hit::Crumb(span.index)) && span.index < last;
                if hovered {
                    canvas.rect(x0 - 10.0, CRUMB_Y - 2.0, x1 - x0 + 20.0, 60.0, 10.0, HOVER);
                }
                let color = if span.index == last || hovered {
                    TEXT
                } else {
                    SUBTLE
                };
                let text = if span.elided {
                    ELLIPSIS
                } else {
                    &view.crumbs[span.index]
                };
                fonts.draw(
                    &mut canvas,
                    text,
                    x0,
                    78.0,
                    CRUMB_SIZE,
                    color,
                    x1 - x0 + 1.0,
                );
                if span.index < last {
                    fonts.draw(&mut canvas, CRUMB_SEP, x1, 78.0, CRUMB_SIZE, FAINT, 200.0);
                }
            }
        }
    }
    // At the right of the path's row, level with it; the tools have their own.
    let build_w = fonts.measure(BUILD, 20.0);
    let build_y = 78.0;
    fonts.draw(
        &mut canvas,
        BUILD,
        w - PAD - build_w,
        build_y,
        20.0,
        FAINT,
        build_w + 2.0,
    );
    if view.form.is_none() {
        canvas.rect(
            PAD,
            HEADER - 8.0,
            w - 2.0 * PAD,
            2.0,
            0.0,
            [0x2a, 0x2f, 0x38],
        );
    }
    if let Some(notice) = &view.notice {
        canvas.rect(0.0, HEIGHT as f32 - 60.0, w, 60.0, 0.0, BG);
        fonts.draw(
            &mut canvas,
            notice,
            PAD,
            HEIGHT as f32 - 22.0,
            26.0,
            SUBTLE,
            w - 2.0 * PAD,
        );
    }

    if let Some(dialog) = &view.dialog {
        // Dim the list behind the dialog.
        for px in canvas.pixels.chunks_exact_mut(4) {
            for c in &mut px[..3] {
                *c /= 3;
            }
        }
        let (dx, dy, dw, dh) = (160.0, 120.0, w - 320.0, HEIGHT as f32 - 240.0);
        canvas.rect(dx, dy, dw, dh, 24.0, [0x22, 0x26, 0x2e]);
        fonts.draw(
            &mut canvas,
            &dialog.title,
            dx + 48.0,
            dy + 84.0,
            42.0,
            TEXT,
            dw - 96.0,
        );
        let mut y = dy + 150.0;
        for paragraph in &dialog.body {
            for line in fonts.wrap(paragraph, 30.0, dw - 96.0) {
                fonts.draw(&mut canvas, &line, dx + 48.0, y, 30.0, SUBTLE, dw - 96.0);
                y += 44.0;
            }
            y += 16.0;
        }
        let rects = dialog_buttons(dialog.buttons.len());
        for (i, (label, rect)) in dialog.buttons.iter().zip(&rects).enumerate() {
            let primary = i + 1 == dialog.buttons.len();
            let hovered = hover == Some(Hit::DialogButton(i));
            let color = match (primary, dialog.danger, hovered) {
                (true, true, true) => [0xff, 0x6b, 0x60],
                (true, true, false) => RED,
                (true, false, true) => [0x6b, 0xa0, 0xff],
                (true, false, false) => ACCENT,
                (false, _, true) => HOVER,
                (false, _, false) => [0x2a, 0x2f, 0x38],
            };
            canvas.rect(rect.0, rect.1, rect.2, rect.3, 16.0, color);
            let label_w = fonts.measure(label, 34.0);
            fonts.draw(
                &mut canvas,
                label,
                rect.0 + (rect.2 - label_w) / 2.0,
                rect.1 + 53.0,
                34.0,
                TEXT,
                rect.2,
            );
        }
    }

    if let Some((x, y)) = pointer.filter(|_| draw_cursor) {
        canvas.circle(x, y, 12.0, [0xff, 0xff, 0xff]);
        canvas.circle(x, y, 8.0, ACCENT);
    }
    canvas
}

/// Human-readable size, e.g. "4.7 GB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> View {
        View {
            crumbs: vec!["Video Box".into(), "PC".into(), "media".into()],
            rows: (0..30)
                .map(|i| Row::new(Icon::Folder, format!("{i}")))
                .collect(),
            ..Default::default()
        }
    }

    /// `cargo test --release -- --ignored --nocapture render_time`: how long
    /// one repaint of a folder of videos with thumbnails takes.
    #[test]
    #[ignore]
    fn render_time() {
        let thumb = Arc::new(Thumb {
            width: 220,
            height: 124,
            rgba: vec![90; 220 * 124 * 4],
        });
        let mut view = View {
            crumbs: vec!["Video Box".into(), "PC".into(), "media".into()],
            rows: (0..30)
                .map(|i| {
                    let mut row =
                        Row::new(Icon::Video(None), format!("Some video file name {i}.mp4"));
                    row.detail = "VR180 3D · 6144x3072 HEVC · 12 GB".into();
                    row.right = "1:23:45".into();
                    row.actions = vec![Action::Edit, Action::Delete];
                    row.thumbnail = Some(thumb.clone());
                    row
                })
                .collect(),
            thumbnails: true,
            ..Default::default()
        };
        let mut fonts = Fonts::load().expect("fonts");
        // As the app does: each repaint reuses the last canvas.
        let mut canvas = render(&view, &mut fonts, Some((400.0, 300.0)), false);
        let started = std::time::Instant::now();
        let n = 50;
        for i in 0..n {
            view.scroll = i as f32 * 0.1;
            canvas = render_into(canvas, &view, &mut fonts, Some((400.0, 300.0)), false);
        }
        println!(
            "browser repaint: {:.2} ms",
            started.elapsed().as_secs_f64() * 1e3 / n as f64
        );
        let started = std::time::Instant::now();
        for _ in 0..n {
            let mut c = Canvas::new(WIDTH, HEIGHT);
            c.clear(BG);
            std::hint::black_box(&c);
        }
        println!(
            "  of which a new cleared canvas: {:.2} ms",
            started.elapsed().as_secs_f64() * 1e3 / n as f64
        );
    }

    #[test]
    fn hit_testing_follows_scroll_and_dialog() {
        let mut view = view();
        let mut fonts = Fonts::load().expect("fonts");
        let spans = crumb_spans(&view, &mut fonts);
        let last = (spans[2].x0 + spans[2].x1) / 2.0;
        assert_eq!(hit(&view, &mut fonts, 400.0, HEADER + 10.0), Hit::Row(0));
        view.scroll = 5.0;
        assert_eq!(
            hit(&view, &mut fonts, 400.0, HEADER + view.row_h() + 10.0),
            Hit::Row(6)
        );
        assert_eq!(hit(&view, &mut fonts, 100.0, 70.0), Hit::Crumb(0));
        assert_eq!(
            hit(&view, &mut fonts, last, 70.0),
            Hit::Nothing,
            "current place is not a link"
        );
        view.dialog = Some(Dialog {
            title: String::new(),
            body: vec![],
            buttons: vec!["Cancel".into(), "Delete".into()],
            danger: true,
        });
        let rects = dialog_buttons(2);
        let (x, y, w, h) = rects[1];
        assert_eq!(
            hit(&view, &mut fonts, x + w / 2.0, y + h / 2.0),
            Hit::DialogButton(1)
        );
        assert_eq!(hit(&view, &mut fonts, 400.0, HEADER + 10.0), Hit::Nothing);
    }

    #[test]
    fn icon_cut_outs_show_the_row_behind() {
        let view = View {
            rows: vec![Row::new(Icon::Settings, "Settings")],
            ..Default::default()
        };
        let mut fonts = Fonts::load().expect("fonts");
        let (_, ry, _, _) = row_rect(&view, 0);
        let (cx, cy) = (PAD + 48.0, ry + ROW / 2.0 - 4.0);
        let centre = |canvas: &Canvas| {
            let i = ((cy as u32 * canvas.width + cx as u32) * 4) as usize;
            [canvas.pixels[i], canvas.pixels[i + 1], canvas.pixels[i + 2]]
        };
        assert_eq!(centre(&render(&view, &mut fonts, None, false)), ROW_BG);
        let hovered = render(&view, &mut fonts, Some((400.0, cy)), false);
        assert_eq!(centre(&hovered), HOVER, "the gear's hole");
    }

    #[test]
    fn scrollbar_is_not_a_row() {
        let view = view();
        let x = WIDTH as f32 - PAD - SCROLL_W / 2.0;
        let mut fonts = Fonts::load().expect("fonts");
        assert_eq!(hit(&view, &mut fonts, x, HEADER + 10.0), Hit::ScrollBar);
        assert_eq!(scroll_at(&view, 0.0), 0.0);
        assert_eq!(scroll_at(&view, HEIGHT as f32), view.max_scroll());
        let short = View {
            rows: view.rows[..3].to_vec(),
            ..view.clone()
        };
        assert_eq!(hit(&short, &mut fonts, x, HEADER + 10.0), Hit::Nothing);
    }

    #[test]
    fn a_server_row_draws_and_hits_its_settings_button() {
        let mut fonts = Fonts::load().expect("fonts");
        let mut view = View {
            crumbs: vec!["Video Box".into()],
            rows: vec![
                Row {
                    detail: "smb://user@nas".into(),
                    actions: vec![Action::Edit],
                    ..Row::new(Icon::Server, "NAS")
                },
                Row::new(Icon::Add, "Add server"),
            ],
            ..Default::default()
        };
        let (ax, ay, aw, ah) = action_rect(&view, 0, 0);
        assert!(aw >= 120.0 && ah >= 60.0, "a VR-sized target: {aw}x{ah}");
        let (rx, _, rw, _) = row_rect(&view, 0);
        assert!(ax + aw <= rx + rw, "inside the row");
        let (cx, cy) = (ax + aw / 2.0, ay + ah / 2.0);
        assert_eq!(
            hit(&view, &mut fonts, cx, cy),
            Hit::RowAction(0, Action::Edit)
        );
        assert_eq!(hit(&view, &mut fonts, rx + 40.0, cy), Hit::Row(0));
        // The button is painted: its corner differs from the bare row.
        let canvas = render(&view, &mut fonts, None, false);
        let px = |c: &Canvas, x: f32, y: f32| {
            let i = ((y as u32 * c.width + x as u32) * 4) as usize;
            [c.pixels[i], c.pixels[i + 1], c.pixels[i + 2]]
        };
        let (bx, by) = (ax + 20.0, ay + 8.0);
        view.rows[0].actions.clear();
        let bare = render(&view, &mut fonts, None, false);
        assert_ne!(px(&canvas, bx, by), px(&bare, bx, by));
    }

    #[test]
    fn row_actions_and_tools() {
        let mut view = view();
        let mut fonts = Fonts::load().expect("fonts");
        view.rows[2].actions = vec![Action::Edit, Action::Delete];
        for (k, action) in [Action::Edit, Action::Delete].into_iter().enumerate() {
            let (ax, ay, aw, ah) = action_rect(&view, 2, k);
            assert_eq!(
                hit(&view, &mut fonts, ax + aw / 2.0, ay + ah / 2.0),
                Hit::RowAction(2, action)
            );
        }
        let (ax, ay, aw, ah) = action_rect(&view, 2, 0);
        assert_eq!(
            hit(
                &view,
                &mut fonts,
                ax + aw / 2.0,
                ay + ah / 2.0 + view.row_h()
            ),
            Hit::Row(3),
            "no actions on row 3"
        );
        view.tools = vec![
            Tool::text("Cancel", false),
            Tool::text("Delete 2", true),
            Tool::text("Move 2", false),
        ];
        for k in 0..3 {
            let (tx, ty, tw, th) = tool_rect(&view, k);
            assert_eq!(
                hit(&view, &mut fonts, tx + tw / 2.0, ty + th / 2.0),
                Hit::Tool(k)
            );
        }
        let (tx, _, tw, _) = tool_rect(&view, 2);
        assert!(tx + tw <= WIDTH as f32 - PAD + 0.5);
        view.heading = Some("2 selected".into());
        assert_eq!(
            hit(&view, &mut fonts, 100.0, 70.0),
            Hit::Nothing,
            "no crumbs under a heading"
        );
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(5_097_390_883), "5.1 GB");
    }

    fn tall() -> View {
        View {
            thumbnails: true,
            ..view()
        }
    }

    fn pattern() -> Arc<Thumb> {
        let (w, h) = (SLOT_W as u32, SLOT_H as u32);
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[200, (x % 256) as u8, (y * 2) as u8, 255]);
            }
        }
        Arc::new(Thumb {
            width: w,
            height: h,
            rgba,
        })
    }

    fn pixel(canvas: &Canvas, x: f32, y: f32) -> Rgb {
        let i = ((y as u32 * canvas.width + x as u32) * 4) as usize;
        [canvas.pixels[i], canvas.pixels[i + 1], canvas.pixels[i + 2]]
    }

    #[test]
    fn row_geometry_follows_the_row_height() {
        let (plain, tall) = (view(), tall());
        assert_eq!((plain.row_h(), tall.row_h()), (ROW, ROW_THUMB));
        assert!((plain.visible_rows() - 856.0 / 88.0).abs() < 1e-3);
        assert!((tall.visible_rows() - 856.0 / 140.0).abs() < 1e-3);
        assert!((tall.max_scroll() - (30.0 - tall.visible_rows())).abs() < 1e-3);
        assert_eq!(rows_for_drag(&plain, 176.0), 2.0);
        assert_eq!(rows_for_drag(&tall, 280.0), 2.0);
        let mut fonts = Fonts::load().expect("fonts");
        for mut v in [plain, tall] {
            v.scroll = 4.5;
            let h = v.row_h();
            // Row 4 shows its lower half at the top; row 5 follows it.
            assert_eq!(hit(&v, &mut fonts, 400.0, HEADER + 1.0), Hit::Row(4));
            assert_eq!(
                hit(&v, &mut fonts, 400.0, HEADER + h / 2.0 + 1.0),
                Hit::Row(5)
            );
            let (_, y, _, rh) = row_rect(&v, 6);
            assert_eq!(y, HEADER + 1.5 * h + 4.0);
            assert_eq!(rh, h - 8.0);
            assert_eq!(hit(&v, &mut fonts, 400.0, y + rh / 2.0), Hit::Row(6));
            let thumb = thumb_height(&v);
            assert!(thumb >= 60.0 && thumb < scroll_track().1);
            assert_eq!(scroll_at(&v, HEIGHT as f32), v.max_scroll());
        }
    }

    #[test]
    fn a_thumbnail_fills_the_slot_with_the_mark_in_the_corner() {
        let mut fonts = Fonts::load().expect("fonts");
        let mut view = View {
            rows: vec![
                Row {
                    thumbnail: Some(pattern()),
                    ..Row::new(Icon::Video(Some(Verdict::Hardware)), "a.mp4")
                },
                Row::new(Icon::Video(Some(Verdict::Hardware)), "b.mp4"),
                Row::new(Icon::Folder, "dir"),
            ],
            ..tall()
        };
        let canvas = render(&view, &mut fonts, None, false);
        let (rx, ry, _, rh) = row_rect(&view, 0);
        let (sx, sy) = (rx + 4.0, ry + (rh - SLOT_H) / 2.0);
        // Interior: the thumbnail's own pixels, copied straight.
        assert_eq!(pixel(&canvas, sx + 150.0, sy + 20.0), [200, 150, 40]);
        // Rounded: the corner pixel shows the row behind, the next one in does not.
        assert_eq!(pixel(&canvas, sx, sy), ROW_BG);
        assert_eq!(pixel(&canvas, sx + 20.0, sy), [200, 20, 0]);
        // The badge: dark backing with the green mark, bottom left.
        let (bx, by) = (sx + 6.0 + 31.0, sy + SLOT_H - 6.0 - 20.0);
        assert_eq!(pixel(&canvas, bx, by), GREEN);
        assert_eq!(pixel(&canvas, sx + 8.0, by), SLOT_BG);
        // No picture yet: a placeholder with the mark centred; a folder has none.
        let (_, ry1, _, rh1) = row_rect(&view, 1);
        let (cx, cy) = (sx + SLOT_W / 2.0, ry1 + rh1 / 2.0);
        assert_eq!(pixel(&canvas, cx, cy), GREEN);
        assert_eq!(pixel(&canvas, sx + 12.0, ry1 + 20.0), SLOT_BG);
        let (_, ry2, _, rh2) = row_rect(&view, 2);
        assert_eq!(pixel(&canvas, sx + 12.0, ry2 + 12.0), ROW_BG, "no slot");
        assert_ne!(pixel(&canvas, cx, ry2 + rh2 / 2.0), ROW_BG, "its icon");
        // Plain rows are untouched by any of this.
        view.thumbnails = false;
        let plain = render(&view, &mut fonts, None, false);
        let (_, py, _, ph) = row_rect(&view, 0);
        assert_eq!(pixel(&plain, PAD + 48.0, py + ph / 2.0), GREEN);
    }

    /// A view of three videos, one per watched state, and its tools.
    fn watched_view(thumbnails: bool) -> View {
        let states = [Watch::Unwatched, Watch::Partial, Watch::Watched];
        View {
            rows: states
                .iter()
                .map(|&w| Row {
                    watch: Some(w),
                    right: "1.0 GB".into(),
                    ..Row::new(Icon::Video(Some(Verdict::Hardware)), "clip.mp4")
                })
                .collect(),
            thumbnails,
            ..Default::default()
        }
    }

    /// Where row `i`'s watched mark is centred.
    fn mark_centre(view: &View, i: usize) -> (f32, f32) {
        let (rx, ry, rw, rh) = row_rect(view, i);
        if view.thumbnails {
            let (sx, sy) = (rx + 4.0, ry + (rh - SLOT_H) / 2.0);
            let side = 2.0 * MARK_R + 16.0;
            (sx + SLOT_W - 6.0 - side / 2.0, sy + 6.0 + side / 2.0)
        } else {
            (rx + rw - 24.0 - MARK_R, ry + rh / 2.0)
        }
    }

    #[test]
    fn watched_states_look_different_in_lists_and_thumbnails() {
        let mut fonts = Fonts::load().expect("fonts");
        for thumbnails in [false, true] {
            let view = watched_view(thumbnails);
            let canvas = render(&view, &mut fonts, None, false);
            // Just left of the centre, further left, right of it, and the ring at the right.
            let probe = |i: usize| {
                let (cx, cy) = mark_centre(&view, i);
                [
                    pixel(&canvas, cx - 1.0, cy),
                    pixel(&canvas, cx - 3.0, cy),
                    pixel(&canvas, cx + 3.0, cy),
                    pixel(&canvas, cx + MARK_R - 1.5, cy),
                ]
            };
            let hole = if thumbnails { SLOT_BG } else { ROW_BG };
            assert_eq!(
                probe(0),
                [ACCENT, ACCENT, ACCENT, ACCENT],
                "unwatched is filled (thumbnails: {thumbnails})"
            );
            assert_eq!(
                probe(1),
                [ACCENT, ACCENT, hole, ACCENT],
                "partial is filled on the left only (thumbnails: {thumbnails})"
            );
            assert_eq!(
                probe(2),
                [hole, hole, hole, ACCENT],
                "watched is an empty ring (thumbnails: {thumbnails})"
            );
        }
    }

    #[test]
    fn no_state_no_mark_and_the_size_text_stays_clear_of_it() {
        let mut fonts = Fonts::load().expect("fonts");
        let mut view = watched_view(false);
        view.rows[0].watch = None;
        let with_mark = render(&view, &mut fonts, None, false);
        let (cx, cy) = mark_centre(&view, 0);
        assert_eq!(
            pixel(&with_mark, cx, cy),
            ROW_BG,
            "a folder-like row has none"
        );
        // The size is drawn left of the mark, with a gap of background.
        view.rows[1].watch = Some(Watch::Watched);
        let canvas = render(&view, &mut fonts, None, false);
        let (cx, cy) = mark_centre(&view, 1);
        let mut ink = 0;
        for x in (cx - 90.0) as u32..(cx - MARK_R - 3.0) as u32 {
            for y in (cy - 14.0) as u32..(cy + 14.0) as u32 {
                if pixel(&canvas, x as f32, y as f32) != ROW_BG {
                    ink += 1;
                    assert!(
                        (x as f32) < cx - MARK_R - 14.0,
                        "text reaches the mark at ({x}, {y})"
                    );
                }
            }
        }
        assert!(ink > 0, "the size is still drawn");
    }

    #[test]
    fn sort_labels_fit_the_tool_button() {
        let mut fonts = Fonts::load().expect("fonts");
        let mut mode = crate::config::SortMode::Name;
        for _ in 0..3 {
            let label = format!("Sort: {}", mode.label());
            let w = fonts.measure(&label, 30.0);
            assert!(w < TOOL_W - 24.0, "{label}: {w}");
            mode = mode.next();
        }
    }

    fn long_path_view() -> View {
        View {
            crumbs: [
                "Video Box",
                "smb://someone@a-very-long-server-name.example",
                "Shared Media Archive",
                "Holiday Recordings From Several Years",
                "Summer Trip To The Mountains Part Two",
                "Raw Footage Straight From The Camera Card",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            tools: vec![
                Tool::icon(ToolIcon::Thumbnails, true),
                Tool::text("Sort: Unwatched", false),
                Tool::text("Autoplay", false),
            ],
            rows: (0..5)
                .map(|i| Row::new(Icon::Folder, format!("{i}")))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_long_path_elides_old_crumbs_and_never_reaches_the_build_label() {
        let mut fonts = Fonts::load().expect("fonts");
        let view = long_path_view();
        let spans = crumb_spans(&view, &mut fonts);
        let n = view.crumbs.len();
        assert_eq!(spans.first().map(|s| (s.index, s.elided)), Some((0, false)));
        assert_eq!(
            spans.last().map(|s| (s.index, s.elided)),
            Some((n - 1, false))
        );
        let dots: Vec<_> = spans.iter().filter(|s| s.elided).collect();
        assert_eq!(dots.len(), 1, "one ellipsis: {spans:?}");
        // It stands for the crumbs dropped before the first one shown after it.
        let after = spans.iter().position(|s| s.elided).unwrap() + 1;
        assert_eq!(dots[0].index, spans[after].index - 1);
        assert!(dots[0].index >= 1);
        let limit = WIDTH as f32 - PAD - fonts.measure(BUILD, 20.0) - BUILD_GAP + 0.5;
        for w in spans.windows(2) {
            assert!(w[0].x1 < w[1].x0, "overlap: {spans:?}");
        }
        assert!(spans.last().unwrap().x1 <= limit, "{spans:?} past {limit}");
        // Clear of every tool, and on a different row from them.
        for k in 0..view.tools.len() {
            let (tx, ty, tw, th) = tool_rect(&view, k);
            assert!(ty > CRUMB_Y + CRUMB_H, "tools sit below the path");
            assert!(tx + tw <= WIDTH as f32 - PAD + 0.5 && th > 0.0);
        }
        // What is drawn is what is hit: shown links navigate, the last does not.
        for s in &spans {
            let (x, y) = ((s.x0 + s.x1) / 2.0, CRUMB_Y + 20.0);
            let want = if s.index == n - 1 {
                Hit::Nothing
            } else {
                Hit::Crumb(s.index)
            };
            assert_eq!(hit(&view, &mut fonts, x, y), want, "{s:?}");
        }
        // The tools are reachable and the list starts below them.
        for k in 0..view.tools.len() {
            let (tx, ty, tw, th) = tool_rect(&view, k);
            assert_eq!(
                hit(&view, &mut fonts, tx + tw / 2.0, ty + th / 2.0),
                Hit::Tool(k)
            );
            assert!(ty + th < HEADER - 8.0);
        }
        // The D-pad's targets are the same ones, none under the build label.
        let targets = fixed_targets(&view, &mut fonts);
        assert_eq!(targets.len(), spans.len() - 1 + view.tools.len());
        for (h, (x, y, w, _)) in &targets {
            if matches!(h, Hit::Crumb(_)) {
                assert!(*y < TOOL_Y && x + w <= limit + 8.0 + 0.5);
            }
        }
    }

    #[test]
    fn a_short_path_is_left_alone() {
        let mut fonts = Fonts::load().expect("fonts");
        let view = view();
        let spans = crumb_spans(&view, &mut fonts);
        assert_eq!(
            spans
                .iter()
                .map(|s| (s.index, s.elided))
                .collect::<Vec<_>>(),
            [(0, false), (1, false), (2, false)]
        );
    }

    #[test]
    fn one_enormous_crumb_is_clipped_to_the_row() {
        let mut fonts = Fonts::load().expect("fonts");
        let view = View {
            crumbs: vec!["Video Box".into(), "x".repeat(300)],
            ..Default::default()
        };
        let spans = crumb_spans(&view, &mut fonts);
        assert_eq!(spans.len(), 2);
        let limit = WIDTH as f32 - PAD - fonts.measure(BUILD, 20.0) - BUILD_GAP + 0.5;
        assert!(spans[1].x1 <= limit);
        // Drawing it must not panic or leave the canvas.
        render(&view, &mut fonts, None, false);
    }

    #[test]
    fn full_screens_of_thumbnails_redraw_quickly() {
        let mut fonts = Fonts::load().expect("fonts");
        let rows = |thumb: bool| -> Vec<Row> {
            (0..30)
                .map(|i| Row {
                    detail: "VR180 3D  ·  Plays with hardware decoding".into(),
                    right: "3.3 GB".into(),
                    thumbnail: thumb.then(pattern),
                    ..Row::new(
                        Icon::VideoVr(Some(Verdict::Hardware)),
                        format!("clip {i}.mp4"),
                    )
                })
                .collect()
        };
        let time = |view: &View, fonts: &mut Fonts| {
            render(view, fonts, None, false);
            let start = std::time::Instant::now();
            for _ in 0..3 {
                std::hint::black_box(render(view, fonts, None, false));
            }
            start.elapsed().as_secs_f64() * 1e3 / 3.0
        };
        let plain = View {
            rows: rows(false),
            ..Default::default()
        };
        let empty = View {
            rows: rows(false),
            thumbnails: true,
            ..Default::default()
        };
        let full = View {
            rows: rows(true),
            thumbnails: true,
            ..Default::default()
        };
        let (a, b, c) = (
            time(&plain, &mut fonts),
            time(&empty, &mut fonts),
            time(&full, &mut fonts),
        );
        let mut canvas = Canvas::new(WIDTH, HEIGHT);
        let image = pattern();
        let start = std::time::Instant::now();
        for _ in 0..200 {
            canvas.image(36, 130, &image, SLOT_R);
        }
        let blit = start.elapsed().as_secs_f64() * 1e3 / 200.0;
        eprintln!(
            "redraw: {a:.1} ms plain, {b:.1} ms tall with placeholders, {c:.1} ms with 7 thumbnails; one thumbnail blit {blit:.3} ms"
        );
        // Only a sanity bound (debug builds, shared machines): blitting is cheap.
        assert!(blit < 1.0, "{blit} ms per thumbnail");
        assert!(c < a * 3.0 + 20.0, "{c} ms vs {a} ms");
    }
}
