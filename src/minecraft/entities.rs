//! Bounded entity tools use received identity/state and the ordinary player action path.
use super::{lifecycle, Outcome};
use crate::{
    api::InputIntent,
    entities::{Entity, Handle, Metadata},
    player::{Bounds, InteractionIntent},
    protocol::{content::Content, Result},
    session::Session,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(super) const SUPPORTED_TOOLS: &[&str] = &[
    "minecraft_list_entities",
    "minecraft_wait_for_visible_entity",
    "minecraft_wait_for_entity_motion",
    "minecraft_wait_for_entity_removed",
    "minecraft_wait_for_health_below",
    "minecraft_approach_entity",
    "minecraft_interact_entity",
    "minecraft_attack_entity_once",
    "minecraft_attack_entity_until_drop_collected",
];

#[derive(Clone)]
struct Identity {
    id: i32,
    uuid: String,
    kind: String,
    handle: Option<Handle>,
}
impl Identity {
    fn capture(entity: &Entity) -> Self {
        Self {
            id: entity.handle.network_id,
            uuid: entity.uuid.clone(),
            kind: entity.type_name.clone(),
            handle: Some(entity.handle),
        }
    }
    fn lookup<'a>(&self, session: &'a Session) -> Option<&'a Entity> {
        session.entities.get(self.id).filter(|entity| {
            entity.uuid == self.uuid
                && entity.type_name == self.kind
                && self.handle.is_none_or(|handle| entity.handle == handle)
        })
    }
    fn value(&self) -> Value {
        json!({"entity_id":self.id,"entity_uuid":self.uuid,"entity_type":self.kind})
    }
}
struct Motion {
    origin: [f64; 3],
    lowest_y: f64,
    horizontal: f64,
    rise: f64,
    speed: f64,
    yaw_delta: f64,
    minimum_distance: f64,
    minimum_rise: f64,
    revision: u64,
}
struct Movement {
    last_tick: Option<u64>,
    detour: i32,
    preferred: i32,
    colliders: Vec<Bounds>,
}
impl Default for Movement {
    fn default() -> Self {
        Self {
            last_tick: None,
            detour: 0,
            preferred: 1,
            colliders: Vec::new(),
        }
    }
}
impl Movement {
    fn step(&mut self, session: &mut Session, point: [f64; 3]) -> Result<()> {
        if self.last_tick == Some(session.client_tick) {
            return Ok(());
        }
        self.last_tick = Some(session.client_tick);
        let next = lifecycle::pickup_step(
            session,
            point,
            self.detour,
            self.preferred,
            &mut self.colliders,
        )?;
        if self.detour == 0 && next != 0 {
            self.preferred = -next
        }
        self.detour = next;
        Ok(())
    }
}
enum Wait {
    Visible {
        kind: String,
        radius: f64,
        version: Option<(u64, u64)>,
    },
    Motion {
        identity: Identity,
        motion: Motion,
    },
    Removed(Identity),
    Health(f64),
    Approach {
        identity: Identity,
        movement: Movement,
    },
    Attack {
        identity: Identity,
        health: Option<f32>,
        strength: f32,
    },
    Collect {
        identity: Identity,
        item: String,
        count: i64,
        before: i64,
        started: bool,
        removed: bool,
        saw_drop: bool,
        last_position: [f64; 3],
        last_attack: Option<u64>,
        movement: Movement,
    },
}
pub(super) struct Task {
    deadline: Instant,
    epochs: (u64, u64),
    wait: Wait,
}
fn content(session: &Session) -> Result<&Content> {
    if session.phase != "play" || !session.player.pose_known {
        return Err("client is not in play".into());
    }
    session
        .content
        .as_ref()
        .ok_or_else(|| "client content not configured".into())
}
fn number(args: &Value, key: &str, default: Option<f64>, min: f64, max: f64) -> Result<f64> {
    let value = args
        .get(key)
        .map(Value::as_f64)
        .unwrap_or(default)
        .filter(|value| value.is_finite() && (min..=max).contains(value));
    value.ok_or_else(|| format!("{key} must be a number between {min} and {max}"))
}
fn integer(args: &Value, key: &str, default: Option<i64>, min: i64, max: i64) -> Result<i64> {
    args.get(key)
        .map(Value::as_i64)
        .unwrap_or(default)
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("{key} must be an integer between {min} and {max}"))
}
fn text<'a>(args: &'a Value, key: &str, max: usize) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= max)
        .ok_or_else(|| format!("{key} must be a nonempty string of at most {max} bytes"))
}
fn identity(
    session: &Session,
    args: &Value,
    fenced: bool,
    require_present: bool,
) -> Result<Identity> {
    let id = integer(args, "entity_id", None, 0, i32::MAX as i64)? as i32;
    let observed = session.entities.get(id);
    let result = if fenced {
        let uuid = uuid::Uuid::parse_str(text(args, "entity_uuid", 36)?)
            .map_err(|_| "invalid entity UUID")?
            .to_string();
        let kind = text(args, "entity_type", 128)?.to_owned();
        Identity {
            id,
            uuid,
            kind,
            handle: observed.map(|entity| entity.handle),
        }
    } else {
        Identity::capture(observed.ok_or("entity is not client-visible")?)
    };
    if require_present && result.lookup(session).is_none() {
        return Err("entity identity changed or is not client-visible".into());
    }
    Ok(result)
}
fn health(session: &Session, handle: Handle) -> Result<Option<f32>> {
    Ok(
        match session
            .entities
            .received_metadata(handle, "DATA_HEALTH_ID")?
        {
            Some(Metadata::Float(value)) => Some(*value),
            None => None,
            _ => return Err("received entity health serializer mismatch".into()),
        },
    )
}

fn received_baby(
    supported: Option<bool>,
    metadata: Option<&Metadata>,
) -> Result<(&'static str, Option<bool>)> {
    match (supported, metadata) {
        (None, _) => Ok(("unknown_schema", None)),
        (Some(false), _) => Ok(("unsupported", None)),
        (Some(true), None) => Ok(("missing", None)),
        (Some(true), Some(Metadata::Bool(value))) => Ok(("received", Some(*value))),
        (Some(true), Some(_)) => Err("received entity baby serializer mismatch".into()),
    }
}

// The binding is a cached presentation decision, not evidence of a rendered frame.
fn list_entity_value(session: &Session, entity: &Entity) -> Result<Value> {
    let models = session
        .entities
        .models_ref()
        .ok_or("entity models not activated")?;
    let supported = models
        .schemas
        .get(&entity.type_name)
        .map(|schema| schema.accessors.contains_key("DATA_BABY_ID"));
    let metadata = session
        .entities
        .received_metadata(entity.handle, "DATA_BABY_ID")?;
    let (baby_status, baby) = received_baby(supported, metadata)?;
    let (rig_status, rig, rig_error) = match entity.binding.as_ref() {
        None => ("pending", None, None),
        Some(Ok(binding)) => ("resolved", Some(binding.rig.as_str()), None),
        Some(Err(error)) => ("error", None, Some(error.as_str())),
    };
    let mut value = entity_value(session, entity)?;
    let walk = entity.source.as_ref().and_then(|source| source.living.as_ref());
    extend(
        value.as_object_mut().unwrap(),
        json!({
            "received_metadata": entity.metadata,
            "received_baby": baby,
            "received_baby_status": baby_status,
            "cached_rig": rig,
            "cached_rig_status": rig_status,
            "walk_position": walk.map(|living| living.walk.position(1.)),
            "walk_speed": walk.map(|living| living.walk.speed(1.)),
            "cached_rig_error": rig_error
        }),
    );
    Ok(value)
}
fn extend(object: &mut serde_json::Map<String, Value>, value: Value) {
    if let Value::Object(fields) = value {
        object.extend(fields);
    }
}
fn yaw_delta(entity: &Entity) -> f64 {
    if entity.velocity[0].hypot(entity.velocity[2]) > 0.01 {
        let movement_yaw = (-entity.velocity[0]).atan2(entity.velocity[2]).to_degrees();
        ((f64::from(entity.transform.yaw) - movement_yaw + 180.).rem_euclid(360.) - 180.).abs()
    } else {
        180.
    }
}
fn distance_squared(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|axis| (a[axis] - b[axis]).powi(2)).sum()
}
fn entity_value(session: &Session, entity: &Entity) -> Result<Value> {
    let flags = session
        .entities
        .received_metadata(entity.handle, "DATA_SHARED_FLAGS_ID")?
        .and_then(Metadata::int);
    let bounds = session
        .entities
        .entity_bounds(entity.handle, session.world_tick)?;
    let mut value = Identity::capture(entity).value();
    let object = value.as_object_mut().unwrap();
    extend(
        object,
        json!({"x":entity.position[0],"y":entity.position[1],"z":entity.position[2],
        "yaw":entity.transform.yaw,"pitch":entity.transform.pitch,
        "distance":distance_squared(entity.position, session.player.position).sqrt(),
        "local_player":entity.handle.network_id == session.player.entity_id,
        "on_ground":entity.on_ground,"crouching":flags.map(|flags| flags & 2 != 0),
        "sprinting":flags.map(|flags| flags & 8 != 0),"swimming":flags.map(|flags| flags & 16 != 0),
        "bounding_box":{"min_x":bounds.min[0],"min_y":bounds.min[1],"min_z":bounds.min[2],
            "max_x":bounds.max[0],"max_y":bounds.max[1],"max_z":bounds.max[2]},
        "velocity":{"x":entity.velocity[0],"y":entity.velocity[1],"z":entity.velocity[2]},
        "generation":entity.handle.generation,"state_origin":"received"}),
    );
    if let Some(value) = health(session, entity.handle)? {
        object.insert("health".into(), json!(value));
    }
    if entity.type_name == "minecraft:item" {
        if let Some(Metadata::Item(stack)) = session
            .entities
            .received_metadata(entity.handle, "DATA_ITEM")?
        {
            let item = if stack.empty() {
                "minecraft:air"
            } else {
                content(session)?
                    .item_names
                    .get(&stack.id)
                    .ok_or("unknown received entity item")?
            };
            object.insert("item".into(), json!({"item_id":item,"count":stack.count}));
        }
    }
    Ok(value)
}
/// Aims at any part of the target's box whose ordinary raycast resolves to it.
/// `Some(reason)` is a typed refusal (out of reach / occluded), never a stale identity.
fn aim_entity(session: &mut Session, handle: Handle) -> Result<Option<String>> {
    session.aim_at_entity(handle, true)
}
/// Eye-to-box distance is the vanilla entity-interaction metric, not feet-to-feet.
fn in_reach(session: &Session, handle: Handle) -> Result<bool> {
    let bounds = session
        .entities
        .entity_bounds(handle, session.world_tick)?;
    let eye = session.player.eye_position();
    let nearest: [f64; 3] =
        std::array::from_fn(|axis| eye[axis].clamp(bounds.min[axis], bounds.max[axis]));
    let reach = session
        .player
        .attribute("minecraft:entity_interaction_range");
    Ok(distance_squared(nearest, eye) <= reach * reach)
}
fn attack(session: &mut Session, handle: Handle) -> Result<bool> {
    if aim_entity(session, handle)?.is_some() {
        return Ok(false);
    }
    dispatch_attack(session, handle)
}
fn dispatch_attack(session: &mut Session, handle: Handle) -> Result<bool> {
    let Some(result) = session.interact_entity(handle, InteractionIntent::Attack)? else {
        return Ok(false);
    };
    result["started"]
        .as_bool()
        .ok_or_else(|| "ordinary entity attack omitted started fact".into())
}
fn inventory_count(session: &Session, item: &str) -> Result<i64> {
    let content = content(session)?;
    let mut count = 0;
    for slot in 5..46 {
        let stack = session
            .inventory
            .received_player_slot(slot)?
            .ok_or("player inventory slot not received")?;
        if !stack.empty()
            && content
                .item_names
                .get(&stack.id)
                .is_some_and(|name| name == item)
        {
            count += i64::from(stack.count);
        }
    }
    Ok(count)
}
fn collection_value(
    session: &Session,
    identity: &Identity,
    item: &str,
    count: i64,
    started: bool,
    removed: bool,
    saw_drop: bool,
    collected: bool,
) -> Result<Value> {
    let mut result = identity.value();
    let selected = session
        .inventory
        .received_player_slot(36 + u16::from(session.inventory.selected_slot))?
        .ok_or("selected player inventory slot not received")?;
    let name = if selected.empty() {
        "minecraft:air"
    } else {
        content(session)?
            .item_names
            .get(&selected.id)
            .ok_or("unknown selected item")?
    };
    extend(
        result.as_object_mut().unwrap(),
        json!({"expected_drop_item_id":item,"expected_drop_count":count,
        "attack_started":started,"entity_removed":removed,"saw_drop":saw_drop,"pickup_restored":collected,
        "selected_item_id":name,"selected_item_count":selected.count}),
    );
    Ok(result)
}

pub(super) fn visible_item(session: &Session, item: &str, origin: [i32; 3]) -> Result<bool> {
    Ok(session
        .entities
        .nearest_visible_item(item, origin, content(session)?, session.world_tick)?
        .is_some())
}
/// Ordinary, unpaged calls retain nearest-first order.
fn entity_page<'a>(
    entities: impl Iterator<Item = (f64, i32, &'a str)>,
    radius: f64,
    limit: usize,
    kind: Option<&str>,
) -> (Vec<(f64, i32)>, usize) {
    let mut page = Vec::with_capacity(limit + 1);
    let mut total = 0;
    for (distance, id, entity_kind) in entities {
        if distance > radius * radius || kind.is_some_and(|kind| kind != entity_kind) {
            continue;
        }
        total += 1;
        let index = page.partition_point(|other: &(f64, i32)| *other < (distance, id));
        if index < limit {
            page.insert(index, (distance, id));
            if page.len() > limit {
                page.pop();
            }
        }
    }
    (page, total)
}

impl super::EntityCensus {
    fn begin(
        &mut self,
        epochs: (u64, u64),
        radius: f64,
        kind: Option<&str>,
        game_time: i64,
        mut rows: Vec<(i32, Value)>,
    ) {
        rows.sort_unstable_by_key(|(id, _)| *id);
        self.snapshot = Some(super::EntityCensusSnapshot {
            epochs,
            radius,
            kind: kind.map(str::to_owned),
            game_time,
            total: rows.len(),
            rows: rows.into_iter(),
        });
    }

    fn page(
        &mut self,
        epochs: (u64, u64),
        radius: f64,
        kind: Option<&str>,
        cursor: &str,
        limit: usize,
    ) -> Result<Value> {
        if !cursor.is_empty()
            && cursor.parse::<u64>().map_err(|_| "invalid_cursor")? != self.serial
        {
            return Err("entity census cursor expired".into());
        }
        let snapshot = self.snapshot.as_mut().ok_or("entity census cursor expired")?;
        if snapshot.epochs != epochs || snapshot.radius != radius || snapshot.kind.as_deref() != kind {
            return Err("entity census query or context changed".into());
        }
        let serial = self.serial.checked_add(1).ok_or("entity census cursor exhausted")?;
        let entities = snapshot.rows.by_ref().take(limit).map(|(_, value)| value).collect::<Vec<_>>();
        let more = snapshot.rows.len() != 0;
        let mut value = json!({
            "in_play":true,"radius":radius,"visible_count":snapshot.total,"game_time":snapshot.game_time,
            "entities":[],"truncated":more,"next_cursor":more.then(|| serial.to_string()),
        });
        value["entities"] = Value::Array(entities);
        self.serial = serial;
        if !more {
            self.snapshot = None;
        }
        Ok(value)
    }
}

pub(super) fn start(
    session: &mut Session,
    name: &str,
    args: &Value,
) -> Result<Option<Outcome<Task>>> {
    if !SUPPORTED_TOOLS.contains(&name) {
        return Ok(None);
    }
    content(session)?;
    let default = if matches!(
        name,
        "minecraft_approach_entity" | "minecraft_attack_entity_until_drop_collected"
    ) {
        30.
    } else {
        8.
    };
    let seconds = number(args, "timeout_seconds", Some(default), 0.1, 120.)?;
    let wait = match name {
        "minecraft_list_entities" => {
            let radius = number(args, "radius", Some(32.), 0., 128.)?;
            let limit = integer(args, "limit", Some(128), 1, 512)? as usize;
            let kind = args.get("entity_type").map(|_| text(args, "entity_type", 128)).transpose()?;
            let cursor = args.get("cursor").map(|value| value.as_str().ok_or("invalid_cursor")).transpose()?;
            if let Some(cursor) = cursor {
                let epochs = session.context_epochs();
                if cursor.is_empty() {
                    let radius_squared = radius * radius;
                    let rows = session.entities.received_entities()
                        .filter(|entity| {
                            distance_squared(entity.position, session.player.position) <= radius_squared
                                && kind.is_none_or(|kind| kind == entity.type_name)
                        })
                        .map(|entity| list_entity_value(session, entity)
                            .map(|value| (entity.handle.network_id, value)))
                        .collect::<Result<Vec<_>>>()?;
                    session.entity_census.begin(epochs, radius, kind, session.game_time(), rows);
                }
                return Ok(Some(Outcome::Complete(
                    session.entity_census.page(epochs, radius, kind, cursor, limit)?,
                )));
            }
            let (page, total) = entity_page(
                session.entities.received_entities().map(|entity| (
                    distance_squared(entity.position, session.player.position),
                    entity.handle.network_id,
                    entity.type_name.as_str(),
                )),
                radius, limit, kind,
            );
            let entities = page
                .iter()
                .map(|(_, id)| list_entity_value(session, session.entities.get(*id).unwrap()))
                .collect::<Result<Vec<_>>>()?;
            return Ok(Some(Outcome::Complete(
                json!({"in_play":true,"radius":radius,"visible_count":total,
                "game_time":session.game_time(),"entities":entities,
                "truncated":total > limit,"next_cursor":null}),
            )));
        }
        "minecraft_wait_for_visible_entity" => Wait::Visible {
            kind: text(args, "entity_type", 128)?.into(),
            radius: number(args, "radius", Some(32.), 0., 128.)?,
            version: None,
        },
        "minecraft_wait_for_entity_removed" => Wait::Removed(identity(session, args, true, false)?),
        "minecraft_wait_for_entity_motion" => {
            let identity = identity(session, args, true, true)?;
            let entity = identity.lookup(session).unwrap();
            Wait::Motion {
                identity: identity.clone(),
                motion: Motion {
                    origin: entity.position,
                    lowest_y: entity.position[1],
                    horizontal: 0.,
                    rise: 0.,
                    speed: entity.velocity[0].hypot(entity.velocity[2]),
                    yaw_delta: yaw_delta(entity),
                    minimum_distance: number(
                        args,
                        "minimum_horizontal_distance",
                        Some(0.01),
                        0.001,
                        128.,
                    )?,
                    minimum_rise: number(args, "minimum_vertical_rise", Some(0.), 0., 128.)?,
                    revision: entity.revision,
                },
            }
        }
        "minecraft_wait_for_health_below" => {
            Wait::Health(number(args, "health", None, 0.001, 2048.)?)
        }
        "minecraft_approach_entity" => Wait::Approach {
            identity: identity(session, args, false, true)?,
            movement: Movement::default(),
        },
        "minecraft_interact_entity" => {
            let identity = identity(session, args, true, true)?;
            let hand_name = args
                .get("hand")
                .and_then(Value::as_str)
                .unwrap_or("main_hand");
            let hand = match hand_name {
                "main_hand" => 0,
                "off_hand" => 1,
                _ => return Err("invalid entity interaction hand".into()),
            };
            let handle = identity.handle.unwrap();
            if let Some(reason) = aim_entity(session, handle)? {
                return Err(reason);
            }
            let observed = session
                .interact_entity(handle, InteractionIntent::Use { hand })?
                .ok_or("ordinary raycast does not target requested entity")?;
            // The executor owns the local interaction result. Dispatch is not server success.
            let mut result = identity.value();
            let object = result.as_object_mut().unwrap();
            object.insert("hand".into(), json!(hand_name));
            object.insert("dispatched".into(), json!(true));
            for key in ["result", "consumes_action", "hit_x", "hit_y", "hit_z"] {
                object.insert(
                    key.into(),
                    observed
                        .get(key)
                        .ok_or_else(|| format!("ordinary entity interaction omitted {key}"))?
                        .clone(),
                );
            }
            return Ok(Some(Outcome::Complete(result)));
        }
        "minecraft_attack_entity_once" => {
            let identity = identity(session, args, true, true)?;
            let handle = identity.handle.unwrap();
            let health = health(session, handle)?;
            let strength = session.player.attack_strength(0.);
            if let Some(reason) = aim_entity(session, handle)? {
                return Err(reason);
            }
            if !dispatch_attack(session, handle)? {
                return Err("ordinary entity attack was not dispatched".into());
            }
            Wait::Attack {
                identity,
                health,
                strength,
            }
        }
        "minecraft_attack_entity_until_drop_collected" => {
            let identity = identity(session, args, false, true)?;
            let last_position = identity.lookup(session).unwrap().position;
            let item = text(args, "expected_drop_item_id", 128)?.to_owned();
            let before = inventory_count(session, &item)?;
            Wait::Collect {
                identity,
                item,
                count: integer(args, "expected_drop_count", None, 1, 64)?,
                before,
                started: false,
                removed: false,
                saw_drop: false,
                last_position,
                last_attack: None,
                movement: Movement::default(),
            }
        }
        _ => unreachable!(),
    };
    let mut task = Task {
        deadline: Instant::now() + Duration::from_secs_f64(seconds),
        epochs: session.context_epochs(),
        wait,
    };
    match task.on_event(session) {
        Ok(Some(value)) => Ok(Some(Outcome::Complete(value))),
        Ok(None) => Ok(Some(Outcome::Pending(task))),
        Err(error) => {
            let _ = task.cancel(session);
            Err(error)
        }
    }
}
impl Task {
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(super) fn cancel(&mut self, session: &mut Session) -> Result<()> {
        if matches!(self.wait, Wait::Approach { .. } | Wait::Collect { .. }) {
            lifecycle::clear_pickup(session)?;
        }
        Ok(())
    }
    pub(super) fn on_event(&mut self, session: &mut Session) -> Result<Option<Value>> {
        match self.evaluate(session) {
            Ok(value) => Ok(value),
            Err(error) => {
                let _ = self.cancel(session);
                Err(error)
            }
        }
    }
    fn evaluate(&mut self, session: &mut Session) -> Result<Option<Value>> {
        if session.context_epochs() != self.epochs {
            self.cancel(session)?;
            return Err("entity action context changed".into());
        }
        content(session)?;
        let expired = Instant::now() >= self.deadline;
        let result = match &mut self.wait {
            Wait::Visible {
                kind,
                radius,
                version,
            } => {
                let current = (session.entities.revision, session.player.pose_revision);
                if *version == Some(current) && !expired {
                    return Ok(None);
                }
                *version = Some(current);
                let nearest = session
                    .entities
                    .received_entities()
                    .filter(|entity| {
                        entity.type_name == *kind
                            && entity.handle.network_id != session.player.entity_id
                    })
                    .filter_map(|entity| {
                        let distance = distance_squared(entity.position, session.player.position);
                        (distance <= *radius * *radius).then_some((entity, distance))
                    })
                    .min_by(|a, b| {
                        a.1.total_cmp(&b.1)
                            .then(a.0.handle.network_id.cmp(&b.0.handle.network_id))
                    });
                if let Some((entity, _)) = nearest {
                    let mut result = entity_value(session, entity)?;
                    result["matched"] = json!(true);
                    Some(result)
                } else {
                    None
                }
            }
            Wait::Motion { identity, motion } => {
                let entity = identity
                    .lookup(session)
                    .ok_or("entity identity left client-visible state before motion matched")?;
                if entity.revision == motion.revision && !expired {
                    return Ok(None);
                }
                motion.revision = entity.revision;
                motion.horizontal = motion.horizontal.max(
                    (entity.position[0] - motion.origin[0])
                        .hypot(entity.position[2] - motion.origin[2]),
                );
                motion.lowest_y = motion.lowest_y.min(entity.position[1]);
                motion.rise = motion.rise.max(entity.position[1] - motion.lowest_y);
                motion.speed = motion
                    .speed
                    .max(entity.velocity[0].hypot(entity.velocity[2]));
                motion.yaw_delta = motion.yaw_delta.min(yaw_delta(entity));
                if motion.horizontal >= motion.minimum_distance
                    && motion.rise >= motion.minimum_rise
                {
                    let mut result = identity.value();
                    extend(
                        result.as_object_mut().unwrap(),
                        json!({"matched":true,"end_x":entity.position[0],"end_y":entity.position[1],"end_z":entity.position[2],
                        "horizontal_distance":motion.horizontal,"vertical_rise":motion.rise,"max_horizontal_speed":motion.speed,
                        "minimum_yaw_delta":motion.yaw_delta}),
                    );
                    Some(result)
                } else {
                    None
                }
            }
            Wait::Removed(identity) => {
                if identity.lookup(session).is_none() {
                    let mut result = identity.value();
                    result["removed"] = json!(true);
                    Some(result)
                } else {
                    None
                }
            }
            Wait::Health(threshold) => session
                .player
                .health
                .filter(|health| f64::from(*health) < *threshold)
                .map(|health| json!({"matched":true,"health":health,"threshold":threshold})),
            Wait::Approach { identity, movement } => {
                if let Some(entity) = identity.lookup(session) {
                    let point = entity.position;
                    let handle = entity.handle;
                    let reached = in_reach(session, handle)?;
                    if reached || expired {
                        let mut result = identity.value();
                        result["in_reach"] = json!(reached);
                        // `in_reach` is distance only; occlusion is reported separately so a
                        // caller never has to discover a blocked line by failing an interaction.
                        match session.aim_at_entity(handle, false)? {
                            None => result["targetable"] = json!(true),
                            Some(reason) => {
                                result["targetable"] = json!(false);
                                result["blocked_reason"] = json!(reason);
                            }
                        }
                        Some(result)
                    } else {
                        movement.step(session, point)?;
                        None
                    }
                } else {
                    return Err(
                        "entity identity left client-visible state while approaching".into(),
                    );
                }
            }
            Wait::Attack {
                identity,
                health: before,
                strength,
            } => {
                let entity = identity.lookup(session);
                let after = entity
                    .map(|entity| health(session, entity.handle))
                    .transpose()?
                    .flatten();
                let removed = entity.is_none();
                if removed
                    || before
                        .zip(after)
                        .is_some_and(|(before, after)| after < before)
                {
                    let mut result = identity.value();
                    let object = result.as_object_mut().unwrap();
                    extend(
                        object,
                        json!({"dispatched":true,"confirmed":true,"removed":removed,"attack_strength_before":strength}),
                    );
                    if let Some(before) = before {
                        object.insert("health_before".into(), json!(before));
                    }
                    if let Some(after) = after {
                        object.insert("health_after".into(), json!(after));
                    }
                    Some(result)
                } else {
                    None
                }
            }
            Wait::Collect {
                identity,
                item,
                count,
                before,
                started,
                removed,
                saw_drop,
                last_position,
                last_attack,
                movement,
            } => {
                if let Some(entity) = identity.lookup(session) {
                    *last_position = entity.position;
                } else {
                    *removed = true
                }
                let near = last_position.map(|value| value.floor() as i32);
                let drop = session.entities.nearest_visible_item(
                    item,
                    near,
                    content(session)?,
                    session.world_tick,
                )?;
                *saw_drop |= drop.is_some();
                let collected = inventory_count(session, item)? >= *before + *count;
                if (*removed && collected) || expired {
                    Some(collection_value(
                        session, identity, item, *count, *started, *removed, *saw_drop, collected,
                    )?)
                } else if !*removed {
                    if session.player.health.is_some_and(|health| health <= 0.) {
                        return Err("entity attack interrupted because the player died".into());
                    }
                    movement.step(session, *last_position)?;
                    if session.player.attack_strength(0.) >= 0.9
                        && *last_attack != Some(session.client_tick)
                        && distance_squared(*last_position, session.player.position) <= 9.
                    {
                        if attack(session, identity.handle.unwrap())? {
                            *started = true;
                            *last_attack = Some(session.client_tick);
                        }
                    }
                    None
                } else {
                    if let Some(point) = drop {
                        movement.step(session, point)?;
                        // Pickups require entering the drop, not stopping two blocks away.
                        session.input(InputIntent::Key {
                            key: "forward".into(),
                            pressed: true,
                        })?;
                    } else {
                        lifecycle::clear_pickup(session)?;
                    }
                    None
                }
            }
        };
        if result.is_some() {
            self.cancel(session)?;
        }
        if result.is_none() && expired {
            self.cancel(session)?;
            return Err("entity wait timed out without its received-state predicate".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;

    fn census_session() -> Session {
        let mut session = Session::new_mode("entity-census".into(), false);
        let mut content = Content::test_cube();
        let facts: Value = serde_json::from_str(include_str!(
            "../../assets/solaris/entities/entity-facts.json"
        )).unwrap();
        content.entity_names = facts["entities"].as_object().unwrap().iter()
            .map(|(name, schema)| (u32::try_from(schema["protocol_id"].as_u64().unwrap()).unwrap(), name.clone()))
            .collect();
        session.entities.activate_models(std::sync::Arc::new(
            crate::entities::Models::load_state(&content).unwrap(),
        ));
        session.content = Some(content);
        session.phase = "play".into();
        session.player.pose_known = true;
        session.player.position = [0.; 3];
        session
    }

    fn census_spawn(session: &mut Session, id: i32, x: f64) {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let content = session.content.as_ref().unwrap();
        let kind = content.entity_names.iter()
            .find(|(_, name)| *name == "minecraft:villager").unwrap().0;
        let mut packet = Writer::default();
        packet.var(id);
        packet.0.extend([u8::try_from(id).unwrap(); 16]);
        packet.var(i32::try_from(*kind).unwrap());
        packet.f64(x);
        packet.f64(0.);
        packet.f64(0.);
        packet.0.extend([0u8; 5]);
        session.entities.apply(cb::ADD_ENTITY, &mut Reader::new(&packet.0), content, 1).unwrap();
    }

    fn census_page(session: &mut Session, arguments: Value) -> Value {
        match start(session, "minecraft_list_entities", &arguments).unwrap().unwrap() {
            Outcome::Complete(value) => value,
            Outcome::Pending(_) => panic!("entity census unexpectedly waits"),
        }
    }

    #[test]
    fn entity_census_snapshot_keeps_received_rows_across_packet_and_pose_changes() {
        let mut session = census_session();
        for id in [9, 5, 7] {
            census_spawn(&mut session, id, 1.);
        }
        let first = census_page(&mut session, json!({"radius":128,"limit":1,"cursor":""}));
        assert_eq!(first["visible_count"], 3);
        assert_eq!(first["entities"][0]["entity_id"], 5);

        session.player.position[0] = 1024.;
        census_spawn(&mut session, 11, 1025.);
        let second = census_page(&mut session, json!({
            "radius":128,"limit":1,"cursor":first["next_cursor"],
        }));
        assert_eq!(second["visible_count"], 3);
        assert_eq!(second["entities"][0]["entity_id"], 7);
        let last = census_page(&mut session, json!({
            "radius":128,"limit":1,"cursor":second["next_cursor"],
        }));
        assert_eq!(last["visible_count"], 3);
        assert_eq!(last["entities"][0]["entity_id"], 9);
        assert_eq!(last["next_cursor"], Value::Null);
        assert_eq!(last["truncated"], false);

        let current = census_page(&mut session, json!({"radius":128,"limit":1,"cursor":""}));
        assert_eq!(current["visible_count"], 1);
        assert_eq!(current["entities"][0]["entity_id"], 11);
    }

    #[test]
    fn entity_census_snapshot_filters_radius_and_type_before_limits() {
        let mut session = census_session();
        for (id, x) in [(5, 127.), (7, 2.), (9, 129.)] {
            census_spawn(&mut session, id, x);
        }
        let absent = census_page(&mut session, json!({
            "radius":128,"limit":1,"cursor":"","entity_type":"minecraft:sheep",
        }));
        assert_eq!(absent["visible_count"], 0);
        assert_eq!(absent["entities"], json!([]));
        assert_eq!(absent["next_cursor"], Value::Null);

        let first = census_page(&mut session, json!({
            "radius":128,"limit":1,"cursor":"","entity_type":"minecraft:villager",
        }));
        assert_eq!(first["visible_count"], 2);
        assert_eq!(first["entities"][0]["entity_id"], 5);
        let last = census_page(&mut session, json!({
            "radius":128,"limit":1,"cursor":first["next_cursor"],"entity_type":"minecraft:villager",
        }));
        assert_eq!(last["entities"][0]["entity_id"], 7);
        assert_eq!(last["next_cursor"], Value::Null);
        assert_eq!(last["truncated"], false);
    }

    #[test]
    fn entity_census_snapshot_fences_query_context_replay_and_replacement() {
        let mut session = census_session();
        for id in [5, 7, 9] {
            census_spawn(&mut session, id, 1.);
        }
        let first = census_page(&mut session, json!({"radius":128,"limit":1,"cursor":""}));
        let cursor = first["next_cursor"].as_str().unwrap();
        assert!(start(&mut session, "minecraft_list_entities", &json!({
            "radius":64,"limit":1,"cursor":cursor,
        })).is_err());
        assert!(start(&mut session, "minecraft_list_entities", &json!({
            "radius":128,"limit":1,"cursor":cursor,"entity_type":"minecraft:villager",
        })).is_err());
        assert!(session.entity_census.page((0, 1), 128., None, cursor, 1).is_err());

        let replacement = census_page(&mut session, json!({"radius":128,"limit":1,"cursor":""}));
        assert!(start(&mut session, "minecraft_list_entities", &json!({
            "radius":128,"limit":1,"cursor":cursor,
        })).is_err());
        let current = replacement["next_cursor"].as_str().unwrap();
        let second = census_page(&mut session, json!({"radius":128,"limit":1,"cursor":current}));
        assert_eq!(second["entities"][0]["entity_id"], 7);
        assert!(start(&mut session, "minecraft_list_entities", &json!({
            "radius":128,"limit":1,"cursor":current,
        })).is_err());
        session.disconnect();
        assert!(session.entity_census.page((0, 0), 128., None,
            second["next_cursor"].as_str().unwrap(), 1).is_err());
    }

    #[test]
    fn entity_census_filters_before_paging_and_preserves_distant_residents() {
        let entities = [
            (1., 1, "minecraft:item"),
            (100., 9, "minecraft:villager"),
            (4., 3, "minecraft:sheep"),
            (9., 7, "minecraft:villager"),
            (16000., 5, "minecraft:villager"),
            (17000., 11, "minecraft:villager"),
        ];
        let (page, total) = entity_page(
            entities.into_iter(), 128., 3, Some("minecraft:villager"),
        );
        assert_eq!(page.iter().map(|row| row.1).collect::<Vec<_>>(), [7, 9, 5]);
        assert_eq!(total, 3);
    }

    #[test]
    fn entity_census_unpaged_calls_preserve_nearest_order() {
        let entities = [(1., 9, "minecraft:villager"), (100., 5, "minecraft:villager"),
                        (81., 7, "minecraft:villager")];
        let (nearest, _) = entity_page(entities.into_iter(), 128., 2, None);
        assert_eq!(nearest.iter().map(|row| row.1).collect::<Vec<_>>(), [9, 7]);
    }

    #[test]
    fn received_baby_distinguishes_absent_and_false() {
        assert_eq!(received_baby(None, None).unwrap(), ("unknown_schema", None));
        assert_eq!(
            received_baby(Some(false), None).unwrap(),
            ("unsupported", None)
        );
        assert_eq!(received_baby(Some(true), None).unwrap(), ("missing", None));
        assert_eq!(
            received_baby(Some(true), Some(&Metadata::Bool(false))).unwrap(),
            ("received", Some(false))
        );
        assert_eq!(
            received_baby(Some(true), Some(&Metadata::Bool(true))).unwrap(),
            ("received", Some(true))
        );
        assert!(received_baby(Some(true), Some(&Metadata::Float(1.0))).is_err());
    }
}
