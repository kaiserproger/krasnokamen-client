//! Battle take scanner for shot planning: one pass over a .solrec with the
//! replay decoder (packet framing, phases, tick markers, entity and item-stack
//! wire formats) and no session, GPU or terrain. Writes a JSONL timeline, one
//! `sample` row per interval of tick time, then one `entity` row per fighter or
//! horse that saw contact, then a `summary` row. Times are the render CLI's
//! tick seconds: `(tick - first_tick) * 0.05`.
use super::format::{RecordingReader, phase_after, tick_marker};
use crate::protocol::{
    Reader, content::Content, network::Phase, packet_ids::play::clientbound as cb,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    fs::OpenOptions,
    io::{BufWriter, Write},
    path::PathBuf,
};

type Result<T, E = String> = std::result::Result<T, E>;

/// Opposite-side fighters within this horizontal distance are in melee contact.
pub(crate) const CONTACT_REACH: f64 = 2.5;
/// Contact fighters within this distance of each other form one cluster.
const CONTACT_LINK: f64 = 4.0;
const CAVALRY_LINK: f64 = 6.0;
const MAX_CONTACTS: usize = 8;
const IDS_PER_SIDE: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Side {
    Red,
    Blue,
}
impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Red => "red",
            Side::Blue => "blue",
        }
    }
}

/// The staging loadout marks sides with `krasnokamen_battle:red_*`/`blue_*`
/// item models (shields) and equippable assets (surcoats, caparisons).
pub(crate) fn side_of_components(components: &BTreeMap<String, Value>) -> Option<Side> {
    let model = components
        .get("minecraft:item_model")
        .and_then(Value::as_str);
    let asset = components
        .get("minecraft:equippable")
        .and_then(|v| v.get("asset_id"))
        .and_then(Value::as_str);
    [model, asset].into_iter().flatten().find_map(|id| {
        let path = id.strip_prefix("krasnokamen_battle:")?;
        if path.starts_with("red_") {
            Some(Side::Red)
        } else if path.starts_with("blue_") {
            Some(Side::Blue)
        } else {
            None
        }
    })
}

/// Takes from pins before the side components: the two armies spawn on either
/// side of an empty gap. The widest gap in the middle 60% of spawn positions on
/// either horizontal axis splits them; the lower half is side 1 (red), as staging
/// numbers them. None when no gap of at least two blocks exists.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Split {
    pub axis: usize,
    pub threshold: f64,
}
impl Split {
    pub(crate) fn side(&self, spawn: [f64; 2]) -> Side {
        if spawn[self.axis] < self.threshold {
            Side::Red
        } else {
            Side::Blue
        }
    }
}
pub(crate) fn spawn_split(points: &[[f64; 2]]) -> Option<Split> {
    if points.len() < 2 {
        return None;
    }
    let mut best: Option<(f64, Split)> = None;
    for axis in 0..2 {
        let mut values: Vec<f64> = points.iter().map(|p| p[axis]).collect();
        values.sort_by(f64::total_cmp);
        let (low, high) = (
            values.len() / 5,
            (values.len() * 4 / 5).min(values.len() - 1),
        );
        for i in low..high {
            let gap = values[i + 1] - values[i];
            if gap >= 2. && best.is_none_or(|(widest, _)| gap > widest) {
                best = Some((
                    gap,
                    Split {
                        axis,
                        threshold: (values[i] + values[i + 1]) / 2.,
                    },
                ));
            }
        }
    }
    best.map(|(_, split)| split)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Fighter {
    pub id: i32,
    pub position: [f64; 3],
    pub side: Side,
}

#[derive(Debug)]
pub(crate) struct Cluster {
    pub members: Vec<usize>,
    pub red: usize,
    pub blue: usize,
    pub centroid: [f64; 3],
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// Unit vector from the red centroid towards the blue centroid (xz).
    pub normal: [f64; 2],
    /// Unit vector along the contact line, perpendicular to `normal`.
    pub line: [f64; 2],
    pub length: f64,
    pub depth: f64,
}

fn grid(points: impl Iterator<Item = [f64; 3]>, cell: f64) -> HashMap<(i64, i64), Vec<usize>> {
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (index, p) in points.enumerate() {
        grid.entry(((p[0] / cell).floor() as i64, (p[2] / cell).floor() as i64))
            .or_default()
            .push(index);
    }
    grid
}
fn near(
    grid: &HashMap<(i64, i64), Vec<usize>>,
    p: [f64; 3],
    cell: f64,
) -> impl Iterator<Item = usize> + '_ {
    let (cx, cz) = ((p[0] / cell).floor() as i64, (p[2] / cell).floor() as i64);
    (-1..=1)
        .flat_map(move |dx| (-1..=1).map(move |dz| (cx + dx, cz + dz)))
        .filter_map(|key| grid.get(&key))
        .flatten()
        .copied()
}
fn close(a: [f64; 3], b: [f64; 3], reach: f64) -> bool {
    (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2) <= reach * reach && (a[1] - b[1]).abs() <= 2.
}
/// Single-linkage groups of points within `link` blocks (horizontal, |dy| <= 2).
pub(crate) fn groups(points: &[[f64; 3]], link: f64) -> Vec<Vec<usize>> {
    let cells = grid(points.iter().copied(), link);
    let mut parent: Vec<usize> = (0..points.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for (i, &p) in points.iter().enumerate() {
        for j in near(&cells, p, link) {
            if j > i && close(p, points[j], link) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                if a != b {
                    parent[a.max(b)] = a.min(b);
                }
            }
        }
    }
    let mut out: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..points.len() {
        let r = root(&mut parent, i);
        out.entry(r).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = out.into_values().collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.len()));
    out
}
fn mean(points: impl Iterator<Item = [f64; 3]>) -> Option<[f64; 3]> {
    let (mut sum, mut n) = ([0.; 3], 0usize);
    for p in points {
        for a in 0..3 {
            sum[a] += p[a];
        }
        n += 1;
    }
    (n > 0).then(|| sum.map(|v| v / n as f64))
}
/// Fighters with an opposite-side fighter within `reach`, grouped into clusters
/// (largest first) that carry the geometry of their contact line.
pub(crate) fn contact_clusters(fighters: &[Fighter], reach: f64, link: f64) -> Vec<Cluster> {
    let cells = grid(fighters.iter().map(|f| f.position), reach);
    let mut contact = vec![false; fighters.len()];
    for (i, f) in fighters.iter().enumerate() {
        if f.side != Side::Red {
            continue;
        }
        for j in near(&cells, f.position, reach) {
            if fighters[j].side == Side::Blue && close(f.position, fighters[j].position, reach) {
                contact[i] = true;
                contact[j] = true;
            }
        }
    }
    let engaged: Vec<usize> = (0..fighters.len()).filter(|&i| contact[i]).collect();
    let points: Vec<[f64; 3]> = engaged.iter().map(|&i| fighters[i].position).collect();
    groups(&points, link)
        .into_iter()
        .map(|group| {
            let members: Vec<usize> = group.into_iter().map(|g| engaged[g]).collect();
            let side_mean = |side| {
                mean(
                    members
                        .iter()
                        .filter(|&&m| fighters[m].side == side)
                        .map(|&m| fighters[m].position),
                )
            };
            let centroid = mean(members.iter().map(|&m| fighters[m].position)).unwrap();
            let (red_c, blue_c) = (
                side_mean(Side::Red).unwrap(),
                side_mean(Side::Blue).unwrap(),
            );
            let mut normal = [blue_c[0] - red_c[0], blue_c[2] - red_c[2]];
            let separation = normal[0].hypot(normal[1]);
            if separation < 0.25 {
                // Fully mixed melee: the contact line is the cluster's long axis.
                let (mut xx, mut zz, mut xz) = (0., 0., 0.);
                for &m in &members {
                    let (dx, dz) = (
                        fighters[m].position[0] - centroid[0],
                        fighters[m].position[2] - centroid[2],
                    );
                    xx += dx * dx;
                    zz += dz * dz;
                    xz += dx * dz;
                }
                let angle = 0.5 * (2. * xz).atan2(xx - zz);
                normal = [-angle.sin(), angle.cos()];
            } else {
                normal = [normal[0] / separation, normal[1] / separation];
            }
            let line = [-normal[1], normal[0]];
            let (mut min, mut max) = ([f64::MAX; 2], [f64::MIN; 2]);
            let (mut along, mut across) = ([f64::MAX, f64::MIN], [f64::MAX, f64::MIN]);
            for &m in &members {
                let p = fighters[m].position;
                for (a, axis) in [(0, 0), (1, 2)] {
                    min[a] = min[a].min(p[axis]);
                    max[a] = max[a].max(p[axis]);
                }
                let (dx, dz) = (p[0] - centroid[0], p[2] - centroid[2]);
                let (u, v) = (dx * line[0] + dz * line[1], dx * normal[0] + dz * normal[1]);
                along = [along[0].min(u), along[1].max(u)];
                across = [across[0].min(v), across[1].max(v)];
            }
            let red = members
                .iter()
                .filter(|&&m| fighters[m].side == Side::Red)
                .count();
            Cluster {
                blue: members.len() - red,
                red,
                centroid,
                min,
                max,
                normal,
                line,
                length: along[1] - along[0],
                depth: across[1] - across[0],
                members,
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Fighter,
    Horse,
    Arrow,
    Other,
}

struct Tracked {
    role: Role,
    position: [f64; 3],
    spawn: [f64; 3],
    sampled: [f64; 3],
    side: Option<Side>,
    weapon: Option<String>,
    vehicle: Option<i32>,
    dead: bool,
    stopped: bool,
    /// Arrows: the client simulates flight from the last server position and
    /// velocity (`base_tick`); the server rarely sends arrow movement.
    velocity: [f64; 3],
    base_tick: u64,
    spawn_tick: u64,
    first_contact: Option<u64>,
    death_tick: Option<u64>,
}

/// Vanilla arrow flight per tick (move, 0.99 drag, 0.05 gravity) for `ticks`
/// ticks, stopping where it enters the block surface. `top(x, z)` is the first
/// free y of a column, None outside the recorded terrain (no landing there).
pub(crate) fn fly(
    mut position: [f64; 3],
    mut velocity: [f64; 3],
    ticks: u64,
    top: impl Fn(f64, f64) -> Option<f64>,
) -> ([f64; 3], bool) {
    for _ in 0..ticks.min(200) {
        for a in 0..3 {
            position[a] += velocity[a];
        }
        if let Some(top) = top(position[0], position[2])
            && position[1] <= top
        {
            position[1] = top;
            return (position, true);
        }
        velocity = [
            velocity[0] * 0.99,
            velocity[1] * 0.99 - 0.05,
            velocity[2] * 0.99,
        ];
    }
    (position, false)
}
impl Tracked {
    /// A server position. Arrows get one about every 20 ticks and no velocity
    /// updates, so the velocity is re-derived from the last two positions by
    /// inverting `fly` over the interval: n ticks of v' = 0.99 v (- 0.05 on y)
    /// move sum(v_k) = v_0 s - g (n - s), with s = (1 - 0.99^n) / 0.01, g = 5.
    fn moved(&mut self, position: [f64; 3], tick: u64) {
        if self.role == Role::Arrow && tick > self.base_tick {
            let n = (tick - self.base_tick) as f64;
            let decay = 0.99f64.powf(n);
            let s = (1. - decay) / 0.01;
            let gravity = |axis: usize| if axis == 1 { 5. } else { 0. };
            self.velocity = std::array::from_fn(|a| {
                let start = (position[a] - self.position[a] + gravity(a) * (n - s)) / s;
                decay * start - gravity(a) * (1. - decay)
            });
        }
        (self.position, self.base_tick) = (position, tick);
    }
}
fn surface_top(chunks: &HashMap<(i32, i32), Box<[[i16; 256]; 2]>>, x: f64, z: f64) -> Option<f64> {
    let (x, z) = (x.floor() as i32, z.floor() as i32);
    chunks
        .get(&(x.div_euclid(16), z.div_euclid(16)))
        .map(|maps| f64::from(maps[0][(x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize]))
}

#[derive(Default)]
struct Interval {
    swings: Vec<i32>,
    hurts: u32,
    deaths: u32,
    launches: Vec<([f64; 3], Option<Side>)>,
    impacts: Vec<([f64; 3], Option<Side>)>,
}

#[derive(Default)]
pub(crate) struct Scan {
    entities: HashMap<i32, Tracked>,
    /// Fighters and horses that left the take (removed), kept for entity rows.
    finished: Vec<(i32, Tracked, u64)>,
    interval: Interval,
    split: Option<Split>,
    split_population: usize,
    pub(crate) first_tick: Option<u64>,
    pub(crate) tick: Option<u64>,
    component_sides: u64,
    samples: u64,
    last_sample_t: f64,
    /// Dimension layout (overworld default); chunk sections decode against it.
    min_y: i32,
    height: i32,
    /// Per chunk: first free y above the top non-air block (trees, leaves,
    /// grass) and above the top solid non-leaf block (ground), from the chunk
    /// sections plus later block updates. Packet heightmaps are not used: the
    /// integrated-ladder-2 platform is missing from them.
    chunks: HashMap<(i32, i32), Box<[[i16; 256]; 2]>>,
    terrain_kinds: HashMap<u32, Terrain>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Terrain {
    Air,
    Cover,
    Ground,
}

/// Air; cover (no collision, or leaves: hides fighters, not a floor); ground.
fn terrain_kind(content: &Content, state: u32) -> Result<Terrain> {
    let info = content.info(state)?;
    let leaves = content
        .names
        .get(info.block)
        .is_some_and(|name| name.ends_with("leaves"));
    Ok(if info.is_air {
        Terrain::Air
    } else if leaves
        || (info.collision == crate::protocol::content::Collision::Empty && info.fluid.is_none())
    {
        Terrain::Cover
    } else {
        Terrain::Ground
    })
}

/// [surface, ground] first-free y per column (index x + 16 z) of one chunk.
pub(crate) fn column_tops(
    sections: &[std::sync::Arc<crate::protocol::world::Section>],
    min_y: i32,
    mut kind: impl FnMut(u32) -> Result<Terrain>,
) -> Result<[[i16; 256]; 2]> {
    let mut out = [[min_y as i16; 256]; 2];
    for column in 0..256 {
        let mut found_surface = false;
        'down: for (index, section) in sections.iter().enumerate().rev() {
            if section.non_empty_block_count == 0 {
                continue;
            }
            for y in (0..16).rev() {
                let top = (min_y + index as i32 * 16 + y as i32 + 1) as i16;
                match kind(section.blocks.get(y * 256 + column))? {
                    Terrain::Air => {}
                    Terrain::Cover => {
                        if !found_surface {
                            found_surface = true;
                            out[0][column] = top;
                        }
                    }
                    Terrain::Ground => {
                        if !found_surface {
                            out[0][column] = top;
                        }
                        out[1][column] = top;
                        break 'down;
                    }
                }
            }
        }
    }
    Ok(out)
}

fn round(v: f64) -> f64 {
    (v * 100.).round() / 100.
}
fn r3(p: [f64; 3]) -> [f64; 3] {
    p.map(round)
}
impl Scan {
    pub(crate) fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            ..Self::default()
        }
    }
    fn kind(&mut self, content: &Content, state: u32) -> Result<Terrain> {
        if let Some(kind) = self.terrain_kinds.get(&state) {
            return Ok(*kind);
        }
        let kind = terrain_kind(content, state)?;
        self.terrain_kinds.insert(state, kind);
        Ok(kind)
    }
    /// Raise-only: a placed block can lift a column; a removed one never lowers it.
    fn block(&mut self, content: &Content, [x, y, z]: [i32; 3], state: u32) -> Result<()> {
        let kind = self.kind(content, state)?;
        if let Some(maps) = self.chunks.get_mut(&(x.div_euclid(16), z.div_euclid(16))) {
            let column = (x.rem_euclid(16) + z.rem_euclid(16) * 16) as usize;
            let top = (y + 1) as i16;
            if kind != Terrain::Air && maps[0][column] < top {
                maps[0][column] = top;
            }
            if kind == Terrain::Ground && maps[1][column] < top {
                maps[1][column] = top;
            }
        }
        Ok(())
    }
    fn side(&self, e: &Tracked) -> Option<Side> {
        e.side
            .or_else(|| {
                e.vehicle
                    .and_then(|v| self.entities.get(&v))
                    .and_then(|v| v.side)
            })
            .or_else(|| self.split.map(|s| s.side([e.spawn[0], e.spawn[2]])))
    }
    fn tick(&self) -> u64 {
        self.tick.unwrap_or(0)
    }
    fn t(&self, tick: u64) -> f64 {
        round((tick.saturating_sub(self.first_tick.unwrap_or(tick))) as f64 * 0.05)
    }

    pub(crate) fn apply(&mut self, id: i32, body: &[u8], content: &Content) -> Result<()> {
        let mut r = Reader::new(body);
        let tick = self.tick();
        match id {
            cb::LEVEL_CHUNK_WITH_LIGHT => {
                let sections = usize::try_from(self.height / 16).map_err(|_| "dimension height")?;
                let (position, chunk) =
                    crate::protocol::world::Chunk::decode(&mut r, sections, |state| {
                        content.block_is_air(state)
                    })
                    .map_err(|e| {
                        format!(
                            "{e} (dimension --min-y {} --height {}?)",
                            self.min_y, self.height
                        )
                    })?;
                let min_y = self.min_y;
                let tops = column_tops(&chunk.sections, min_y, |state| self.kind(content, state))?;
                self.chunks
                    .insert((position[0], position[1]), Box::new(tops));
            }
            cb::BLOCK_UPDATE => {
                let position = crate::protocol::unpack_pos(r.i64()?);
                let state = r.count(1_000_000)? as u32;
                r.done()?;
                self.block(content, position, state)?;
            }
            cb::SECTION_BLOCKS_UPDATE => {
                let packed = r.i64()?;
                let (sx, sy, sz) = (
                    (packed >> 42) as i32,
                    ((packed << 44) >> 44) as i32,
                    ((packed << 22) >> 42) as i32,
                );
                for _ in 0..r.count(4096)? {
                    let value = r.varlong()?;
                    let position = [
                        sx * 16 + ((value >> 8) & 15) as i32,
                        sy * 16 + (value & 15) as i32,
                        sz * 16 + ((value >> 4) & 15) as i32,
                    ];
                    self.block(content, position, (value >> 12) as u32)?;
                }
                r.done()?;
            }
            cb::ADD_ENTITY => {
                let entity = r.var()?;
                r.i64()?;
                r.i64()?; // UUID
                let kind = r.count(65535)? as u32;
                let position = [r.f64()?, r.f64()?, r.f64()?];
                let velocity = crate::entities::read_velocity(&mut r)?;
                r.u8()?;
                r.u8()?;
                r.u8()?;
                let data = r.var()?;
                r.done()?;
                let name = content
                    .entity_names
                    .get(&kind)
                    .ok_or_else(|| format!("unknown entity type {kind}"))?;
                let role = match name.as_str() {
                    "minecraft:arrow" | "minecraft:spectral_arrow" => Role::Arrow,
                    "minecraft:horse" => Role::Horse,
                    "minecraft:husk"
                    | "minecraft:zombie"
                    | "minecraft:skeleton"
                    | "minecraft:pillager"
                    | "minecraft:vindicator" => Role::Fighter,
                    _ => Role::Other,
                };
                if role == Role::Other {
                    self.remove(entity);
                    return Ok(());
                }
                // An arrow belongs to its shooter's side (spawn data is the owner id).
                let side = if role == Role::Arrow {
                    let owner = self
                        .entities
                        .get(&data)
                        .or_else(|| self.entities.get(&(data - 1)));
                    let side = owner.and_then(|o| self.side(o));
                    self.interval.launches.push((position, side));
                    side
                } else {
                    None
                };
                self.remove(entity);
                self.entities.insert(
                    entity,
                    Tracked {
                        role,
                        position,
                        spawn: position,
                        sampled: position,
                        side,
                        weapon: None,
                        vehicle: None,
                        dead: false,
                        stopped: false,
                        velocity,
                        base_tick: tick,
                        spawn_tick: tick,
                        first_contact: None,
                        death_tick: None,
                    },
                );
            }
            cb::MOVE_ENTITY_POS | cb::MOVE_ENTITY_POS_ROT => {
                let target = r.var()?;
                let props = r.var()?;
                let steps = props >> 1;
                let mut delta = [0.; 3];
                for _ in 0..steps.max(1) {
                    if steps != 0 {
                        r.var()?;
                    }
                    for coordinate in &mut delta {
                        *coordinate += f64::from(r.i16()?) / 4096.;
                    }
                }
                if id == cb::MOVE_ENTITY_POS_ROT {
                    r.u8()?;
                    r.u8()?;
                }
                r.done()?;
                if let Some(e) = self.entities.get_mut(&target) {
                    e.moved([0, 1, 2].map(|a| e.position[a] + delta[a]), tick);
                }
            }
            cb::ENTITY_POSITION_SYNC => {
                let target = r.var()?;
                let position = match r.var()? {
                    0 => [r.f64()?, r.f64()?, r.f64()?],
                    1 => {
                        let count = r.count(256)?;
                        if count == 0 {
                            return Err("empty entity position path".into());
                        }
                        let mut last = [0.; 3];
                        for _ in 0..count {
                            last = [r.f64()?, r.f64()?, r.f64()?];
                            r.var()?;
                        }
                        last
                    }
                    _ => return Err("unknown entity position path type".into()),
                };
                r.f32()?;
                r.f32()?;
                r.bool()?;
                r.done()?;
                if let Some(e) = self.entities.get_mut(&target) {
                    e.moved(position, tick);
                }
            }
            cb::TELEPORT_ENTITY => {
                let target = r.var()?;
                let mut position = [r.f64()?, r.f64()?, r.f64()?];
                for _ in 0..3 {
                    r.f64()?;
                }
                r.f32()?;
                r.f32()?;
                let flags = r.i32()? as u32;
                r.bool()?;
                r.done()?;
                if let Some(e) = self.entities.get_mut(&target) {
                    for (a, value) in position.iter_mut().enumerate() {
                        if flags & (1 << a) != 0 {
                            *value += e.position[a];
                        }
                    }
                    e.moved(position, tick);
                }
            }
            cb::SET_EQUIPMENT => {
                let target = r.var()?;
                let mut seen = 0u16;
                let (mut side, mut weapon) = (None, None);
                loop {
                    let b = r.u8()?;
                    let slot = b & 127;
                    if slot > 7 || seen & (1 << slot) != 0 {
                        return Err("invalid/duplicate entity equipment slot".into());
                    }
                    seen |= 1 << slot;
                    let stack = crate::inventory::read_stack(&mut r, content)?;
                    if !stack.empty() {
                        side = side.or_else(|| side_of_components(&stack.components));
                        if slot == 0 {
                            weapon = content.item_names.get(&stack.id).cloned();
                        }
                    }
                    if b & 128 == 0 {
                        break;
                    }
                }
                r.done()?;
                if let Some(e) = self.entities.get_mut(&target) {
                    if side.is_some() && e.side.is_none() {
                        self.component_sides += 1;
                    }
                    e.side = side.or(e.side);
                    e.weapon = weapon.or(e.weapon.take());
                }
            }
            cb::SET_PASSENGERS => {
                let vehicle = r.var()?;
                let n = r.count(4096)?;
                let riders = (0..n).map(|_| r.var()).collect::<Result<Vec<_>>>()?;
                r.done()?;
                for rider in riders {
                    if let Some(e) = self.entities.get_mut(&rider) {
                        e.vehicle = Some(vehicle);
                    }
                }
            }
            cb::SET_ENTITY_MOTION => {
                let target = r.var()?;
                let velocity = crate::entities::read_velocity(&mut r)?;
                r.done()?;
                if let Some(e) = self.entities.get_mut(&target)
                    && e.role == Role::Arrow
                    && !e.stopped
                {
                    let (position, _) = fly(e.position, e.velocity, tick - e.base_tick, |x, z| {
                        surface_top(&self.chunks, x, z)
                    });
                    (e.position, e.velocity, e.base_tick) = (position, velocity, tick);
                }
            }
            cb::ANIMATE => {
                let target = r.var()?;
                let action = r.u8()?;
                r.done()?;
                if matches!(action, 0 | 3) && self.entities.contains_key(&target) {
                    self.interval.swings.push(target);
                }
            }
            cb::SWING_ANIMATION => {
                let target = r.var()?;
                if self.entities.contains_key(&target) {
                    self.interval.swings.push(target);
                }
            }
            cb::DAMAGE_EVENT | cb::HURT_ANIMATION => {
                let target = r.var()?;
                if self
                    .entities
                    .get(&target)
                    .is_some_and(|e| e.role == Role::Fighter)
                {
                    self.interval.hurts += 1;
                }
            }
            cb::ENTITY_EVENT => {
                let target = r.i32()?;
                let event = r.u8()?;
                r.done()?;
                if event == 3
                    && let Some(e) = self.entities.get_mut(&target)
                    && !e.dead
                {
                    e.dead = true;
                    e.death_tick = Some(tick);
                    if e.role == Role::Fighter {
                        self.interval.deaths += 1;
                    }
                }
            }
            cb::REMOVE_ENTITIES => {
                let n = r.count(1 << 20)?;
                let ids = (0..n).map(|_| r.var()).collect::<Result<Vec<_>>>()?;
                r.done()?;
                for id in ids {
                    self.remove(id);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn remove(&mut self, id: i32) {
        let Some(e) = self.entities.remove(&id) else {
            return;
        };
        match e.role {
            Role::Arrow if !e.stopped => {
                // Removed in flight: it hit someone where its flight had reached.
                let (position, _) = fly(
                    e.position,
                    e.velocity,
                    self.tick().saturating_sub(e.base_tick),
                    |x, z| surface_top(&self.chunks, x, z),
                );
                let side = self.side(&e);
                self.interval.impacts.push((position, side));
            }
            Role::Fighter | Role::Horse if e.first_contact.is_some() || e.role == Role::Horse => {
                let tick = self.tick();
                self.finished.push((id, e, tick));
            }
            _ => {}
        }
    }

    fn refresh_split(&mut self) {
        let points: Vec<[f64; 2]> = self
            .entities
            .values()
            .filter(|e| e.role == Role::Fighter && e.side.is_none())
            .map(|e| [e.spawn[0], e.spawn[2]])
            .collect();
        if points.len() != self.split_population {
            self.split_population = points.len();
            self.split = spawn_split(&points);
        }
    }

    /// One timeline row for the state after every packet of `tick`.
    pub(crate) fn sample(&mut self, tick: u64, seconds: f64) -> Value {
        self.refresh_split();
        self.samples += 1;
        let mut fighters = Vec::new();
        let mut live = BTreeMap::from([("red", 0u32), ("blue", 0), ("unknown", 0)]);
        let mut army: HashMap<Side, Vec<[f64; 3]>> = HashMap::new();
        let riders: HashMap<i32, Side> = self
            .entities
            .values()
            .filter_map(|e| Some((e.vehicle?, self.side(e)?)))
            .collect();
        for (&id, e) in &self.entities {
            if e.role != Role::Fighter || e.dead {
                continue;
            }
            // A rider fights where its horse is; its own position is not streamed.
            let position = e
                .vehicle
                .and_then(|v| self.entities.get(&v))
                .map_or(e.position, |v| v.position);
            match self.side(e) {
                Some(side) => {
                    *live.get_mut(side.name()).unwrap() += 1;
                    army.entry(side).or_default().push(position);
                    fighters.push(Fighter { id, position, side });
                }
                None => *live.get_mut("unknown").unwrap() += 1,
            }
        }
        fighters.sort_by_key(|f| f.id);
        let clusters = contact_clusters(&fighters, CONTACT_REACH, CONTACT_LINK);
        let swings: std::collections::HashSet<i32> = self.interval.swings.iter().copied().collect();
        let mut contacts = Vec::new();
        for cluster in &clusters {
            for &m in &cluster.members {
                let e = self.entities.get_mut(&fighters[m].id).unwrap();
                e.first_contact.get_or_insert(tick);
            }
            if contacts.len() == MAX_CONTACTS || cluster.members.len() < 4 {
                continue;
            }
            let mut ids = BTreeMap::new();
            for side in [Side::Red, Side::Blue] {
                let mut near: Vec<(f64, i32)> = cluster
                    .members
                    .iter()
                    .filter(|&&m| fighters[m].side == side)
                    .map(|&m| {
                        let p = fighters[m].position;
                        (
                            (p[0] - cluster.centroid[0]).hypot(p[2] - cluster.centroid[2]),
                            fighters[m].id,
                        )
                    })
                    .collect();
                near.sort_by(|a, b| a.0.total_cmp(&b.0));
                ids.insert(
                    side.name(),
                    near.iter()
                        .take(IDS_PER_SIDE)
                        .map(|n| n.1)
                        .collect::<Vec<_>>(),
                );
            }
            contacts.push(json!({"count":cluster.members.len(),"red":cluster.red,"blue":cluster.blue,
                "centroid":r3(cluster.centroid),"min":cluster.min.map(round),"max":cluster.max.map(round),
                "normal":cluster.normal.map(round),"line":cluster.line.map(round),
                "length":round(cluster.length),"depth":round(cluster.depth),
                "swings":cluster.members.iter().filter(|&&m| swings.contains(&fighters[m].id)).count(),
                "ids":ids}));
        }
        let interval_seconds = (seconds - self.last_sample_t).max(0.05);
        // Arrows, per shooter side: in flight when moved since the previous
        // sample; the first sample with no movement after flight is the impact.
        let split = self.split;
        let (mut flight, mut impacts) = (Vec::new(), std::mem::take(&mut self.interval.impacts));
        for e in self
            .entities
            .values_mut()
            .filter(|e| e.role == Role::Arrow && !e.stopped)
        {
            let side = e
                .side
                .or_else(|| split.map(|s| s.side([e.spawn[0], e.spawn[2]])));
            let (position, landed) = fly(
                e.position,
                e.velocity,
                tick.saturating_sub(e.base_tick),
                |x, z| surface_top(&self.chunks, x, z),
            );
            if landed {
                (e.stopped, e.position) = (true, position);
                impacts.push((position, side));
            } else {
                flight.push((position, side));
            }
        }
        let area = |points: &[[f64; 3]]| -> Value {
            let Some(c) = mean(points.iter().copied()) else {
                return Value::Null;
            };
            let (mut min, mut max) = ([f64::MAX; 3], [f64::MIN; 3]);
            for p in points {
                for a in 0..3 {
                    min[a] = min[a].min(p[a]);
                    max[a] = max[a].max(p[a]);
                }
            }
            json!({"centroid":r3(c),"min":r3(min),"max":r3(max)})
        };
        let of = |points: &[([f64; 3], Option<Side>)], side: Side| -> Vec<[f64; 3]> {
            points
                .iter()
                .filter(|p| p.1 == Some(side))
                .map(|p| p.0)
                .collect()
        };
        let mut arrows = serde_json::Map::new();
        arrows.insert("in_flight".into(), json!(flight.len()));
        arrows.insert("launched".into(), json!(self.interval.launches.len()));
        arrows.insert("impacts".into(), json!(impacts.len()));
        for side in [Side::Red, Side::Blue] {
            let (f, l, i) = (
                of(&flight, side),
                of(&self.interval.launches, side),
                of(&impacts, side),
            );
            arrows.insert(
                side.name().into(),
                json!({"in_flight":f.len(),"launched":l.len(),"impacts":i.len(),
                "flight":area(&f),"launch":area(&l),"impact":area(&i)}),
            );
        }
        let (mut melee_swings, mut bow_swings) = (0usize, 0usize);
        for id in &self.interval.swings {
            match self.entities.get(id).and_then(|e| e.weapon.as_deref()) {
                Some("minecraft:bow" | "minecraft:crossbow") => bow_swings += 1,
                _ => melee_swings += 1,
            }
        }
        // Cavalry: live horses grouped per side, with mean velocity per group.
        let mut cavalry = Vec::new();
        for side in [Side::Red, Side::Blue] {
            let horses: Vec<(i32, [f64; 3], [f64; 3])> = self
                .entities
                .iter()
                .filter(|(_, e)| e.role == Role::Horse && !e.dead)
                .filter(|(id, e)| {
                    e.side
                        .or_else(|| riders.get(id).copied())
                        .or_else(|| self.split.map(|s| s.side([e.spawn[0], e.spawn[2]])))
                        == Some(side)
                })
                .map(|(&id, e)| (id, e.position, e.sampled))
                .collect();
            let points: Vec<[f64; 3]> = horses.iter().map(|h| h.1).collect();
            for group in groups(&points, CAVALRY_LINK)
                .into_iter()
                .filter(|g| g.len() >= 3)
                .take(4)
            {
                let centroid = mean(group.iter().map(|&g| horses[g].1)).unwrap();
                let previous = mean(group.iter().map(|&g| horses[g].2)).unwrap();
                let mut ids: Vec<(f64, i32)> = group
                    .iter()
                    .map(|&g| {
                        (
                            (horses[g].1[0] - centroid[0]).hypot(horses[g].1[2] - centroid[2]),
                            horses[g].0,
                        )
                    })
                    .collect();
                ids.sort_by(|a, b| a.0.total_cmp(&b.0));
                let extent = area(&group.iter().map(|&g| horses[g].1).collect::<Vec<_>>());
                cavalry.push(json!({"side":side.name(),"count":group.len(),"centroid":r3(centroid),
                    "min":extent["min"],"max":extent["max"],
                    "velocity":[round((centroid[0]-previous[0])/interval_seconds),round((centroid[2]-previous[2])/interval_seconds)],
                    "ids":ids.iter().take(IDS_PER_SIDE).map(|i| i.1).collect::<Vec<_>>()}));
            }
        }
        let armies: BTreeMap<&str, Value> = army
            .iter()
            .map(|(side, points)| (side.name(), area(points)))
            .collect();
        for e in self.entities.values_mut() {
            e.sampled = e.position;
        }
        let interval = std::mem::take(&mut self.interval);
        self.last_sample_t = seconds;
        json!({"kind":"sample","t":seconds,"tick":tick,"live":live,
            "events":{"swings":interval.swings.len(),"melee_swings":melee_swings,"bow_swings":bow_swings,"hurts":interval.hurts,"deaths":interval.deaths},
            "contact_fighters":clusters.iter().map(|c| c.members.len()).sum::<usize>(),
            "contacts":contacts,"arrows":arrows,"cavalry":cavalry,"armies":armies})
    }

    fn entity_rows(&self) -> Vec<Value> {
        let row = |id: i32, e: &Tracked, removed: Option<u64>| {
            json!({"kind":"entity","id":id,
            "role":if e.role == Role::Horse { "horse" } else { "fighter" },
            "side":self.side(e).map(Side::name),"weapon":e.weapon,"vehicle":e.vehicle,
            "spawn_t":self.t(e.spawn_tick),"first_contact_t":e.first_contact.map(|t| self.t(t)),
            "death_t":e.death_tick.map(|t| self.t(t)),"removed_t":removed.map(|t| self.t(t))})
        };
        let mut rows: Vec<(i32, Value)> = self
            .finished
            .iter()
            .map(|(id, e, removed)| (*id, row(*id, e, Some(*removed))))
            .collect();
        rows.extend(
            self.entities
                .iter()
                .filter(|(_, e)| {
                    e.role == Role::Horse || (e.role == Role::Fighter && e.first_contact.is_some())
                })
                .map(|(&id, e)| (id, row(id, e, None))),
        );
        rows.sort_by_key(|r| r.0);
        rows.into_iter().map(|r| r.1).collect()
    }
}

pub(crate) fn cli(mut args: impl Iterator<Item = String>) -> Result<()> {
    let (mut recording, mut output, mut data, mut every) = (None, None, None, 10u64);
    let (mut min_y, mut height) = (-64i32, 384i32);
    while let Some(arg) = args.next() {
        if arg == "--help" {
            println!(
                "scan-replay --recording PATH --output NEW.jsonl [--data-dir PATH] [--interval-ticks 1..200] [--min-y -64 --height 384]"
            );
            return Ok(());
        }
        let value = args
            .next()
            .ok_or_else(|| format!("scan-replay missing value for {arg}"))?;
        match arg.as_str() {
            "--recording" => recording = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--data-dir" => data = Some(PathBuf::from(value)),
            "--interval-ticks" => every = value.parse::<u64>().map_err(|e| e.to_string())?,
            "--min-y" => min_y = value.parse::<i32>().map_err(|e| e.to_string())?,
            "--height" => height = value.parse::<i32>().map_err(|e| e.to_string())?,
            _ => return Err(format!("unknown scan-replay argument {arg}")),
        }
    }
    if !(1..=200).contains(&every) {
        return Err("--interval-ticks must be 1..200".into());
    }
    if height <= 0 || height % 16 != 0 || min_y % 16 != 0 {
        return Err("--min-y and --height must be multiples of 16".into());
    }
    let recording = recording.ok_or("scan-replay requires --recording")?;
    let output = output.ok_or("scan-replay requires --output")?;
    let root = data
        .map(Ok)
        .unwrap_or_else(crate::content_paths::default_root)?;
    crate::content_paths::apply_defaults(&root);
    crate::content_paths::validate(false)?;
    let content = Content::load_state()?;
    let mut out = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| format!("create {}: {e}", output.display()))?,
    );
    let mut input = RecordingReader::open(&recording)?;
    let mut write =
        |value: &Value| writeln!(out, "{value}").map_err(|e| format!("write timeline: {e}"));
    write(
        &json!({"kind":"header","recording":recording,"server_build":input.header.server_build,
        "interval_ticks":every,"contact_reach":CONTACT_REACH,"contact_link":CONTACT_LINK,"min_y":min_y,"height":height}),
    )?;
    let (mut scan, mut phase, mut packets) = (Scan::new(min_y, height), Phase::Login, 0u64);
    while let Some(packet) = input.next_packet()? {
        packets += 1;
        let (id, body) = packet.split()?;
        if let Some((tick, _)) = tick_marker(phase, id, body)? {
            let first = *scan.first_tick.get_or_insert(tick);
            if let Some(previous) = scan.tick
                && (tick - first) / every > (previous - first) / every
            {
                let seconds = round((previous - first) as f64 * 0.05);
                write(&scan.sample(previous, seconds))?;
            }
            scan.tick = Some(tick);
        } else if phase == Phase::Play {
            scan.apply(id, body, &content)
                .map_err(|e| format!("scan-replay packet {id} at tick {:?}: {e}", scan.tick))?;
        }
        phase = phase_after(phase, id);
    }
    let last = scan
        .tick
        .ok_or("recording has no krasnokamen:tick marker")?;
    let seconds = round((last - scan.first_tick.unwrap()) as f64 * 0.05);
    write(&scan.sample(last, seconds))?;
    for row in scan.entity_rows() {
        write(&row)?;
    }
    let mut chunks: Vec<_> = scan.chunks.iter().collect();
    chunks.sort_by_key(|(position, _)| **position);
    for ((x, z), maps) in chunks {
        write(
            &json!({"kind":"chunk","x":x,"z":z,"surface":maps[0].as_slice(),"ground":maps[1].as_slice()}),
        )?;
    }
    let split = scan.split.map(
        |s| json!({"axis":if s.axis == 0 { "x" } else { "z" },"threshold":round(s.threshold)}),
    );
    write(
        &json!({"kind":"summary","first_tick":scan.first_tick,"last_tick":last,"packets":packets,"samples":scan.samples,
        "side_source":if scan.component_sides > 0 { "components" } else { "spawn_split" },
        "component_sides":scan.component_sides,"chunks":scan.chunks.len(),"split":split}),
    )?;
    out.flush().map_err(|e| format!("flush timeline: {e}"))?;
    eprintln!(
        "scan-replay: {packets} packets, {} samples -> {}",
        scan.samples,
        output.display()
    );
    Ok(())
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
