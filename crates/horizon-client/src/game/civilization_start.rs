//! Server-owned race selection. Generation queries run off-thread; arrival uses loaded blocks.
use super::{Dim, Game, civilization_guide as guide, commands::Target};
use horizon_client::{streaming::ChunkGenerator, text::Text, vanillagen::VanillaTerrain};
use horizon_core::position::ChunkPos;
use horizon_mods::civilization::{
    Bell,
    races::{Race, Start},
};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};

pub(super) struct Search {
    race: Race,
    receiver: Option<Receiver<Result<Start, String>>>,
    result: Option<Result<Start, String>>,
}
impl Search {
    pub(super) fn phase(&self) -> &'static str {
        if self.result.is_some() {
            "ready"
        } else {
            "searching"
        }
    }
}

fn find_start(
    terrain: Arc<VanillaTerrain>,
    race: Race,
    from: [i32; 3],
    occupied: Vec<Bell>,
) -> Result<Start, String> {
    let mut worker = terrain.worker();
    let allowed = |name: &str| race.habitat(name);
    let (_, at) = worker
        .find_closest_biome(&horizon_world::worldgen::generator::BiomeSearch {
            allowed: &allowed,
            origin: [from[0], 64, from[2]],
            radius: 6400,
            horizontal: 32,
            vertical: 64,
            min_y: 64,
            max_y: 96,
        })
        .ok_or("horizon.civilization.race.not_found")?;
    let mut decorated_sites = 0;
    for (dx, dz) in (0_i32..=16).flat_map(|r| {
        (-r..=r).flat_map(move |x| {
            (-r..=r).filter_map(move |z| (x.abs().max(z.abs()) == r).then_some((x * 32, z * 32)))
        })
    }) {
        let [x, z] = [at[0] + dx, at[2] + dz];
        if x.unsigned_abs() > 29_999_800 || z.unsigned_abs() > 29_999_800 {
            continue;
        }
        if occupied.iter().any(|b| {
            (i64::from(x) - i64::from(b.x)).pow(2) + (i64::from(z) - i64::from(b.z)).pow(2)
                <= 64 * 64
        }) {
            continue;
        }
        let Some(surface) = worker.surface_sample(x, z) else {
            continue;
        };
        let y = surface.height;
        if !(64..=287).contains(&y)
            || [-10, 0, 10].into_iter().any(|dx| {
                [-10, 0, 10].into_iter().any(|dz| {
                    worker
                        .surface_sample(x + dx, z + dz)
                        .is_none_or(|s| s.water.is_some() || (s.height - y).abs() > 1)
                })
            })
        {
            continue;
        }
        let mut chunks = std::collections::BTreeMap::new();
        for cx in (x - 10) >> 4..=(x + 10) >> 4 {
            for cz in (z - 10) >> 4..=(z + 10) >> 4 {
                chunks.insert(
                    (cx, cz),
                    worker
                        .generate_proto(ChunkPos::new(cx, cz))
                        .map_err(|_| "horizon.civilization.race.not_found")?,
                );
            }
        }
        let biome = chunks[&(x >> 4, z >> 4)].biome_at(x, y, z).to_owned();
        if !race.habitat(&biome) {
            continue;
        }
        let clear = (-10..=10).all(|dx| {
            (-10..=10).all(|dz| {
                let (px, pz) = (x + dx, z + dz);
                let chunk = &chunks[&(px >> 4, pz >> 4)];
                (chunk.surface_height(px, pz) - y).abs() <= 1
                    && race.habitat(chunk.biome_at(px, y, pz))
                    && (-2..=4).all(|dy| {
                        horizon_mods::civilization::settlements::natural(chunk.block_name(
                            px,
                            y + dy,
                            pz,
                        ))
                    })
            })
        });
        if !clear {
            continue;
        }
        if decorated_sites >= 24 {
            break;
        }
        decorated_sites += 1;
        // Features can add a lava lake, cactus or a structure after TERRAIN. Decorate
        // the footprint and its neighbors off-thread before accepting a camp site.
        let (min_x, max_x, min_z, max_z) =
            ((x - 10) >> 4, (x + 10) >> 4, (z - 10) >> 4, (z + 10) >> 4);
        for cx in min_x - 2..=max_x + 2 {
            for cz in min_z - 2..=max_z + 2 {
                if let std::collections::btree_map::Entry::Vacant(e) = chunks.entry((cx, cz)) {
                    e.insert(
                        worker
                            .generate_proto(ChunkPos::new(cx, cz))
                            .map_err(|_| "horizon.civilization.race.not_found")?,
                    );
                }
            }
        }
        for cz in min_z - 1..=max_z + 1 {
            for cx in min_x - 1..=max_x + 1 {
                let keys: Vec<_> = (-1..=1)
                    .flat_map(|dz| (-1..=1).map(move |dx| (cx + dx, cz + dz)))
                    .collect();
                let region: Vec<_> = keys.iter().map(|key| chunks.remove(key).unwrap()).collect();
                let mut region: [_; 9] = region.try_into().ok().unwrap();
                worker
                    .decorate(&mut region)
                    .map_err(|_| "horizon.civilization.race.not_found")?;
                for (key, chunk) in keys.into_iter().zip(region) {
                    chunks.insert(key, chunk);
                }
            }
        }
        if !(-10..=10).all(|dx| {
            (-10..=10).all(|dz| {
                let (px, pz) = (x + dx, z + dz);
                let chunk = &chunks[&(px >> 4, pz >> 4)];
                (-1..=4).contains(&(chunk.surface_height(px, pz) - y))
                    && (-2..=4).all(|dy| {
                        horizon_mods::civilization::settlements::natural(chunk.block_name(
                            px,
                            y + dy,
                            pz,
                        ))
                    })
            })
        }) {
            continue;
        }
        return Ok(Start {
            settlement: Bell {
                dimension: "minecraft:overworld".into(),
                x,
                y,
                z,
            },
            structure: "civilization:settlement".into(),
            position: [x, y, z - 2],
            biome,
        });
    }
    Err("horizon.civilization.race.not_found".into())
}

impl Game<'_> {
    pub(super) fn civ_request_race(&mut self, race: Race) -> Result<String, String> {
        let player = self.civ_player_id();
        if self.dim != Dim::Overworld
            || self.spectator()
            || self
                .player
                .as_ref()
                .is_none_or(|p| p.health.health() <= 0.0)
        {
            return Err("horizon.civilization.race.overworld".into());
        }
        if self
            .civilization
            .registry
            .race_choice(&player)
            .is_some_and(|c| {
                c.race != race
                    || c.settlement_id.as_ref().is_some_and(|id| {
                        self.civilization.registry.settlement(id).is_none_or(|t| {
                            t.status != horizon_mods::civilization::settlements::Status::Blocked
                        })
                    })
            })
        {
            return Err("horizon.civilization.race.locked".into());
        }
        if self.civilization.searches.contains_key(&player) {
            return Ok("horizon.civilization.race.searching".into());
        }
        if self
            .civilization
            .searches
            .values()
            .filter(|s| s.receiver.is_some())
            .count()
            >= 4
        {
            return Err("horizon.civilization.race.busy".into());
        }
        if !self.civ_compass_room() {
            return Err("horizon.civilization.book.inventory_full".into());
        }
        let terrain = self
            .end
            .locator()
            .ok_or("horizon.civilization.race.generator_required")?;
        let p = self
            .player
            .as_ref()
            .ok_or("horizon.civilization.race.overworld")?
            .position;
        let from = [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32];
        let existing = self
            .civilization
            .registry
            .settlements()
            .find(|t| {
                t.race == Some(race)
                    && t.claimed_by.is_none()
                    && matches!(
                        t.status,
                        horizon_mods::civilization::settlements::Status::Ready
                            | horizon_mods::civilization::settlements::Status::Building
                    )
                    && (i64::from(t.bell.x) - i64::from(from[0])).pow(2)
                        + (i64::from(t.bell.z) - i64::from(from[2])).pow(2)
                        <= 6400 * 6400
            })
            .cloned();
        if let Some(town) = existing {
            self.civilization.searches.insert(
                player,
                Search {
                    race,
                    receiver: None,
                    result: Some(Ok(Start {
                        position: [town.bell.x, town.bell.y, town.bell.z - 2],
                        settlement: town.bell,
                        biome: town.biome,
                        structure: "civilization:settlement".into(),
                    })),
                },
            );
            return Ok("horizon.civilization.race.searching".into());
        }
        let occupied = self
            .civilization
            .registry
            .settlements()
            .map(|t| t.bell.clone())
            .collect();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("Civilization start search".into())
            .spawn(move || {
                let _ = tx.send(find_start(terrain, race, from, occupied));
            })
            .map_err(|_| "horizon.civilization.race.busy")?;
        self.civilization.searches.insert(
            player.clone(),
            Search {
                race,
                receiver: Some(rx),
                result: None,
            },
        );
        self.civilization.start_errors.remove(&player);
        Ok("horizon.civilization.race.searching".into())
    }

    pub(super) fn civ_poll_race_searches(&mut self) {
        for search in self.civilization.searches.values_mut() {
            let Some(rx) = &search.receiver else {
                continue;
            };
            let result = match rx.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => continue,
                Err(TryRecvError::Disconnected) => {
                    Err("horizon.civilization.race.not_found".into())
                }
            };
            search.result = Some(result);
            search.receiver = None;
        }
    }

    fn civ_compass_room(&self) -> bool {
        self.inventory
            .iter()
            .chain(std::iter::once(&self.offhand))
            .flatten()
            .any(|s| {
                s.item.as_ref() == "minecraft:compass" && guide::owned(s, "horizon_civ_compass")
            })
            || self.inventory.iter().take(36).any(Option::is_none)
    }

    pub(super) fn civ_recover_compass(&mut self) -> Result<String, String> {
        let player = self.civ_player_id();
        let compass = guide::compass(self.civilization.registry.compass_target(&player));
        if !self
            .inventory
            .iter()
            .chain(std::iter::once(&self.offhand))
            .flatten()
            .any(|s| {
                s.item.as_ref() == "minecraft:compass" && guide::owned(s, "horizon_civ_compass")
            })
        {
            let slot = self
                .inventory
                .iter()
                .take(36)
                .position(Option::is_none)
                .ok_or("horizon.civilization.book.inventory_full")?;
            self.inventory[slot] = Some(compass);
        }
        self.civ_refresh_guide();
        Ok("horizon.civilization.compass.ready".into())
    }

    pub(super) fn civ_refresh_guide(&mut self) {
        let player = self.civ_player_id();
        let choice = self.civilization.registry.race_choice(&player);
        let book = guide::guide_book(choice);
        let compass = guide::compass(self.civilization.registry.compass_target(&player));
        for stack in self
            .inventory
            .iter_mut()
            .chain(std::iter::once(&mut self.offhand))
            .flatten()
        {
            if guide::is_guide(stack) {
                if *stack != book {
                    *stack = book.clone();
                }
            } else if stack.item.as_ref() == "minecraft:compass"
                && guide::owned(stack, "horizon_civ_compass")
                && *stack != compass
            {
                *stack = compass.clone();
            }
        }
    }

    pub(super) fn civ_recover_guide(&mut self) -> Result<String, String> {
        if !self
            .inventory
            .iter()
            .chain(std::iter::once(&self.offhand))
            .flatten()
            .any(guide::is_guide)
        {
            let player = self.civ_player_id();
            let book = guide::guide_book(self.civilization.registry.race_choice(&player));
            let slot = self
                .inventory
                .iter()
                .take(36)
                .position(Option::is_none)
                .ok_or("horizon.civilization.book.inventory_full")?;
            self.inventory[slot] = Some(book);
        }
        self.civ_refresh_guide();
        Ok("horizon.civilization.book.ready".into())
    }

    pub(super) fn civ_tick_start(&mut self) {
        let player = self.civ_player_id();
        if self.dim == Dim::Overworld
            && self
                .player
                .as_ref()
                .is_some_and(|p| p.health.health() > 0.0)
            && self
                .civilization
                .searches
                .get(&player)
                .is_some_and(|s| s.result.is_some())
        {
            let search = self.civilization.searches.remove(&player).unwrap();
            let result: Result<String, String> = search.result.unwrap().and_then(|start| {
                let [sx, _, sz] = start.position;
                if !self
                    .border
                    .contains(f64::from(sx) + 0.1, f64::from(sz) + 0.1)
                    || !self
                        .border
                        .contains(f64::from(sx) + 0.9, f64::from(sz) + 0.9)
                    || !self.border.contains(
                        f64::from(start.settlement.x) + 0.5,
                        f64::from(start.settlement.z) + 0.5,
                    )
                {
                    return Err("horizon.civilization.race.invalid_start".into());
                }
                if !self.civ_compass_room() {
                    return Err("horizon.civilization.book.inventory_full".into());
                }
                let old = self.civilization.registry.clone();
                let inventory = self.inventory.clone();
                let offhand = self.offhand.clone();
                self.civilization
                    .registry
                    .choose_race(&player, search.race, start)?;
                let saved = self
                    .civ_recover_compass()
                    .and_then(|_| self.save_civilization());
                if let Err(error) = saved {
                    self.civilization.registry = old;
                    self.inventory = inventory;
                    self.offhand = offhand;
                    log::warn!("Civilization race selection rolled back: {error}");
                    return Err("horizon.civilization.error.save_failed".into());
                }
                Ok("horizon.civilization.race.accepted".into())
            });
            match result {
                Ok(key) => self.system_message(Text::tr(&key, &[])),
                Err(key) => {
                    self.civilization
                        .start_errors
                        .insert(player.clone(), key.clone());
                    self.system_message(Text::tr(&key, &[]).with_color(horizon_client::text::RED));
                    log::debug!("Civilization start rejected for {player}: {key}");
                }
            }
        }
        self.civ_refresh_guide();
        let choice = self.civilization.registry.race_choice(&player).cloned();
        let Some(choice) = choice else {
            return;
        };
        if choice
            .settlement_id
            .as_ref()
            .and_then(|id| self.civilization.registry.settlement(id))
            .is_some_and(|t| t.status == horizon_mods::civilization::settlements::Status::Blocked)
            && !self.civilization.start_errors.contains_key(&player)
        {
            let key = "horizon.civilization.settlement.blocked";
            self.civilization
                .start_errors
                .insert(player.clone(), key.into());
            self.system_message(Text::tr(key, &[]).with_color(horizon_client::text::RED));
        }
        if choice.settlement_id.is_none() && !self.civilization.searches.contains_key(&player) {
            let _ = self.civ_request_race(choice.race);
        }
        let Some(p) = &self.player else {
            return;
        };
        if p.health.health() <= 0.0 {
            return;
        }
        let at = p.position;
        let biome = self.biome_name_at(at).unwrap_or_default();
        let ground = self.res.facts().get(self.world.block(
            at.x.floor() as i32,
            at.y.floor() as i32 - 1,
            at.z.floor() as i32,
        ));
        let bonus = choice.race.movement_bonus(
            &biome,
            matches!(
                ground.name.as_ref(),
                "minecraft:sand" | "minecraft:red_sand"
            ),
        );
        if let Some(p) = &mut self.player {
            let _ = p.set_modifier(
                "minecraft:movement_speed",
                "civilization:race",
                bonus,
                "add_multiplied_base",
                bonus != 0.0,
            );
        }
        if !choice.arriving || self.dim != Dim::Overworld {
            return;
        }
        if choice
            .settlement_id
            .as_ref()
            .and_then(|id| self.civilization.registry.settlement(id))
            .is_none_or(|t| {
                !matches!(
                    t.status,
                    horizon_mods::civilization::settlements::Status::Ready
                        | horizon_mods::civilization::settlements::Status::PotentialCapital
                )
            })
        {
            return;
        }
        let [x, y, z] = choice.start.position;
        self.place_portal_ticket((x >> 4, z >> 4));
        let mut landing = None;
        'find: for radius in 0_i32..=8 {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs().max(dz.abs()) != radius || !self.world.loaded(x + dx, z + dz) {
                        continue;
                    }
                    for sy in ((y - 16).max(-63)..=(y + 32).min(317)).rev() {
                        let facts = self.res.facts();
                        let floor = facts.get(self.world.block(x + dx, sy - 1, z + dz));
                        if !floor.solid_render
                            || matches!(
                                floor.name.as_ref(),
                                "minecraft:cactus"
                                    | "minecraft:magma_block"
                                    | "minecraft:campfire"
                                    | "minecraft:soul_campfire"
                            )
                            || !facts.is_air(self.world.block(x + dx, sy, z + dz))
                            || !facts.is_air(self.world.block(x + dx, sy + 1, z + dz))
                        {
                            continue;
                        }
                        let pos = horizon_core::geometry::Vec3::new(
                            f64::from(x + dx) + 0.5,
                            f64::from(sy),
                            f64::from(z + dz) + 0.5,
                        );
                        if !self.border.contains(pos.x - 0.4, pos.z - 0.4)
                            || !self.border.contains(pos.x + 0.4, pos.z + 0.4)
                        {
                            continue;
                        }
                        if self
                            .biome_name_at(pos)
                            .is_some_and(|b| choice.race.habitat(&b))
                        {
                            landing = Some([pos.x, pos.y, pos.z]);
                            break 'find;
                        }
                    }
                }
            }
        }
        if let Some(pos) = landing {
            let old = self.civilization.registry.clone();
            self.civilization.registry.finish_race_arrival(&player);
            if let Err(error) = self.save_civilization() {
                self.civilization.registry = old;
                log::warn!("Civilization arrival was deferred: {error}");
                return;
            }
            self.teleport_to(Target::Player, pos, None);
            self.system_message(Text::tr("horizon.civilization.race.arrived", &[]));
        }
    }
}
