//! Tiling layout: a binary split tree of tiles, each showing one panel.
//!
//! Geometry is computed in a unit square so directional focus and moves are
//! independent of window size.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Panel {
    Browser,
    ChannelRack,
    PianoRoll,
    Playlist,
    Mixer,
    Automation,
    Parameters,
    Settings,
}

impl Panel {
    pub const ALL: [Panel; 8] = [
        Panel::Playlist,
        Panel::ChannelRack,
        Panel::PianoRoll,
        Panel::Mixer,
        Panel::Automation,
        Panel::Browser,
        Panel::Parameters,
        Panel::Settings,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Panel::Browser => "browser",
            Panel::ChannelRack => "channel rack",
            Panel::PianoRoll => "piano roll",
            Panel::Playlist => "playlist",
            Panel::Mixer => "mixer",
            Panel::Automation => "automation",
            Panel::Parameters => "parameters",
            Panel::Settings => "settings",
        }
    }
}

impl std::fmt::Display for Panel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TileId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Axis {
    /// Children side by side, split line is vertical.
    Horizontal,
    /// Children stacked, split line is horizontal.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    fn axis(self) -> Axis {
        match self {
            Direction::Left | Direction::Right => Axis::Horizontal,
            Direction::Up | Direction::Down => Axis::Vertical,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(TileId),
    Split { axis: Axis, ratio: f32, first: Box<Node>, second: Box<Node> },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const UNIT: Rect = Rect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

    pub fn split(self, axis: Axis, ratio: f32) -> (Rect, Rect) {
        match axis {
            Axis::Horizontal => {
                let w = self.width * ratio;
                (Rect { width: w, ..self }, Rect { x: self.x + w, width: self.width - w, ..self })
            }
            Axis::Vertical => {
                let h = self.height * ratio;
                (Rect { height: h, ..self }, Rect { y: self.y + h, height: self.height - h, ..self })
            }
        }
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// A split node's geometry: its path from the root, the area it divides, and the gap between its children.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub path: Vec<bool>,
    pub axis: Axis,
    pub area: Rect,
    pub line: Rect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    pub id: TileId,
    pub panel: Panel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub root: Node,
    pub tiles: Vec<Tile>,
    pub focused: TileId,
    /// Focus mode: the focused tile fills the window. The tree is untouched.
    pub zoomed: bool,
    next_id: u32,
}

const MIN_RATIO: f32 = 0.05;

impl Layout {
    pub fn single(panel: Panel) -> Self {
        let id = TileId(0);
        Self { root: Node::Leaf(id), tiles: vec![Tile { id, panel }], focused: id, zoomed: false, next_id: 1 }
    }

    /// Playlist on top, channel rack and piano roll below it, mixer at the bottom,
    /// browser at the left.
    pub fn default_workspace() -> Self {
        let mut layout = Layout::single(Panel::Playlist);
        let playlist = layout.focused;
        layout.split(Axis::Vertical, Panel::ChannelRack);
        layout.split(Axis::Horizontal, Panel::PianoRoll);
        layout.set_ratio_of(layout.focused, 0.6);
        layout.focused = playlist;
        layout.split(Axis::Horizontal, Panel::Automation);
        layout.set_ratio_of(layout.focused, 0.7);
        layout.focused = playlist;
        layout.root = Node::Split {
            axis: Axis::Horizontal,
            ratio: 0.16,
            first: Box::new(Node::Leaf(layout.add_tile(Panel::Browser))),
            second: Box::new(layout.root.clone()),
        };
        layout
    }

    pub fn panel(&self, id: TileId) -> Panel {
        self.tile(id).panel
    }

    fn tile(&self, id: TileId) -> &Tile {
        self.tiles.iter().find(|t| t.id == id).expect("tile ids in the tree exist in the tile list")
    }

    pub fn set_panel(&mut self, id: TileId, panel: Panel) {
        if let Some(tile) = self.tiles.iter_mut().find(|t| t.id == id) {
            tile.panel = panel;
        }
    }

    pub fn find_panel(&self, panel: Panel) -> Option<TileId> {
        self.leaves().into_iter().find(|&id| self.panel(id) == panel)
    }

    fn add_tile(&mut self, panel: Panel) -> TileId {
        let id = TileId(self.next_id);
        self.next_id += 1;
        self.tiles.push(Tile { id, panel });
        id
    }

    pub fn leaves(&self) -> Vec<TileId> {
        let mut out = Vec::new();
        collect_leaves(&self.root, &mut out);
        out
    }

    /// Tile rectangles within `area`. In focus mode only the focused tile is returned.
    pub fn rects(&self, area: Rect) -> Vec<(TileId, Rect)> {
        self.rects_with_gap(area, 0.0)
    }

    /// Pixel geometry when split handles reserve space between the children.
    pub fn rects_with_gap(&self, area: Rect, gap: f32) -> Vec<(TileId, Rect)> {
        if self.zoomed {
            return vec![(self.focused, area)];
        }
        let mut out = Vec::new();
        collect_rects(&self.root, area, gap, &mut out);
        out
    }

    /// Splits within `area`, for mouse resizing.
    pub fn splits(&self, area: Rect) -> Vec<Split> {
        self.splits_with_gap(area, 0.0)
    }

    pub fn splits_with_gap(&self, area: Rect, gap: f32) -> Vec<Split> {
        let mut out = Vec::new();
        if !self.zoomed {
            collect_splits(&self.root, area, gap, &mut Vec::new(), &mut out);
        }
        out
    }

    /// Split the focused tile, placing a new tile with `panel` after it, and focus it.
    pub fn split(&mut self, axis: Axis, panel: Panel) -> TileId {
        let new = self.add_tile(panel);
        let target = self.focused;
        replace_leaf(&mut self.root, target, &mut |_| Node::Split {
            axis,
            ratio: 0.5,
            first: Box::new(Node::Leaf(target)),
            second: Box::new(Node::Leaf(new)),
        });
        self.focused = new;
        self.zoomed = false;
        new
    }

    /// Close the focused tile; its sibling takes its space. The last tile stays.
    pub fn close(&mut self) -> bool {
        let target = self.focused;
        if matches!(self.root, Node::Leaf(_)) {
            return false;
        }
        let Some(sibling) = remove_leaf(&mut self.root, target) else {
            return false;
        };
        self.tiles.retain(|t| t.id != target);
        self.focused = first_leaf(&sibling);
        self.zoomed = false;
        true
    }

    fn neighbor(&self, direction: Direction) -> Option<TileId> {
        let rects = self.rects_unzoomed();
        let (_, from) = *rects.iter().find(|(id, _)| *id == self.focused)?;
        let eps = 1e-4;
        let (cx, cy) = from.center();
        rects
            .iter()
            .filter(|(id, _)| *id != self.focused)
            .filter(|(_, r)| match direction {
                Direction::Left => (r.x + r.width - from.x).abs() < eps,
                Direction::Right => (from.x + from.width - r.x).abs() < eps,
                Direction::Up => (r.y + r.height - from.y).abs() < eps,
                Direction::Down => (from.y + from.height - r.y).abs() < eps,
            })
            .filter(|(_, r)| match direction.axis() {
                Axis::Horizontal => r.y < from.y + from.height - eps && r.y + r.height > from.y + eps,
                Axis::Vertical => r.x < from.x + from.width - eps && r.x + r.width > from.x + eps,
            })
            .min_by(|(_, a), (_, b)| {
                let distance = |r: &Rect| {
                    let (x, y) = r.center();
                    match direction.axis() {
                        Axis::Horizontal => (y - cy).abs(),
                        Axis::Vertical => (x - cx).abs(),
                    }
                };
                distance(a).total_cmp(&distance(b))
            })
            .map(|(id, _)| *id)
    }

    fn rects_unzoomed(&self) -> Vec<(TileId, Rect)> {
        let mut out = Vec::new();
        collect_rects(&self.root, Rect::UNIT, 0.0, &mut out);
        out
    }

    pub fn focus(&mut self, direction: Direction) -> bool {
        match self.neighbor(direction) {
            Some(id) => {
                self.focused = id;
                true
            }
            None => false,
        }
    }

    /// Swap the focused tile with its neighbor; focus follows the tile.
    pub fn swap(&mut self, direction: Direction) -> bool {
        let Some(other) = self.neighbor(direction) else {
            return false;
        };
        let focused = self.focused;
        swap_leaves(&mut self.root, focused, other);
        true
    }

    /// Grow (positive) or shrink the focused tile along the direction's axis by
    /// moving the nearest split edge on that side.
    pub fn resize(&mut self, direction: Direction, amount: f32) -> bool {
        let Some(path) = path_to(&self.root, self.focused) else {
            return false;
        };
        // Moving the right/bottom edge uses the deepest ancestor where we are
        // the first child; left/top uses one where we are the second child.
        let wants_first = matches!(direction, Direction::Right | Direction::Down);
        let axis = direction.axis();
        for depth in (0..path.len()).rev() {
            let node = node_at_mut(&mut self.root, &path[..depth]);
            if let Node::Split { axis: split_axis, ratio, .. } = node
                && *split_axis == axis
                && path[depth] != wants_first
            {
                let delta = if wants_first { amount } else { -amount };
                *ratio = (*ratio + delta).clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                return true;
            }
        }
        false
    }

    /// Set the ratio of the split directly containing `id`, from the perspective
    /// of `id`'s size.
    fn set_ratio_of(&mut self, id: TileId, share: f32) {
        let Some(path) = path_to(&self.root, id) else { return };
        let Some((&is_second, parent)) = path.split_last() else { return };
        if let Node::Split { ratio, .. } = node_at_mut(&mut self.root, parent) {
            *ratio = if is_second { 1.0 - share } else { share };
        }
    }

    /// Set a split's ratio by path (from `splits`), for mouse dragging.
    pub fn set_split_ratio(&mut self, path: &[bool], value: f32) {
        if let Node::Split { ratio, .. } = node_at_mut(&mut self.root, path) {
            *ratio = value.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
        }
    }

    pub fn toggle_zoom(&mut self) {
        self.zoomed = !self.zoomed;
    }
}

/// Paths use `false` for the first child and `true` for the second.
fn node_at_mut<'a>(mut node: &'a mut Node, path: &[bool]) -> &'a mut Node {
    for &second in path {
        node = match node {
            Node::Split { first, second: s, .. } => {
                if second {
                    s
                } else {
                    first
                }
            }
            Node::Leaf(_) => return node,
        };
    }
    node
}

fn path_to(node: &Node, id: TileId) -> Option<Vec<bool>> {
    match node {
        Node::Leaf(leaf) => (*leaf == id).then(Vec::new),
        Node::Split { first, second, .. } => {
            if let Some(mut path) = path_to(first, id) {
                path.insert(0, false);
                Some(path)
            } else {
                let mut path = path_to(second, id)?;
                path.insert(0, true);
                Some(path)
            }
        }
    }
}

fn collect_leaves(node: &Node, out: &mut Vec<TileId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split { first, second, .. } => {
            collect_leaves(first, out);
            collect_leaves(second, out);
        }
    }
}

fn split_with_gap(area: Rect, axis: Axis, ratio: f32, gap: f32) -> (Rect, Rect) {
    let mut available = area;
    match axis {
        Axis::Horizontal => available.width = (area.width - gap).max(0.0),
        Axis::Vertical => available.height = (area.height - gap).max(0.0),
    }
    // The renderer distributes integer fill portions on a 1000-point scale.
    let portion = (ratio * 1000.0).round().clamp(1.0, 999.0) / 1000.0;
    let (first, mut second) = available.split(axis, portion);
    match axis {
        Axis::Horizontal => second.x += gap,
        Axis::Vertical => second.y += gap,
    }
    (first, second)
}

fn collect_rects(node: &Node, area: Rect, gap: f32, out: &mut Vec<(TileId, Rect)>) {
    match node {
        Node::Leaf(id) => out.push((*id, area)),
        Node::Split { axis, ratio, first, second } => {
            let (a, b) = if gap == 0.0 { area.split(*axis, *ratio) } else { split_with_gap(area, *axis, *ratio, gap) };
            collect_rects(first, a, gap, out);
            collect_rects(second, b, gap, out);
        }
    }
}

fn collect_splits(node: &Node, area: Rect, gap: f32, path: &mut Vec<bool>, out: &mut Vec<Split>) {
    if let Node::Split { axis, ratio, first, second } = node {
        let (a, b) = if gap == 0.0 { area.split(*axis, *ratio) } else { split_with_gap(area, *axis, *ratio, gap) };
        let line = match axis {
            Axis::Horizontal => Rect { x: a.x + a.width, width: b.x - a.x - a.width, ..area },
            Axis::Vertical => Rect { y: a.y + a.height, height: b.y - a.y - a.height, ..area },
        };
        out.push(Split { path: path.clone(), axis: *axis, area, line });
        path.push(false);
        collect_splits(first, a, gap, path, out);
        path.pop();
        path.push(true);
        collect_splits(second, b, gap, path, out);
        path.pop();
    }
}

fn first_leaf(node: &Node) -> TileId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split { first, .. } => first_leaf(first),
    }
}

fn replace_leaf(node: &mut Node, id: TileId, make: &mut dyn FnMut(TileId) -> Node) -> bool {
    match node {
        Node::Leaf(leaf) if *leaf == id => {
            *node = make(id);
            true
        }
        Node::Leaf(_) => false,
        Node::Split { first, second, .. } => replace_leaf(first, id, make) || replace_leaf(second, id, make),
    }
}

/// Remove leaf `id`, collapsing its parent into the sibling. Returns a copy of
/// the sibling subtree that took its place.
fn remove_leaf(node: &mut Node, id: TileId) -> Option<Node> {
    let Node::Split { first, second, .. } = node else {
        return None;
    };
    let sibling = if matches!(**first, Node::Leaf(leaf) if leaf == id) {
        Some((**second).clone())
    } else if matches!(**second, Node::Leaf(leaf) if leaf == id) {
        Some((**first).clone())
    } else {
        None
    };
    if let Some(sibling) = sibling {
        *node = sibling.clone();
        return Some(sibling);
    }
    remove_leaf(first, id).or_else(|| remove_leaf(second, id))
}

fn swap_leaves(node: &mut Node, a: TileId, b: TileId) {
    match node {
        Node::Leaf(id) if *id == a => *id = b,
        Node::Leaf(id) if *id == b => *id = a,
        Node::Leaf(_) => {}
        Node::Split { first, second, .. } => {
            swap_leaves(first, a, b);
            swap_leaves(second, a, b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [ A | B ]
    /// [   C   ]
    fn three() -> (Layout, TileId, TileId, TileId) {
        let mut layout = Layout::single(Panel::Playlist);
        let a = layout.focused;
        let c = layout.split(Axis::Vertical, Panel::Mixer);
        layout.focused = a;
        let b = layout.split(Axis::Horizontal, Panel::PianoRoll);
        (layout, a, b, c)
    }

    #[test]
    fn split_places_new_tile_and_focuses_it() {
        let (layout, a, b, c) = three();
        assert_eq!(layout.leaves(), vec![a, b, c]);
        assert_eq!(layout.focused, b);
        let rects = layout.rects(Rect::UNIT);
        assert_eq!(rects[0].1, Rect { x: 0.0, y: 0.0, width: 0.5, height: 0.5 });
        assert_eq!(rects[2].1, Rect { x: 0.0, y: 0.5, width: 1.0, height: 0.5 });
    }

    #[test]
    fn pixel_geometry_reserves_handles_in_nested_splits() {
        let (layout, a, b, c) = three();
        let area = Rect { x: 20.0, y: 64.0, width: 1000.0, height: 600.0 };
        let rects = layout.rects_with_gap(area, 8.0);
        assert_eq!(rects, vec![
            (a, Rect { x: 20.0, y: 64.0, width: 496.0, height: 296.0 }),
            (b, Rect { x: 524.0, y: 64.0, width: 496.0, height: 296.0 }),
            (c, Rect { x: 20.0, y: 368.0, width: 1000.0, height: 296.0 }),
        ]);
        let splits = layout.splits_with_gap(area, 8.0);
        assert_eq!(splits[1], Split {
            path: vec![false],
            axis: Axis::Horizontal,
            area: Rect { x: 20.0, y: 64.0, width: 1000.0, height: 296.0 },
            line: Rect { x: 516.0, y: 64.0, width: 8.0, height: 296.0 },
        });
    }

    #[test]
    fn directional_focus() {
        let (mut layout, a, b, c) = three();
        assert!(layout.focus(Direction::Left));
        assert_eq!(layout.focused, a);
        assert!(!layout.focus(Direction::Left));
        assert!(layout.focus(Direction::Down));
        assert_eq!(layout.focused, c);
        assert!(layout.focus(Direction::Up));
        assert_eq!(layout.focused, a, "up from full-width tile picks nearest by center; tie goes to first");
        layout.focused = b;
        assert!(layout.focus(Direction::Down));
        assert_eq!(layout.focused, c);
    }

    #[test]
    fn close_collapses_into_sibling() {
        let (mut layout, a, b, c) = three();
        assert!(layout.close());
        assert_eq!(layout.leaves(), vec![a, c]);
        assert_eq!(layout.focused, a);
        assert_eq!(layout.tiles.len(), 2);
        layout.focused = c;
        assert!(layout.close());
        assert!(!layout.close(), "last tile stays");
        assert_eq!(layout.leaves(), vec![a]);
        assert!(layout.tiles.iter().all(|t| t.id != b));
    }

    #[test]
    fn swap_moves_tile_and_keeps_focus_on_it() {
        let (mut layout, a, b, c) = three();
        assert!(layout.swap(Direction::Left));
        assert_eq!(layout.leaves(), vec![b, a, c]);
        assert_eq!(layout.focused, b);
        let rect = layout.rects(Rect::UNIT).into_iter().find(|(id, _)| *id == b).unwrap().1;
        assert_eq!(rect.x, 0.0);
    }

    #[test]
    fn resize_moves_the_right_edge() {
        let (mut layout, a, b, _) = three();
        layout.focused = a;
        assert!(layout.resize(Direction::Right, 0.1));
        let width = |layout: &Layout, id| layout.rects(Rect::UNIT).into_iter().find(|(t, _)| *t == id).unwrap().1.width;
        assert!((width(&layout, a) - 0.6).abs() < 1e-6);
        layout.focused = b;
        assert!(layout.resize(Direction::Left, 0.1), "left edge of b is the same split");
        assert!((width(&layout, b) - 0.5).abs() < 1e-6);
        assert!(!layout.resize(Direction::Right, 0.1), "b has no right edge to move");
        assert!(layout.resize(Direction::Down, 0.1));
    }

    #[test]
    fn focus_mode_round_trip() {
        let (mut layout, _, b, _) = three();
        let before = layout.root.clone();
        layout.toggle_zoom();
        assert_eq!(layout.rects(Rect::UNIT), vec![(b, Rect::UNIT)]);
        layout.toggle_zoom();
        assert_eq!(layout.root, before);
        assert_eq!(layout.rects(Rect::UNIT).len(), 3);
    }
}
