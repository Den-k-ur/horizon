//! Existing citizens work at player-built tables. Production never depends on the client animation.
use super::Game;
use horizon_client::{scoreboard::json_to_tag, text::Text};
use horizon_mods::{
    civilization::{
        Stage,
        population::{Job, Observation, Profession, Status, WORK_TICKS},
    },
    owned_npcs::{Order, Position, Presence},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn tr(key: &str) -> Value {
    json!({"translate":format!("horizon.civilization.population.{key}")})
}
fn action(key: &str, command: &str) -> Value {
    json!({"label":tr(key),"width":170,"action":{"type":"minecraft:run_command","command":command}})
}
impl Game<'_> {
    /// Loaded safe standing cell adjacent to the actual workstation; never force-load a chunk.
    fn civ_work_approach(&self, job: &Job) -> Option<Position> {
        let [x, y, z] = job.station;
        if !self.world.loaded(x, z)
            || self
                .res
                .facts()
                .get(self.world.block(x, y, z))
                .name
                .as_ref()
                != "minecraft:crafting_table"
        {
            return None;
        }
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let [sx, sz] = [x + dx, z + dz];
            if self
                .world
                .chunks
                .get(&(sx >> 4, sz >> 4))
                .is_some_and(|c| c.lit)
                && self.res.facts().is_air(self.world.block(sx, y, sz))
                && self.res.facts().is_air(self.world.block(sx, y + 1, sz))
                && self
                    .res
                    .facts()
                    .get(self.world.block(sx, y - 1, sz))
                    .solid_render
                && self
                    .border
                    .contains(f64::from(sx) + 0.5, f64::from(sz) + 0.5)
            {
                return Some(Position {
                    x: f64::from(sx) + 0.5,
                    y: f64::from(y),
                    z: f64::from(sz) + 0.5,
                });
            }
        }
        None
    }
    pub(super) fn civ_tick_population(&mut self) {
        let cities: Vec<_> = self
            .civilization
            .registry
            .civilizations()
            .filter(|c| {
                c.stage == Stage::Elder
                    && c.bell.dimension == self.dim.id()
                    && self.world.loaded(c.bell.x, c.bell.z)
            })
            .cloned()
            .collect();
        for c in cities {
            let town = self
                .civilization
                .registry
                .settlement(&c.settlement_id)
                .unwrap()
                .clone();
            let now = self.world.game_time.max(0);
            let a = c.governance.as_ref().unwrap().assessment.as_ref();
            let known = a.is_some_and(|a| now >= a.tick && now - a.tick <= 120);
            let mut observations = BTreeMap::new();
            let mut approaches = BTreeMap::new();
            let mut citizens = BTreeMap::new();
            for (key, w) in &c.population.as_ref().unwrap().workers {
                let npc = self.owned_npc_state(&town.npc_owner, key).ok();
                let available = npc.as_ref().is_some_and(|n| {
                    n.presence == Presence::Loaded && n.dimension == self.dim.id()
                });
                let approach = w.job.as_ref().and_then(|j| self.civ_work_approach(j));
                let arrived = npc.as_ref().zip(approach.as_ref()).is_some_and(|(n, p)| {
                    (n.position.x - p.x).powi(2)
                        + (n.position.y - p.y).powi(2)
                        + (n.position.z - p.z).powi(2)
                        <= 0.9 * 0.9
                });
                observations.insert(
                    key.clone(),
                    Observation {
                        available,
                        workplace: approach.is_some(),
                        arrived,
                        blocked: npc.as_ref().is_some_and(|n| {
                            n.order_status == horizon_mods::owned_npcs::OrderStatus::Blocked
                        }),
                    },
                );
                if let Some(p) = approach {
                    approaches.insert(key.clone(), p);
                }
                if let Some(n) = npc {
                    citizens.insert(key.clone(), n);
                }
            }
            let old = self.civilization.registry.clone();
            let result = self
                .civilization
                .registry
                .tick_population(&c.id, now, a.map_or(0, |a| a.beds), known, &observations)
                .and_then(|_| {
                    if old == self.civilization.registry {
                        Ok(())
                    } else {
                        self.save_civilization()
                    }
                });
            if let Err(error) = result {
                self.civilization.registry = old;
                log::debug!("Population {}: {error}", c.id);
                continue;
            }
            let population = self
                .civilization
                .registry
                .civilization(&c.id)
                .unwrap()
                .population
                .as_ref()
                .unwrap()
                .clone();
            for (key, w) in population.workers {
                let Some(npc) = citizens
                    .get(&key)
                    .filter(|n| n.presence == Presence::Loaded)
                else {
                    continue;
                };
                let profession = w.job.as_ref().map_or("unassigned", |j| j.profession.key());
                if npc.profession != profession {
                    let _ = self.profession_owned_npc(&town.npc_owner, &key, profession);
                }
                let desired = match w.status {
                    Status::Walking | Status::Blocked => {
                        approaches.get(&key).map(|p| Order::MoveTo {
                            target: *p,
                            arrival_radius: 0.7,
                        })
                    }
                    Status::Working => Some(Order::Work {
                        action: format!("civilization:{profession}"),
                        seconds: 3600.0,
                    }),
                    _ => Some(Order::Idle),
                };
                if let Some(order) = desired
                    && npc.order != order
                {
                    let _ = self.order_owned_npc(&town.npc_owner, &key, order);
                }
            }
        }
    }

    pub(super) fn civ_population_panel(&mut self, page: &str) -> Result<String, String> {
        let player = self.civ_player_id();
        let c = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?;
        let p = c.population.as_ref().unwrap();
        let admin = c.leader == player && c.stage == Stage::Elder;
        let survey = c.governance.as_ref().and_then(|g| g.assessment.as_ref());
        let mut text = if p.last_tick.is_none() || survey.is_none() {
            Text::tr("horizon.civilization.population.before_activation", &[])
        } else {
            Text::tr(
                "horizon.civilization.population.summary",
                &[
                    &p.observed_population,
                    &p.observed_beds,
                    &(p.meal_in / 20),
                    &p.rations,
                ],
            )
        };
        text = text.append(Text::literal("\n")).append(Text::tr(
            if p.hungry {
                "horizon.civilization.population.hunger"
            } else {
                "horizon.civilization.population.fed"
            },
            &[],
        ));
        let mut actions = Vec::new();
        let mut inputs = Vec::new();
        if let Some(survey) = survey {
            let age = self.world.game_time.saturating_sub(survey.tick).max(0) / 20;
            text = text
                .append(Text::literal("\n"))
                .append(Text::tr("horizon.civilization.population.age", &[&age]));
        }
        match page {
            "population" => {
                for (index, (_, w)) in p.workers.iter().enumerate() {
                    text = text
                        .append(Text::literal("\n"))
                        .append(Text::tr(
                            "horizon.civilization.population.resident",
                            &[&(index + 1)],
                        ))
                        .append(Text::literal(": "))
                        .append(Text::tr(
                            &format!(
                                "horizon.civilization.population.{}",
                                w.job.as_ref().map_or("unassigned", |j| j.profession.key())
                            ),
                            &[],
                        ))
                        .append(Text::literal(" — "))
                        .append(Text::tr(
                            &format!("horizon.civilization.population.status_{}", w.status.key()),
                            &[],
                        ))
                        .append(Text::literal(format!(
                            " ({}%)",
                            w.progress * 100 / WORK_TICKS
                        )));
                    if let Some(j) = &w.job {
                        text = text.append(Text::literal(format!(
                            " [{}, {}, {}]",
                            j.station[0], j.station[1], j.station[2]
                        )));
                    }
                }
                if admin {
                    actions.push(action("assign", "civ panel population_assign"));
                    actions.push(action("release", "civ panel population_release"));
                }
                actions.push(action("refresh", "civ panel population"));
                actions.push(json!({"label":{"translate":"horizon.civilization.stock.title"},"width":170,"action":{"type":"minecraft:run_command","command":"civ panel stock"}}));
            }
            "population_assign" | "population_release" => {
                if !admin {
                    return Err("horizon.civilization.population.leader_only".into());
                }
                text = Text::tr(
                    if page == "population_assign" {
                        "horizon.civilization.population.help_assign"
                    } else {
                        "horizon.civilization.population.help_release"
                    },
                    &[],
                );
                let workers:Vec<_>=p.workers.keys().enumerate().map(|(i,k)|json!({"id":k,"display":Text::tr("horizon.civilization.population.resident",&[&(i+1)]).component()})).collect();
                inputs.push(json!({"type":"minecraft:single_option","key":"worker","label":tr("citizen"),"width":300,"options":workers}));
                if page == "population_assign" {
                    inputs.push(json!({"type":"minecraft:single_option","key":"job","label":tr("profession"),"width":300,"options":[{"id":"baker","display":tr("baker")},{"id":"carpenter","display":tr("carpenter") }]}));
                    inputs.push(json!({"type":"minecraft:text","key":"place","label":tr("coordinates"),"width":300,"max_length":48}));
                    actions.push(json!({"label":tr("confirm"),"width":170,"action":{"type":"minecraft:dynamic/run_command","template":format!("civ workers assign {} {} $(worker) $(job) $(place)",c.id,p.revision)}}));
                } else {
                    actions.push(json!({"label":tr("confirm"),"width":170,"action":{"type":"minecraft:dynamic/run_command","template":format!("civ workers release {} {} $(worker)",c.id,p.revision)}}));
                }
                actions.push(action("back", "civ panel population"));
            }
            _ => return Err("horizon.civilization.population.invalid".into()),
        }
        actions.push(action("overview", "civ panel overview"));
        let dialog = json!({"type":"minecraft:multi_action","title":tr("title"),"pause":false,"can_close_with_escape":true,"after_action":"close","body":[{"type":"minecraft:plain_message","width":360,"contents":text.component()}],"inputs":inputs,"actions":actions,"columns":2,"exit_action":{"label":tr("close"),"width":170}});
        self.show_click_dialog(&json_to_tag(&dialog));
        Ok(String::new())
    }

    pub(super) fn civ_workers_command(&mut self, args: &[&str]) -> Result<String, String> {
        let invalid = "horizon.civilization.population.invalid";
        let (op, city, revision, key, job) = match args {
            ["release", city, revision, key] => ("release", *city, *revision, *key, None),
            ["assign", city, revision, key, profession, x, y, z] => {
                let profession = match *profession {
                    "baker" => Profession::Baker,
                    "carpenter" => Profession::Carpenter,
                    _ => return Err(invalid.into()),
                };
                let station = [
                    x.parse().map_err(|_| invalid)?,
                    y.parse().map_err(|_| invalid)?,
                    z.parse().map_err(|_| invalid)?,
                ];
                (
                    "assign",
                    *city,
                    *revision,
                    *key,
                    Some(Job {
                        profession,
                        station,
                    }),
                )
            }
            _ => return Err(invalid.into()),
        };
        let revision = revision.parse().map_err(|_| invalid)?;
        let player = self.civ_player_id();
        let c = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?
            .clone();
        if self.spectator()
            || self
                .player
                .as_ref()
                .is_none_or(|p| p.health.health() <= 0.0)
        {
            return Err("horizon.civilization.quest.unavailable".into());
        }
        self.civ_village_evidence([c.bell.x, c.bell.y, c.bell.z])?;
        if let Some(j) = &job
            && (!j.in_territory(&c.bell) || self.civ_work_approach(j).is_none())
        {
            return Err("horizon.civilization.population.invalid_workplace".into());
        }
        let old = self.civilization.registry.clone();
        self.civilization
            .registry
            .assign_worker(&player, city, revision, key, job)?;
        if let Err(e) = self.save_civilization() {
            self.civilization.registry = old;
            return Err(e);
        }
        let _ = op;
        self.system_message(Text::tr("horizon.civilization.population.updated", &[]));
        self.civ_population_panel("population")
    }
}
