//! Bounded live census, recognition assessment and server-authenticated council dialogs.
use super::Game;
use horizon_client::{
    scoreboard::json_to_tag,
    text::{ClickEvent, Style, Text},
};
use horizon_mods::civilization::{
    Stage,
    governance::{Assessment, RECOGNITION_REQUIRED, SAMPLE_INTERVAL},
};
use serde_json::{Value, json};

fn tr(key: &str) -> Value {
    json!({"translate":format!("horizon.civilization.council.{key}")})
}
fn message(text: Text) -> Value {
    json!({"type":"minecraft:plain_message","contents":text.component(),"width":360})
}
fn action(key: &str, command: &str) -> Value {
    json!({"label":tr(key),"width":170,"action":{"type":"minecraft:run_command","command":command}})
}

impl Game<'_> {
    pub(super) fn civ_tick_council(&mut self) {
        let now = self.world.game_time;
        let cities: Vec<_> = self
            .civilization
            .registry
            .civilizations()
            .filter(|c| {
                c.bell.dimension == self.dim.id()
                    && self.world.loaded(c.bell.x, c.bell.z)
                    && self
                        .civilization
                        .survey_attempts
                        .get(&c.id)
                        .is_none_or(|tick| {
                            now < *tick || now.saturating_sub(*tick) >= SAMPLE_INTERVAL
                        })
            })
            .take(4)
            .map(|c| c.id.clone())
            .collect();
        for id in cities {
            if let Err(e) = self.civ_assess(&id) {
                log::debug!("Civilization census {id}: {e}");
            }
        }
    }

    fn civ_assess(&mut self, id: &str) -> Result<(), String> {
        let c = self
            .civilization
            .registry
            .civilization(id)
            .ok_or("horizon.civilization.quest.no_city")?
            .clone();
        if c.bell.dimension != self.dim.id() {
            return Ok(());
        }
        let now = self.world.game_time;
        if self
            .civilization
            .survey_attempts
            .get(id)
            .is_some_and(|tick| now >= *tick && now.saturating_sub(*tick) < SAMPLE_INTERVAL)
        {
            return Ok(());
        }
        self.civilization.survey_attempts.insert(id.into(), now);
        for cx in (c.bell.x - 33) >> 4..=(c.bell.x + 33) >> 4 {
            for cz in (c.bell.z - 33) >> 4..=(c.bell.z + 33) >> 4 {
                if self
                    .world
                    .chunks
                    .get(&(cx, cz))
                    .is_none_or(|chunk| !chunk.lit)
                {
                    return Ok(());
                }
            }
        }
        let town = self
            .civilization
            .registry
            .settlement(&c.settlement_id)
            .unwrap()
            .clone();
        let population = self.civ_resident_count(&town).min(32) as u16;
        let (beds, table, furnace) = self.civ_census([c.bell.x, c.bell.y, c.bell.z]);
        let bell = self
            .res
            .facts()
            .get(self.world.block(c.bell.x, c.bell.y, c.bell.z))
            .name
            .as_ref()
            == "minecraft:bell";
        let old = self.civilization.registry.clone();
        let granted = self.civilization.registry.assess(
            id,
            Assessment::observation(
                now.max(0),
                population,
                beds.min(4096) as u16,
                bell,
                table,
                furnace,
            ),
        )?;
        if let Err(e) = self.save_civilization() {
            self.civilization.registry = old;
            return Err(e);
        }
        if granted {
            self.civilization.new_elders.insert(id.into());
        }
        Ok(())
    }

    pub(super) fn civ_title_notice(&mut self) {
        let player = self.civ_player_id();
        if self
            .civilization
            .registry
            .civilization_for(&player)
            .is_some_and(|c| c.leader == player && self.civilization.new_elders.contains(&c.id))
        {
            self.system_message(
                Text::tr("horizon.civilization.council.granted", &[])
                    .append(Text::literal(" "))
                    .append(
                        Text::tr("horizon.civilization.council.open", &[]).with_style(&Style {
                            click: Some(ClickEvent::RunCommand("/civ panel".into())),
                            underlined: Some(true),
                            ..Default::default()
                        }),
                    ),
            );
        }
    }

    pub(super) fn civ_provision(&mut self) -> Result<String, String> {
        let player = self.civ_player_id();
        let c = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?
            .clone();
        let town = self
            .civilization
            .registry
            .settlement(&c.settlement_id)
            .unwrap()
            .clone();
        self.civ_near_speaker(&town.npc_owner, &town.residents[1])?;
        self.civ_assess(&c.id)?;
        let c = self.civilization.registry.civilization(&c.id).unwrap();
        let m = c
            .metrics()
            .ok_or("horizon.civilization.council.no_survey")?;
        let missing = m.food_target.saturating_sub(m.food);
        if missing == 0 {
            return Err("horizon.civilization.council.enough_food".into());
        }
        let old = self.civilization.registry.clone();
        let inventory = self.inventory.clone();
        let offhand = self.offhand.clone();
        let result = self
            .civ_take_available_materials(&[("minecraft:bread".into(), missing)])
            .and_then(|receipt| {
                self.civilization
                    .registry
                    .provision_bread(&player, receipt[0].count)
            })
            .and_then(|_| self.save_civilization());
        if let Err(e) = result {
            self.civilization.registry = old;
            self.inventory = inventory;
            self.offhand = offhand;
            return Err(e);
        }
        Ok("horizon.civilization.council.provisioned".into())
    }

    pub(super) fn civ_panel(&mut self, page: &str) -> Result<String, String> {
        if page.starts_with("population") {
            return self.civ_population_panel(page);
        }
        if page.starts_with("stock") {
            return self.civ_stock_panel(page);
        }
        let player = self.civ_player_id();
        let id = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?
            .id
            .clone();
        self.civ_assess(&id)?;
        let c = self
            .civilization
            .registry
            .civilization(&id)
            .unwrap()
            .clone();
        let g = c.governance.as_ref().unwrap();
        let leader = c.leader == player;
        let admin = leader && c.stage == Stage::Elder;
        let role = if admin {
            "role_elder"
        } else if c.founder == player {
            "role_founder"
        } else {
            "role_member"
        };
        let mut body = vec![message(Text::translate(
            "horizon.civilization.council.identity",
            &[
                Text::literal(c.name.clone()),
                Text::literal(c.id.clone()),
                Text::tr(&format!("horizon.civilization.council.{role}"), &[]),
            ],
        ))];
        let mut actions = Vec::new();
        let mut inputs = Vec::new();
        match page {
            "overview" | "metrics" | "materials" => {
                if let Some(m) = c.metrics() {
                    let age = self.world.game_time.saturating_sub(m.observed_at).max(0) / 20;
                    body.push(message(Text::tr(
                        "horizon.civilization.council.age",
                        &[&age],
                    )));
                    if page == "materials" {
                        body.push(message(Text::tr(
                            "horizon.civilization.council.reserves",
                            &[
                                &m.food,
                                &c.reserved_item("minecraft:oak_log"),
                                &c.reserved_item("minecraft:oak_planks"),
                                &c.reserved_item("minecraft:cobblestone"),
                                &c.reserved_item("minecraft:white_wool"),
                            ],
                        )));
                        body.push(message(Text::tr(
                            "horizon.civilization.council.allocation",
                            &[],
                        )));
                        body.push(message(Text::tr(
                            "horizon.civilization.council.iron_production",
                            &[],
                        )));
                    } else {
                        body.push(message(Text::tr(
                            "horizon.civilization.council.metrics",
                            &[
                                &m.recognition,
                                &RECOGNITION_REQUIRED,
                                &m.wellbeing,
                                &m.development,
                                &m.stability,
                            ],
                        )));
                        body.push(message(Text::tr(
                            "horizon.civilization.council.population",
                            &[
                                &m.population,
                                &m.beds,
                                &m.housing_occupied,
                                &m.housing_free,
                                &m.food,
                                &m.food_target,
                            ],
                        )));
                        if page == "overview" {
                            body = vec![
                                json!({"type":"minecraft:plain_message","width":360,"contents":{"text":"","extra":[
                                    body[0]["contents"].clone(),{"text":"\n\n"},body[2]["contents"].clone(),{"text":"\n\n"},body[3]["contents"].clone(),{"text":"\n\n"},Text::tr("horizon.civilization.council.age",&[&age]).component()
                                ]}}),
                            ];
                        } else {
                            let a = g.assessment.as_ref().unwrap();
                            for (key, ok) in [
                                ("population", m.population > 0),
                                ("recognition", m.recognition >= RECOGNITION_REQUIRED),
                                ("food", m.food >= m.food_target && m.population > 0),
                                ("housing", a.beds >= a.population && a.population > 0),
                                ("bell", a.bell),
                                ("table", a.crafting_table),
                                ("furnace", a.furnace),
                                ("stability", a.stable_samples == 3),
                            ] {
                                body.push(message(
                                    Text::tr(
                                        &format!("horizon.civilization.council.condition_{key}"),
                                        &[],
                                    )
                                    .append(Text::literal(" — "))
                                    .append(Text::tr(
                                        if ok {
                                            "horizon.civilization.council.yes"
                                        } else {
                                            "horizon.civilization.council.no"
                                        },
                                        &[],
                                    )),
                                ));
                            }
                        }
                    }
                } else {
                    body.push(message(Text::tr(
                        "horizon.civilization.council.no_survey",
                        &[],
                    )));
                }
                if c.stage == Stage::Elder && page != "overview" {
                    body.push(message(Text::tr(
                        "horizon.civilization.council.title_kept",
                        &[],
                    )));
                }
                actions.push(action("refresh", &format!("civ panel {page}")));
                actions.push(action("indicators", "civ panel metrics"));
                actions.push(action("supply", "civ provision"));
                actions.push(action("orders", "civ quests"));
                actions.push(action("materials", "civ panel materials"));
                actions.push(action("members", "civ panel members"));
                if page == "materials" {
                    actions.push(json!({"label":{"translate":"horizon.civilization.population.title"},"width":170,"action":{"type":"minecraft:run_command","command":"civ panel population"}}));
                    actions.push(json!({"label":{"translate":"horizon.civilization.stock.title"},"width":170,"action":{"type":"minecraft:run_command","command":"civ panel stock"}}));
                }
            }
            "members" => {
                body.push(message(Text::tr(
                    "horizon.civilization.council.member_count",
                    &[&c.members.len()],
                )));
                body.push(message(Text::tr(
                    "horizon.civilization.council.invite_count",
                    &[&c.invitations.len()],
                )));
                for invitation in &c.invitations {
                    body.push(message(Text::literal(invitation.clone())));
                }
                for member in &c.members {
                    body.push(message(Text::literal(member.clone()).append(
                        Text::literal(if *member == c.leader { " ★" } else { "" }),
                    )));
                }
                if admin {
                    actions.push(action("invite", "civ panel invite"));
                    if c.members.len() > 1 {
                        actions.push(action("transfer", "civ panel transfer"));
                        actions.push(action("kick", "civ panel kick"));
                    }
                } else {
                    body.push(message(Text::tr(
                        "horizon.civilization.council.read_only",
                        &[],
                    )));
                }
            }
            "invite" | "transfer" | "kick" => {
                if !admin {
                    return Err("horizon.civilization.council.leader_only".into());
                }
                if page == "invite" {
                    inputs.push(json!({"type":"minecraft:text","key":"player","label":tr("uuid"),"width":300,"max_length":128}));
                } else {
                    let options: Vec<_> = c
                        .members
                        .iter()
                        .filter(|m| **m != player)
                        .map(|m| json!({"id":m,"display":m}))
                        .collect();
                    if options.is_empty() {
                        return Err("horizon.civilization.council.no_target".into());
                    }
                    inputs.push(json!({"type":"minecraft:single_option","key":"player","label":tr("target"),"width":300,"options":options}));
                }
                body.push(message(Text::tr(
                    &format!("horizon.civilization.council.confirm_{page}"),
                    &[],
                )));
                actions.push(json!({"label":tr("confirm"),"width":170,"action":{"type":"minecraft:dynamic/run_command","template":format!("civ govern {page} $(player)")}}));
            }
            _ => return Err("horizon.civilization.council.usage".into()),
        }
        if page != "overview" {
            actions.push(action("overview", "civ panel overview"));
        }
        let dialog = json!({"type":"minecraft:multi_action","title":tr(if c.stage==Stage::Elder {"title"}else{"progress_title"}),"pause":false,"can_close_with_escape":true,"after_action":"close", "body":body,"inputs":inputs,"actions":actions,"columns":2,"exit_action":{"label":tr("close"),"width":170}});
        self.show_click_dialog(&json_to_tag(&dialog));
        Ok(String::new())
    }

    pub(super) fn civ_govern(&mut self, operation: &str, target: &str) -> Result<String, String> {
        let player = self.civ_player_id();
        let c = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?;
        if c.stage != Stage::Elder || c.leader != player {
            return Err("horizon.civilization.council.leader_only".into());
        }
        let old = self.civilization.registry.clone();
        match operation {
            "invite" => self.civilization.registry.invite(&player, target)?,
            "transfer" => self
                .civilization
                .registry
                .transfer_leadership(&player, target)?,
            "kick" => self.civilization.registry.remove_member(&player, target)?,
            _ => return Err("horizon.civilization.council.usage".into()),
        }
        if let Err(e) = self.save_civilization() {
            self.civilization.registry = old;
            return Err(e);
        }
        self.system_message(Text::tr("horizon.civilization.council.updated", &[]));
        self.civ_panel("members")
    }
}
