//! Settlement construction runs on the root server, independently of acting players.
use super::Game;
use horizon_mods::civilization::settlements::{self, Settlement, Status};
use horizon_mods::owned_npcs::{Appearance, EnsureNpc, Position, Presence};

impl Game<'_> {
    pub(super) fn civ_tick_settlements(&mut self) {
        let towns: Vec<_> = self
            .civilization
            .registry
            .settlements()
            .filter(|t| t.bell.dimension == self.dim.id())
            .cloned()
            .collect();
        for town in towns {
            let result = if town.status == Status::Building {
                self.civ_build_settlement(&town)
            } else if matches!(town.status, Status::Ready | Status::PotentialCapital) {
                self.civ_ensure_residents(&town)
            } else {
                Ok(())
            };
            if let Err(error) = result {
                log::debug!("Civilization settlement {}: {error}", town.id);
            }
        }
    }

    fn civ_build_settlement(&mut self, town: &Settlement) -> Result<(), String> {
        let b = &town.bell;
        for x in (b.x - 10) >> 4..=(b.x + 10) >> 4 {
            for z in (b.z - 10) >> 4..=(b.z + 10) >> 4 {
                self.place_portal_ticket((x, z));
            }
        }
        let cells = settlements::blueprint(town);
        if cells.keys().any(|p| {
            self.world
                .chunks
                .get(&(p[0] >> 4, p[2] >> 4))
                .is_none_or(|c| !c.lit)
        }) {
            return Ok(());
        }
        let mut patch = Vec::new();
        for (pos, spec) in cells {
            let state = super::blockctl::parse_state(spec)
                .and_then(|s| self.pack.assets.registry.resolve(&s).map_err(Into::into))
                .map_err(|e| e.to_string())?;
            let current = self.world.block(pos[0], pos[1], pos[2]);
            let name = &self.res.facts().get(current).name;
            let allowed = settlements::natural(name) || town.building_started && current == state;
            if !allowed
                || !self.border.contains(f64::from(pos[0]), f64::from(pos[2]))
                || !self
                    .border
                    .contains(f64::from(pos[0]) + 1.0, f64::from(pos[2]) + 1.0)
                || self
                    .world
                    .block_entities
                    .contains_key(&(pos[0], pos[1], pos[2]))
                    && !(town.building_started && current == state)
            {
                log::debug!("Civilization site {} blocked at {pos:?} by {name}", town.id);
                let old = self.civilization.registry.clone();
                self.civilization.registry.block_settlement(&town.id);
                if let Err(error) = self.save_civilization() {
                    self.civilization.registry = old;
                    return Err(error);
                }
                return Ok(());
            }
            if current != state {
                patch.push((pos, state));
            }
        }
        if !town.building_started {
            let old = self.civilization.registry.clone();
            self.civilization.registry.start_building(&town.id);
            if let Err(error) = self.save_civilization() {
                self.civilization.registry = old;
                return Err(error);
            }
        }
        // Bound changes per settlement/tick. The deterministic patch resumes after reload.
        let complete = patch.len() <= 512;
        for (pos, state) in patch.into_iter().take(512) {
            self.place_state(pos, state, true)
                .map_err(|_| "horizon.civilization.settlement.invalid".to_owned())?;
        }
        if complete {
            self.civ_ensure_residents(town)?;
            // Flush actual world blocks before publishing Ready in the separate campaign file.
            self.save_game().map_err(|e| e.to_string())?;
            let old = self.civilization.registry.clone();
            self.civilization.registry.finish_building(&town.id);
            if let Err(error) = self.save_civilization() {
                self.civilization.registry = old;
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn civ_ensure_residents(&mut self, town: &Settlement) -> Result<(), String> {
        if !self.world.loaded(town.bell.x, town.bell.z) {
            return Ok(());
        }
        self.preflight_owned_npc_keys(&town.npc_owner, &town.residents)?;
        for (index, key) in town.residents.iter().enumerate() {
            if self.owned_npc_state(&town.npc_owner, key).is_ok() {
                continue;
            }
            let mut spawn = None;
            'search: for radius in 2_i32..=8 {
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        if dx.abs().max(dz.abs()) != radius
                            || (dx + dz).rem_euclid(3) != index as i32
                        {
                            continue;
                        }
                        for dy in -4..=4 {
                            let [x, y, z] = [town.bell.x + dx, town.bell.y + dy, town.bell.z + dz];
                            let facts = self.res.facts();
                            if self.world.loaded(x, z)
                                && facts.is_air(self.world.block(x, y, z))
                                && facts.is_air(self.world.block(x, y + 1, z))
                                && facts.get(self.world.block(x, y - 1, z)).solid_render
                            {
                                spawn = Some(Position {
                                    x: f64::from(x) + 0.5,
                                    y: f64::from(y),
                                    z: f64::from(z) + 0.5,
                                });
                                break 'search;
                            }
                        }
                    }
                }
            }
            self.ensure_owned_npc(
                &town.npc_owner,
                EnsureNpc {
                    stable_key: key.clone(),
                    definition: "civilization:representative".into(),
                    city: town.npc_city.clone(),
                    profession: ["mason", "farmer", "builder"][index].into(),
                    appearance: Appearance {
                        model: "minecraft:entity/villager".into(),
                        texture: "minecraft:entity/villager/villager".into(),
                    },
                    dimension: town.bell.dimension.clone(),
                    spawn: spawn.ok_or("horizon.civilization.settlement.no_space")?,
                },
            )?;
        }
        Ok(())
    }

    pub(super) fn civ_resident_count(&mut self, town: &Settlement) -> u32 {
        town.residents
            .iter()
            .filter(|key| {
                self.owned_npc_state(&town.npc_owner, key).is_ok_and(|n| {
                    n.dimension == town.bell.dimension
                        && n.presence != Presence::Despawned
                        && (n.position.x - f64::from(town.bell.x)).powi(2)
                            + (n.position.z - f64::from(town.bell.z)).powi(2)
                            <= 32.0 * 32.0
                        && (n.position.y - f64::from(town.bell.y)).abs() <= 12.0
                })
            })
            .count() as u32
    }
}
