//! The entities one frame draws, gathered once (`Store::frame_entities`): the candidates of the
//! body pass's area in `Store::candidates` order, each with its presented transform, camera
//! distance and vanilla draw-distance verdict. The body, shadow and nameplate passes each read
//! the part their own range covers, instead of each walking the spatial index and resolving
//! every transform (a mounted body's attachment chain included) again.

use super::{Store, Transform, physical::Place, resources::Models, view_range};
use crate::player::Bounds;

/// One candidate seen from the frame's camera.
#[derive(Debug)]
pub(crate) struct Entry {
    pub slot: usize,
    /// Where `Store::candidate_slots` found it (a body's cell, a mounted body's root cell).
    place: Place,
    pub transform: Result<Transform, String>,
    /// Squared camera distance of the presented position (infinite when it failed).
    pub distance_squared: f64,
    /// `view_range::within` at the presented position, resolved within the gathered range
    /// (false beyond it, where every pass skips the entity by distance first).
    pub within: bool,
}

impl Entry {
    #[expect(clippy::too_many_arguments, reason = "One candidate's view inputs")]
    pub(crate) fn of(
        store: &Store,
        models: &Models,
        slot: usize,
        place: Place,
        camera: [f64; 3],
        range: f64,
        tick: u64,
        partial: f32,
    ) -> Self {
        let entity = &store.slots[slot];
        let transform = store.presented_transform(entity, tick, partial);
        let distance_squared = transform.as_ref().map_or(f64::INFINITY, |transform| {
            transform
                .position
                .iter()
                .zip(camera)
                .map(|(a, b)| (a - b) * (a - b))
                .sum::<f64>()
        });
        let within = transform.as_ref().is_ok_and(|transform| {
            distance_squared <= range * range
                && view_range::within(
                    models,
                    entity,
                    transform.position,
                    distance_squared,
                    store.view_scale.0,
                )
        });
        Self {
            slot,
            place,
            transform,
            distance_squared,
            within,
        }
    }
}

/// What a gathered frame was gathered for; a pass reuses it only for the same camera, moment,
/// view scale and index padding, and a range it covers.
#[derive(Clone, Copy, Debug)]
struct Key {
    camera: [f64; 3],
    tick: u64,
    partial: f32,
    view_scale: f64,
    padding: f64,
    range: f64,
}

#[derive(Debug, Default)]
pub(crate) struct FrameEntities {
    key: Option<Key>,
    pub(crate) entries: Vec<Entry>,
}

impl FrameEntities {
    /// Gathers the candidates within `range` of `camera`.
    pub(crate) fn gather(
        &mut self,
        store: &Store,
        models: &Models,
        camera: [f64; 3],
        range: f64,
        tick: u64,
        partial: f32,
    ) {
        self.entries.clear();
        self.entries.extend(
            store
                .candidate_slots(area(camera, range))
                .map(|(place, slot)| {
                    Entry::of(store, models, slot, place, camera, range, tick, partial)
                }),
        );
        self.key = Some(Key {
            camera,
            tick,
            partial,
            view_scale: store.view_scale.0,
            padding: store.query_padding(),
            range,
        });
    }
    /// Forgets the gathered key: the next pass gathers again.
    pub(crate) fn end(&mut self) {
        self.key = None;
    }
    fn covers(&self, store: &Store, camera: [f64; 3], range: f64, tick: u64, partial: f32) -> bool {
        self.key.is_some_and(|key| {
            key.camera.map(f64::to_bits) == camera.map(f64::to_bits)
                && key.tick == tick
                && key.partial.to_bits() == partial.to_bits()
                && key.view_scale.to_bits() == store.view_scale.0.to_bits()
                && key.padding.to_bits() == store.query_padding().to_bits()
                && range <= key.range
        })
    }
    /// The entries `Store::candidates` yields for `range` around `camera`, in its order: the
    /// bodies in the cells it visits, then the mounted bodies whose root vehicle it visits.
    fn within_range<'a>(
        &'a self,
        store: &Store,
        camera: [f64; 3],
        range: f64,
    ) -> impl Iterator<Item = &'a Entry> + 'a {
        let cells = store.query_cells(area(camera, range));
        self.entries
            .iter()
            .filter(move |entry| entry.place.within(&cells))
    }
}

fn area(camera: [f64; 3], range: f64) -> Bounds {
    Bounds {
        min: camera.map(|value| value - range),
        max: camera.map(|value| value + range),
    }
}

impl Store {
    /// The frame entities a pass of `range` around `camera` draws from: those the body pass
    /// gathered for this camera and moment when they cover `range`, else gathered into `own`.
    #[expect(clippy::too_many_arguments, reason = "One pass's view inputs")]
    pub(crate) fn frame_entities<'a>(
        &'a self,
        own: &'a mut FrameEntities,
        models: &Models,
        camera: [f64; 3],
        range: f64,
        tick: u64,
        partial: f32,
    ) -> impl Iterator<Item = &'a Entry> + 'a {
        let frame = if self
            .frame_entities
            .covers(self, camera, range, tick, partial)
        {
            &self.frame_entities
        } else {
            own.gather(self, models, camera, range, tick, partial);
            &*own
        };
        frame.within_range(self, camera, range)
    }
    /// Ends the frame: later passes gather their own entities until the next body pass.
    pub(crate) fn end_entity_frame(&mut self) {
        self.frame_entities.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
    use std::sync::Arc;

    /// Husks and horses on a grid 300 blocks wide around the origin, every third husk riding
    /// the horse beside it.
    fn battle_store() -> (Store, Arc<Models>) {
        let (content, models) = super::super::resources::every_kind();
        let models = Arc::new(models);
        let mut store = Store::default();
        store.activate_models(Arc::clone(&models));
        let mut id = 0;
        for x in (-150..=150).step_by(10) {
            for z in (-150..=150).step_by(10) {
                for (kind, dx) in [("minecraft:husk", 0.), ("minecraft:horse", 2.5)] {
                    id += 1;
                    let mut spawn = Writer::default();
                    spawn.var(id);
                    spawn.0.extend_from_slice(&[0; 16]);
                    spawn.var(models.schemas.get(kind).unwrap().protocol_id as i32);
                    for coordinate in [f64::from(x) + dx, 64., f64::from(z) + 0.5] {
                        spawn.0.extend_from_slice(&coordinate.to_be_bytes());
                    }
                    spawn.0.extend_from_slice(&[0; 4]);
                    spawn.var(0);
                    store
                        .apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1)
                        .unwrap();
                }
                if id % 3 == 0 {
                    let mut mount = Writer::default();
                    mount.var(id);
                    mount.var(1);
                    mount.var(id - 1);
                    store
                        .apply(cb::SET_PASSENGERS, &mut Reader::new(&mount.0), &content, 1)
                        .unwrap();
                }
            }
        }
        (store, models)
    }

    /// A pass's part of the gathered frame is exactly `Store::candidates` of its own range:
    /// same entities, same order, same transforms, distances and draw-distance verdicts.
    #[test]
    fn frame_parts_match_each_range_candidates() {
        let (mut store, models) = battle_store();
        let camera = [3., 64., -5.];
        let (tick, partial) = (1, 0.25);
        let mut frame = FrameEntities::default();
        frame.gather(&store, &models, camera, 128., tick, partial);
        assert!(
            frame
                .entries
                .iter()
                .any(|entry| matches!(entry.place, Place::Seat(Some(_)))),
            "riders are gathered"
        );
        for range in [16., 40., 64., 128.] {
            let expected: Vec<_> = store
                .candidates(area(camera, range))
                .map(|entity| {
                    let transform = store.presented_transform(entity, tick, partial).unwrap();
                    let distance = transform
                        .position
                        .iter()
                        .zip(camera)
                        .map(|(a, b)| (a - b) * (a - b))
                        .sum::<f64>();
                    let within = distance <= range * range
                        && view_range::within(
                            &models,
                            entity,
                            transform.position,
                            distance,
                            store.view_scale.0,
                        );
                    (entity.handle, transform.position, distance, within)
                })
                .collect();
            assert!(
                expected.len() > 10,
                "range {range}: {} candidates",
                expected.len()
            );
            let mut own = FrameEntities::default();
            store.frame_entities = std::mem::take(&mut frame);
            let got: Vec<_> = store
                .frame_entities(&mut own, &models, camera, range, tick, partial)
                .map(|entry| {
                    let transform = entry.transform.as_ref().unwrap();
                    let within = entry.distance_squared <= range * range && entry.within;
                    (
                        store.slots[entry.slot].handle,
                        transform.position,
                        entry.distance_squared,
                        within,
                    )
                })
                .collect();
            assert!(
                own.entries.is_empty(),
                "range {range} reads the gathered frame"
            );
            frame = std::mem::take(&mut store.frame_entities);
            assert_eq!(got, expected, "range {range}");
        }
        // Another moment or a wider range is not covered: the pass gathers its own.
        store.frame_entities = frame;
        for (range, tick) in [(64., tick + 1), (200., tick)] {
            let mut own = FrameEntities::default();
            let count = store
                .frame_entities(&mut own, &models, camera, range, tick, partial)
                .count();
            assert_eq!(count, store.candidates(area(camera, range)).count());
            assert!(!own.entries.is_empty());
        }
        store.end_entity_frame();
        let mut own = FrameEntities::default();
        let _ = store
            .frame_entities(&mut own, &models, camera, 16., 1, partial)
            .count();
        assert!(!own.entries.is_empty(), "an ended frame is not reused");
    }

    /// A small query (a crosshair ray, a push, a shadow range) yields the mounted bodies seated
    /// near it, not all of them: every rider whose presented box can reach the area is a
    /// candidate, riders seated far away are not.
    #[test]
    fn riders_are_candidates_near_their_root_vehicle_only() {
        let (store, _) = battle_store();
        let riders: Vec<_> = store.passenger_vehicles.keys().copied().collect();
        assert!(riders.len() > 100, "{} riders", riders.len());
        let seated = |id: i32| {
            store
                .presented_transform(store.get(id).unwrap(), 1, 0.)
                .unwrap()
                .position
        };
        let camera = seated(riders[riders.len() / 2]);
        let range = 4.;
        let found: std::collections::BTreeSet<_> = store
            .candidates(area(camera, range))
            .map(|entity| entity.handle.network_id)
            .collect();
        let mut near = 0;
        for id in riders {
            let position = seated(id);
            let reach = (0..3)
                .step_by(2)
                .map(|i| (position[i] - camera[i]).abs())
                .fold(0., f64::max);
            if reach <= range + 1. {
                near += 1;
                assert!(
                    found.contains(&id),
                    "rider {id} at {position:?} reaches the area"
                );
            } else if reach > range + 4. * store.query_padding() + 32. {
                assert!(
                    !found.contains(&id),
                    "rider {id} at {position:?} is seated far away"
                );
            }
        }
        assert!(near > 0, "a rider sits within the area");
    }
}
