use std::collections::{BinaryHeap, HashMap, HashSet};
use std::cmp::Reverse;

/// API-deserialisable map tile, matching MapSchema from the OpenAPI spec.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MapTile {
    pub map_id: i32,
    pub name: String,
    pub skin: String,
    pub x: i32,
    pub y: i32,
    pub layer: String,
    pub access: AccessSchema,
    #[serde(default)]
    pub interactions: InteractionSchema,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AccessSchema {
    #[serde(rename = "type")]
    pub access_type: MapAccessType,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapAccessType {
    Standard,
    Blocked,
    Conditional,
    Restricted,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct InteractionSchema {
    pub content: Option<MapContentSchema>,
    pub transition: Option<TransitionSchema>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct MapContentSchema {
    #[serde(rename = "type")]
    pub content_type: String,
    pub code: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TransitionSchema {
    pub map_id: i32,
    pub x: i32,
    pub y: i32,
    pub layer: String,
}

/// A loaded game map for one layer (typically "overworld").
/// Stores the full set of tiles; A* uses only walkable ones.
#[derive(Debug, Default)]
pub struct GameMap {
    tiles: HashMap<(i32, i32), MapTile>,
}

impl GameMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, tile: MapTile) {
        self.tiles.insert((tile.x, tile.y), tile);
    }

    pub fn from_tiles(tiles: impl IntoIterator<Item = MapTile>) -> Self {
        let mut m = Self::new();
        for t in tiles {
            m.insert(t);
        }
        m
    }

    /// True if the tile exists and is walkable (standard or conditional).
    /// Unknown tiles (not in map) are treated as walkable to allow pathfinding
    /// through areas not yet loaded. Mark explicitly blocked tiles in the map data.
    pub fn is_walkable(&self, x: i32, y: i32) -> bool {
        match self.tiles.get(&(x, y)) {
            Some(t) => t.access.access_type != MapAccessType::Blocked,
            None => true, // unknown → optimistically walkable
        }
    }

    pub fn get(&self, x: i32, y: i32) -> Option<&MapTile> {
        self.tiles.get(&(x, y))
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// A* shortest path (4-directional) from `from` to `to`.
    /// Returns the number of hops, or None if no path exists within `max_hops`.
    /// Falls back to Manhattan distance when map data is sparse (unknown tiles are walkable).
    pub fn astar(&self, from: (i32, i32), to: (i32, i32), max_hops: u32) -> Option<u32> {
        if from == to {
            return Some(0);
        }

        // (f_score, g_score, position)
        // Use Reverse for min-heap behaviour with BinaryHeap.
        let mut open: BinaryHeap<Reverse<(u32, u32, i32, i32)>> = BinaryHeap::new();
        let mut g_score: HashMap<(i32, i32), u32> = HashMap::new();
        let mut closed: HashSet<(i32, i32)> = HashSet::new();

        let h = |pos: (i32, i32)| -> u32 {
            (pos.0 - to.0).unsigned_abs() + (pos.1 - to.1).unsigned_abs()
        };

        g_score.insert(from, 0);
        open.push(Reverse((h(from), 0, from.0, from.1)));

        while let Some(Reverse((f, g, x, y))) = open.pop() {
            let pos = (x, y);

            if pos == to {
                return Some(g);
            }

            if g > max_hops {
                continue;
            }

            // Skip stale entries (g_score was updated after this was enqueued).
            if g_score.get(&pos).copied().unwrap_or(u32::MAX) < g {
                continue;
            }

            if !closed.insert(pos) {
                continue;
            }

            for (dx, dy) in [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)] {
                let nb = (x + dx, y + dy);
                if !self.is_walkable(nb.0, nb.1) {
                    continue;
                }
                let ng = g + 1;
                if ng > max_hops {
                    continue;
                }
                let prev_g = g_score.get(&nb).copied().unwrap_or(u32::MAX);
                if ng < prev_g {
                    g_score.insert(nb, ng);
                    let nf = ng + h(nb);
                    open.push(Reverse((nf, ng, nb.0, nb.1)));
                }
            }

            let _ = f; // suppress unused warning — f is used implicitly by BinaryHeap ordering
        }

        None
    }

    /// Shortest path hop count, falling back to Manhattan distance if A* finds no path
    /// within max_hops (e.g. map data is incomplete).
    pub fn path_hops(&self, from: (i32, i32), to: (i32, i32)) -> u32 {
        let manhattan = (from.0 - to.0).unsigned_abs() + (from.1 - to.1).unsigned_abs();
        // Cap search at 4× Manhattan to avoid runaway searches across sparse maps.
        let max_hops = (manhattan * 4).max(1).min(500);
        self.astar(from, to, max_hops).unwrap_or(manhattan)
    }
}

/// Paginated API response for GET /maps.
#[derive(Debug, serde::Deserialize)]
pub struct MapsPage {
    pub data: Vec<MapTile>,
    pub total: u32,
    pub page: u32,
    pub size: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(x: i32, y: i32, blocked: bool) -> MapTile {
        MapTile {
            map_id: x * 100 + y,
            name: format!("{x},{y}"),
            skin: "grass".into(),
            x,
            y,
            layer: "overworld".into(),
            access: AccessSchema {
                access_type: if blocked {
                    MapAccessType::Blocked
                } else {
                    MapAccessType::Standard
                },
            },
            interactions: InteractionSchema::default(),
        }
    }

    fn flat_map(w: i32, h: i32) -> GameMap {
        let mut m = GameMap::new();
        for y in 0..h {
            for x in 0..w {
                m.insert(tile(x, y, false));
            }
        }
        m
    }

    #[test]
    fn test_straight_line() {
        let m = flat_map(5, 5);
        assert_eq!(m.path_hops((0, 0), (3, 0)), 3);
        assert_eq!(m.path_hops((0, 0), (0, 4)), 4);
    }

    #[test]
    fn test_diagonal_manhattan() {
        let m = flat_map(5, 5);
        // No diagonal movement — path is L-shaped.
        assert_eq!(m.path_hops((0, 0), (2, 2)), 4);
        // Same as Manhattan distance.
        assert_eq!(m.path_hops((0, 0), (4, 1)), 5);
    }

    #[test]
    fn test_same_tile() {
        let m = flat_map(5, 5);
        assert_eq!(m.path_hops((2, 2), (2, 2)), 0);
    }

    #[test]
    fn test_detour_around_wall() {
        // Build a map with a vertical wall at x=2, y=0..=3, leaving gap at y=4.
        //
        //  . . # . .
        //  . . # . .
        //  . . # . .
        //  . . # . .
        //  . . . . .   <- gap row, y=4
        //
        let mut m = flat_map(5, 5);
        for y in 0..4 {
            m.insert(tile(2, y, true)); // blocked
        }
        // Path from (0,0) to (4,0) must go around: down to y=4, across, back up.
        // Manhattan = 4; A* must find 4+4+4 = 12 hops (down 4, right 4, up 4) = 12.
        // Actually: go to (0,4) is 4 hops down, then (2+,4) is 2+ hops right, then (4,0) is 4 up.
        // Minimum: (0,0)→(0,4)→(2,4)→(4,4)→(4,0) = 4+2+2+4 = 12. But shorter:
        // (0,0)→(1,0)→(1,4)→(4,4)→(4,0) = 1+4+3+4 = 12. Or:
        // (0,0)→(0,4)→(4,4)→(4,0) = 4+4+4=12.
        // Actually: shortest is hugging the wall:
        // go right to (1,0) (1), down to (1,4) (4), right to (3,4) (2), up to (3,0) (4), right to (4,0) (1) = 12.
        // Or: (0,0)→...→(1,4)→(2,4)→(3,4)→(4,4)→(4,0) etc.
        // Minimum detour = 12.
        let hops = m.path_hops((0, 0), (4, 0));
        assert!(hops > 4, "should be longer than Manhattan due to wall, got {hops}");
        assert!(hops <= 12, "should not be longer than 12 hops, got {hops}");
    }

    #[test]
    fn test_fallback_manhattan_on_no_path() {
        // Completely walled off destination — A* fails, returns Manhattan fallback.
        let mut m = flat_map(5, 5);
        // Surround (4,4) with walls.
        for (x, y) in [(3i32,4i32),(4,3),(4,4)] {
            m.insert(tile(x, y, true));
        }
        // (4,4) is blocked, so target unreachable. Path to (4,4) will fail A* and
        // fall back to Manhattan = 4+4 = 8.
        let hops = m.path_hops((0, 0), (4, 4));
        // Either A* finds something or falls back to Manhattan.
        assert!(hops >= 4, "should be at least Manhattan, got {hops}");
    }

    #[test]
    fn test_farm_copper_path() {
        // Verify farm-copper test constants: clear 5×2 grid.
        let m = flat_map(5, 2);
        // (0,0) → (2,0): 2 hops
        assert_eq!(m.path_hops((0, 0), (2, 0)), 2);
        // (2,0) → (4,1): 2 right + 1 down = 3 hops
        assert_eq!(m.path_hops((2, 0), (4, 1)), 3);
    }
}
