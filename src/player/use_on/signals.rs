use super::*;

fn shifted(position: [i32; 3], direction: u8) -> [i32; 3] {
    let delta = match direction {
        0 => [0, -1, 0],
        1 => [0, 1, 0],
        2 => [0, 0, -1],
        3 => [0, 0, 1],
        4 => [-1, 0, 0],
        5 => [1, 0, 0],
        _ => unreachable!("source direction"),
    };
    std::array::from_fn(|axis| position[axis] + delta[axis])
}
fn opposite(direction: u8) -> u8 {
    direction ^ 1
}
fn direction(value: &str) -> Result<u8> {
    match value {
        "down" => Ok(0),
        "up" => Ok(1),
        "north" => Ok(2),
        "south" => Ok(3),
        "west" => Ok(4),
        "east" => Ok(5),
        _ => Err(format!("invalid source direction {value}")),
    }
}
fn direction_name(value: u8) -> &'static str {
    ["down", "up", "north", "south", "west", "east"][usize::from(value)]
}

pub(super) struct SignalQuery {
    pub position: [i32; 3],
    pub state: u32,
    pub face: u8,
    pub direct: bool,
}

impl Player {
    pub(super) fn neighbor_signal(
        &self,
        position: [i32; 3],
        world: &World,
        content: &Content,
        entities: &EntityStore,
    ) -> Result<bool> {
        for face in 0..6 {
            let neighbor = shifted(position, face);
            let state = self.use_block(world, neighbor)?;
            if self.block_has_signal(SignalQuery { position: neighbor, state, face, direct: false }, world, content, entities)? {
                return Ok(true);
            }
            if content.block_redstone_conductor(state)? {
                for direct_face in 0..6 {
                    let direct_neighbor = shifted(neighbor, direct_face);
                    let direct_state = self.use_block(world, direct_neighbor)?;
                    if self.block_has_signal(
                        SignalQuery { position: direct_neighbor, state: direct_state, face: direct_face, direct: true },
                        world,
                        content,
                        entities,
                    )? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub(super) fn block_has_signal(
        &self,
        query: SignalQuery,
        world: &World,
        content: &Content,
        entities: &EntityStore,
    ) -> Result<bool> {
        let SignalQuery { position, state, face, direct } = query;
        let owners = content.block_signal_owners(state)?;
        let owner = short_class(if direct { owners.1 } else { owners.0 });
        let facing = || direction(property(content, state, "facing")?);
        let powered = || boolean(content, state, "powered");
        let connected_direction = || match property(content, state, "face")? {
            "floor" => Ok(1),
            "ceiling" => Ok(0),
            "wall" => facing(),
            _ => Err("invalid source attached face".into()),
        };
        Ok(match owner {
            "BlockBehaviour" => false,
            "PoweredBlock" => true,
            "BasePressurePlateBlock" => {
                if !direct || face == 1 {
                    if content
                        .state_properties(state)
                        .ok_or("missing pressure plate state")?
                        .contains_key("power")
                    {
                        number(content, state, "power")? > 0
                    } else {
                        powered()?
                    }
                } else {
                    false
                }
            }
            "ButtonBlock" | "LeverBlock" => {
                powered()? && (!direct || face == connected_direction()?)
            }
            "DaylightDetectorBlock" | "TargetBlock" => number(content, state, "power")? > 0,
            "CalibratedSculkSensorBlock" => {
                face != facing()? && number(content, state, "power")? > 0
            }
            "SculkSensorBlock" => {
                (!direct || face == 1)
                    && !(short_class(owners.0) == "CalibratedSculkSensorBlock" && face == facing()?)
                    && number(content, state, "power")? > 0
            }
            "DetectorRailBlock" | "LecternBlock" => powered()? && (!direct || face == 1),
            "LightningRodBlock" | "TripWireHookBlock" => {
                powered()? && (!direct || face == facing()?)
            }
            "ObserverBlock" | "DiodeBlock" => powered()? && face == facing()?,
            "RedstoneTorchBlock" => {
                let weak_face = if direct { 0 } else { face };
                let enabled = boolean(content, state, "lit")?
                    && if short_class(owners.0) == "RedstoneWallTorchBlock" {
                        weak_face != facing()?
                    } else {
                        weak_face != 1
                    };
                enabled && (!direct || face == 0)
            }
            "RedstoneWallTorchBlock" => boolean(content, state, "lit")? && face != facing()?,
            "JukeboxBlock" => entities.jukebox_playing(position).unwrap_or(false),
            "TrappedChestBlock" => {
                (!direct || face == 1) && entities.block_open_count(position).unwrap_or(0) > 0
            }
            "TestBlock" => {
                property(content, state, "mode")? == "start"
                    && entities.test_block_powered(position).unwrap_or(false)
            }
            "RedstoneWireBlock" => {
                let power = number(content, state, "power")?;
                face != 0
                    && power > 0
                    && (face == 1
                        || self.wire_connections(position, state, world, content)?
                            [usize::from(opposite(face) - 2)])
            }
            owner => return Err(format!("unimplemented verified signal owner {owner}")),
        })
    }

    fn wire_connections(
        &self,
        position: [i32; 3],
        state: u32,
        world: &World,
        content: &Content,
    ) -> Result<[bool; 4]> {
        let connected = self.wire_connection_state(position, state, world, content)?;
        let mut result = [false; 4];
        for face in 2..6 {
            result[usize::from(face - 2)] =
                property(content, connected, direction_name(face))? != "none";
        }
        Ok(result)
    }

    pub(super) fn wire_connection_state(
        &self,
        position: [i32; 3],
        state: u32,
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let was_dot = (2..6).try_fold(true, |dot, face| -> Result<bool> {
            Ok(dot && property(content, state, direction_name(face))? == "none")
        })?;
        let can_connect_up =
            !content.block_redstone_conductor(self.use_block(world, shifted(position, 1))?)?;
        let mut connections = ["none"; 4];
        for face in 2..6 {
            let neighbor = shifted(position, face);
            let relative_state = self.use_block(world, neighbor)?;
            let placeable_above = short_class(content.block_use_owners(relative_state)?.1)
                == "TrapDoorBlock"
                || content.info(relative_state)?.sturdy_faces[1]
                || content.name(relative_state) == Some("minecraft:hopper");
            let up_connected = can_connect_up
                && placeable_above
                && self.wire_connects_to(
                    self.use_block(world, shifted(neighbor, 1))?,
                    None,
                    content,
                )?;
            connections[usize::from(face - 2)] = if up_connected {
                if content.info(relative_state)?.sturdy_faces[usize::from(opposite(face))] {
                    "up"
                } else {
                    "side"
                }
            } else if self.wire_connects_to(relative_state, Some(face), content)?
                || !content.block_redstone_conductor(relative_state)?
                    && self.wire_connects_to(
                        self.use_block(world, shifted(neighbor, 0))?,
                        None,
                        content,
                    )?
            {
                "side"
            } else {
                "none"
            };
        }
        if !was_dot || connections.iter().any(|connection| *connection != "none") {
            let north_south_empty = connections[0] == "none" && connections[1] == "none";
            let east_west_empty = connections[2] == "none" && connections[3] == "none";
            for (index, connection) in connections.iter_mut().enumerate() {
                if *connection == "none"
                    && if index < 2 {
                        east_west_empty
                    } else {
                        north_south_empty
                    }
                {
                    *connection = "side";
                }
            }
        }
        let mut result = content.block_default_state(state)?;
        result =
            content.with_state_property(result, "power", property(content, state, "power")?)?;
        for face in 2..6 {
            result = content.with_state_property(
                result,
                direction_name(face),
                connections[usize::from(face - 2)],
            )?;
        }
        Ok(result)
    }

    pub(super) fn toggled_wire_state(
        &self,
        position: [i32; 3],
        state: u32,
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let dot = (2..6).try_fold(true, |dot, face| -> Result<bool> {
            Ok(dot && property(content, state, direction_name(face))? == "none")
        })?;
        let mut seed = content.block_default_state(state)?;
        seed = content.with_state_property(seed, "power", property(content, state, "power")?)?;
        for face in 2..6 {
            seed = content.with_state_property(
                seed,
                direction_name(face),
                if dot { "side" } else { "none" },
            )?;
        }
        self.wire_connection_state(position, seed, world, content)
    }


    pub(super) fn wire_connects_to(
        &self,
        state: u32,
        face: Option<u8>,
        content: &Content,
    ) -> Result<bool> {
        Ok(
            match content.name(state).ok_or("unknown wire neighbor state")? {
                "minecraft:redstone_wire" => true,
                "minecraft:repeater" => {
                    let facing = direction(property(content, state, "facing")?)?;
                    face == Some(facing) || face == Some(opposite(facing))
                }
                "minecraft:observer" => {
                    face == Some(direction(property(content, state, "facing")?)?)
                }
                _ => face.is_some() && content.block_is_signal_source(state)?,
            },
        )
    }
}
