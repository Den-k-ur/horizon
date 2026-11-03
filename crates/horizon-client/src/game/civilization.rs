//! Opt-in, server-owned first campaign slice. No client-supplied inventory or progress.
use super::{Game, execution_role::ExecutionRole};
use horizon_client::text::Text;
use horizon_mods::{
    civilization::{Bell, Registry, Stage, VillageEvidence, storage},
    owned_npcs::Presence,
};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct Campaign {
    pub(super) registry: Registry,
    pub(super) searches: std::collections::BTreeMap<String, super::civilization_start::Search>,
    pub(super) start_errors: std::collections::BTreeMap<String, String>,
    pub(super) survey_attempts: std::collections::BTreeMap<String, i64>,
    pub(super) new_elders: std::collections::BTreeSet<String>,
    path: Option<PathBuf>,
    error: Option<String>,
}

impl Game<'_> {
    pub(super) fn civilization_enabled(&self) -> bool {
        self.civilization.path.is_some() && self.civilization.error.is_none()
    }

    pub(super) fn prepare_civilization(&mut self) {
        if self.options.execution_role == ExecutionRole::Client || self.remote.is_some() {
            return;
        }
        self.civilization = Campaign::default();
        let path = self.world_root.join("horizon-civilization.json");
        if !self.options.civilization && !path.exists() {
            return;
        }
        let registry = storage::load(&path);
        match registry {
            Ok(registry) => {
                self.civilization = Campaign {
                    registry,
                    path: Some(path),
                    error: None,
                    ..Default::default()
                };
                if let Err(error) = self.save_civilization() {
                    self.civilization.error = Some(error);
                }
            }
            Err(error) => {
                log::warn!("Civilization campaign was not loaded: {error}");
                self.civilization = Campaign {
                    path: Some(path),
                    error: Some(error),
                    ..Default::default()
                };
            }
        }
    }

    pub(super) fn save_civilization(&self) -> Result<(), String> {
        if let Some(error) = &self.civilization.error {
            return Err(error.clone());
        }
        let Some(path) = &self.civilization.path else {
            return Ok(());
        };
        storage::save(path, &self.civilization.registry)
    }

    pub(super) fn civilization_status(&self) -> Value {
        let mut status = json!({"enabled": self.civilization_enabled(),
            "campaign": self.civilization.registry,
            "start_searches": self.civilization.searches.iter().map(|(player,search)| (player.clone(),search.phase())).collect::<std::collections::BTreeMap<_,_>>(),
            "start_errors": self.civilization.start_errors});
        if let Some(error) = &self.civilization.error {
            status["error"] = error.clone().into();
        }
        status
    }

    pub(super) fn tick_civilization(&mut self) {
        if !self.civilization_enabled() || !self.ticks.is_multiple_of(20) {
            return;
        }
        self.civ_poll_race_searches();
        self.civ_tick_settlements();
        self.civ_tick_council();
        self.civ_tick_population();
        self.for_each_player_messaged(|g| {
            let _ = g.civ_introduce();
            g.civ_tick_start();
            g.civ_title_notice();
        });
        self.civilization.new_elders.clear();
    }

    fn civ_introduce(&mut self) -> Result<(), String> {
        if self.player.is_none() || self.spectator() {
            return Ok(());
        }
        let player = self.civ_player_id();
        if self.civilization.registry.has_started(&player) {
            return Ok(());
        }
        let old = self.civilization.registry.clone();
        let inventory = self.inventory.clone();
        let offhand = self.offhand.clone();
        self.civ_give_start_items()?;
        let result = self
            .civilization
            .registry
            .begin_player(&player)
            .and_then(|_| self.save_civilization());
        if result.is_err() {
            self.civilization.registry = old;
            self.inventory = inventory;
            self.offhand = offhand;
        }
        result
    }

    fn civ_found(&mut self, pos: [i32; 3], name: &str) -> Result<String, String> {
        if !self.civ_has_charter() {
            return Err("Нужна учредительная грамота из стартового набора".into());
        }
        let beds = self.civ_village_evidence(pos)?;
        if beds < 2 {
            return Err("horizon.civilization.settlement.beds".into());
        }
        let player = self.civ_player_id();
        if self
            .civilization
            .registry
            .civilization_for(&player)
            .is_some()
        {
            return Err("horizon.civilization.settlement.member".into());
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 48 || name.chars().any(char::is_control) {
            return Err("Название должно содержать от 1 до 48 печатных символов".into());
        }
        let bell = Bell {
            dimension: self.dim.id().into(),
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
        let old = self.civilization.registry.clone();
        let town_id = self.civilization.registry.register_settlement(bell)?;
        let town = self
            .civilization
            .registry
            .settlement(&town_id)
            .unwrap()
            .clone();
        if town.status != horizon_mods::civilization::settlements::Status::Ready
            || town.claimed_by.is_some()
        {
            self.civilization.registry = old;
            return Err("horizon.civilization.settlement.not_ready".into());
        }
        if let Err(error) = self.save_civilization() {
            self.civilization.registry = old;
            return Err(error);
        }
        self.civ_ensure_residents(&town)?;
        let residents = self.civ_resident_count(&town);
        if residents < 1 {
            return Err("horizon.civilization.settlement.no_space".into());
        }
        let player = self.civ_player_id();
        let old = self.civilization.registry.clone();
        let inventory = self.inventory.clone();
        let offhand = self.offhand.clone();
        let id = self.civilization.registry.found(
            &player,
            name,
            Bell {
                dimension: self.dim.id().into(),
                x: pos[0],
                y: pos[1],
                z: pos[2],
            },
            VillageEvidence {
                beds: beds.min(u32::from(u16::MAX)) as u16,
                residents: residents.min(u32::from(u16::MAX)) as u16,
            },
        )?;
        let result = self
            .civilization
            .registry
            .activate_needs(&id, residents as u16, beds.min(4096) as u16)
            .and_then(|_| self.civ_consume_charter())
            .and_then(|_| self.save_civilization());
        if let Err(error) = result {
            self.civilization.registry = old;
            self.inventory = inventory;
            self.offhand = offhand;
            return Err(error);
        }
        Ok(format!(
            "Община «{name}» основана ({id}). Колокол объявлен ратушей. Вы — основатель: заслужите признание жителей, чтобы стать Старостой. /civ quests"
        ))
    }

    pub(super) fn civ_use_charter(&mut self, pos: [i32; 3]) -> bool {
        if !self.civilization_enabled()
            || !self.civ_holds_charter()
            || self
                .res
                .facts()
                .get(self.world.block(pos[0], pos[1], pos[2]))
                .name
                .as_ref()
                != "minecraft:bell"
        {
            return false;
        }
        let result = self.civ_found(pos, "Новая община");
        self.civ_reply(result);
        true
    }

    pub(super) fn civ_near_speaker(&mut self, owner: &str, key: &str) -> Result<(), String> {
        let npc = self.owned_npc_state(owner, key)?;
        let p = self.player.as_ref().ok_or("Нет игрока")?.position;
        if npc.dimension != self.dim.id()
            || npc.presence != Presence::Loaded
            || (p.x - npc.position.x).powi(2)
                + (p.y - npc.position.y).powi(2)
                + (p.z - npc.position.z).powi(2)
                > 64.0
        {
            return Err("horizon.civilization.quest.too_far".into());
        }
        if self.spectator()
            || self
                .player
                .as_ref()
                .is_none_or(|p| p.health.health() <= 0.0)
        {
            return Err("horizon.civilization.quest.unavailable".into());
        }
        let eye = self
            .eye_position()
            .ok_or("horizon.civilization.quest.unavailable")?;
        let delta = [
            npc.position.x - eye[0],
            npc.position.y + 1.0 - eye[1],
            npc.position.z - eye[2],
        ];
        let distance = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
        if distance > 0.01
            && self
                .clip_ray(eye, delta.map(|v| v / distance), distance - 0.05)
                .is_some()
        {
            return Err("horizon.civilization.quest.obstructed".into());
        }
        Ok(())
    }

    fn civ_execute(&mut self, command: &str) -> Result<String, String> {
        self.civ_introduce()?;
        let player = self.civ_player_id();
        let mut words = command.split_whitespace();
        words.next();
        match words.next().unwrap_or("status") {
            "race" => {
                if words.next() != Some("choose") {
                    return Ok("horizon.civilization.race.help".into());
                }
                let race = words
                    .next()
                    .and_then(horizon_mods::civilization::races::Race::parse)
                    .ok_or("horizon.civilization.race.help")?;
                if words.next().is_some() {
                    return Err("horizon.civilization.race.help".into());
                }
                self.civ_request_race(race)
            }
            "settlement" => {
                if words.next() != Some("retry") || words.next().is_some() {
                    return Err("horizon.civilization.settlement.help".into());
                }
                let choice = self
                    .civilization
                    .registry
                    .race_choice(&player)
                    .ok_or("horizon.civilization.race.help")?;
                let race = choice.race;
                if choice
                    .settlement_id
                    .as_ref()
                    .and_then(|id| self.civilization.registry.settlement(id))
                    .is_none_or(|t| {
                        t.status != horizon_mods::civilization::settlements::Status::Blocked
                    })
                {
                    return Err("horizon.civilization.settlement.not_blocked".into());
                }
                self.civ_request_race(race)
            }
            "compass" => {
                if words.next().is_some() {
                    return Err("horizon.civilization.race.help".into());
                }
                self.civ_recover_compass()
            }
            "guide" => self.civ_recover_guide(),
            "help" => {
                self.system_message(Text::tr("horizon.civilization.membership.help", &[]));
                self.system_message(Text::tr(horizon_mods::civilization::GUIDE, &[]));
                self.system_message(Text::tr("horizon.civilization.quest.help", &[&player]));
                Ok("horizon.civilization.quest.list_hint".into())
            }
            "id" => Ok(player),
            "recover-kit" => {
                if self
                    .civilization
                    .registry
                    .civilization_for(&player)
                    .is_some()
                    || self.civ_has_charter()
                {
                    return Err(
                        "Восстановление нужно только до основания общины при утрате грамоты".into(),
                    );
                }
                self.civ_give_start_items()?;
                Ok("Книга и утраченная грамота восстановлены.".into())
            }
            "found" => {
                let pos = self.pick().ok_or("Наведите прицел на колокол деревни")?.pos;
                let name = words.collect::<Vec<_>>().join(" ");
                self.civ_found(
                    pos,
                    if name.is_empty() {
                        "Новая община"
                    } else {
                        &name
                    },
                )
            }
            "charter" => {
                let mut position = [0_i32; 3];
                for coordinate in &mut position {
                    *coordinate = words
                        .next()
                        .and_then(|v| v.parse().ok())
                        .ok_or("horizon.civilization.settlement.charter_usage")?;
                }
                if words.next().is_some() || !self.civ_holds_charter() {
                    return Err("horizon.civilization.settlement.charter_usage".into());
                }
                self.civ_found(position, "Новая община")
            }
            "status" | "resources" => {
                let city = self
                    .civilization
                    .registry
                    .civilization_for(&player)
                    .ok_or("horizon.civilization.quest.no_city")?
                    .clone();
                let role = if city.stage == Stage::Elder && city.leader == player {
                    "elder"
                } else if city.founder == player {
                    "founder"
                } else {
                    "member"
                };
                let total = city.quests.len() as u32;
                self.system_message(Text::translate(
                    "horizon.civilization.status.summary",
                    &[
                        Text::literal(city.name.clone()),
                        Text::tr(&format!("horizon.civilization.status.{role}"), &[]),
                        Text::literal(city.recognition.to_string()),
                        Text::literal(
                            horizon_mods::civilization::governance::RECOGNITION_REQUIRED
                                .to_string(),
                        ),
                        Text::literal(city.projects.to_string()),
                        Text::literal(total.to_string()),
                        Text::literal(city.members.len().to_string()),
                        Text::literal(city.id.clone()),
                    ],
                ));
                if let Some(m) = city.metrics() {
                    self.system_message(Text::tr(
                        "horizon.civilization.council.population",
                        &[
                            &m.population,
                            &m.beds,
                            &m.housing_occupied,
                            &m.housing_free,
                            &m.food,
                            &m.food_target,
                        ],
                    ));
                }
                self.system_message(
                    Text::tr("horizon.civilization.council.open", &[]).with_style(
                        &horizon_client::text::Style {
                            click: Some(horizon_client::text::ClickEvent::RunCommand(
                                "/civ panel".into(),
                            )),
                            ..Default::default()
                        },
                    ),
                );
                Ok("horizon.civilization.quest.list_hint".into())
            }
            "quests" => self.civ_list_quests(),
            "panel" => {
                let page = words.next().unwrap_or("overview");
                if words.next().is_some() {
                    return Err("horizon.civilization.council.usage".into());
                }
                self.civ_panel(page)
            }
            "provision" => {
                if words.next().is_some() {
                    return Err("horizon.civilization.council.usage".into());
                }
                self.civ_provision()
            }
            "stock" => self.civ_stock_command(&words.collect::<Vec<_>>()),
            "workers" => self.civ_workers_command(&words.collect::<Vec<_>>()),
            "govern" => {
                let op = words.next().ok_or("horizon.civilization.council.usage")?;
                let target = words.next().ok_or("horizon.civilization.council.usage")?;
                if words.next().is_some() {
                    return Err("horizon.civilization.council.usage".into());
                }
                self.civ_govern(op, target)
            }
            "talk" | "deliver" | "resident" => {
                let operation = command.split_whitespace().nth(1).unwrap();
                let argument = words.next().ok_or("horizon.civilization.quest.usage")?;
                if words.next().is_some() {
                    return Err("horizon.civilization.quest.usage".into());
                }
                if operation == "resident" {
                    self.civ_talk_resident(argument)
                } else if operation == "talk" {
                    self.civ_talk_quest(argument)
                } else {
                    self.civ_deliver_quest(argument)
                }
            }
            "invite" | "join" => {
                let operation = command.split_whitespace().nth(1).unwrap();
                let argument = words.next().ok_or("Укажите UUID игрока или ID общины")?;
                let old = self.civilization.registry.clone();
                if operation == "invite" {
                    self.civilization.registry.invite(&player, argument)?;
                } else {
                    self.civilization.registry.join(&player, argument)?;
                }
                if let Err(error) = self.save_civilization() {
                    self.civilization.registry = old;
                    return Err(error);
                }
                Ok(if operation == "invite" {
                    "Приглашение сохранено. Другой игрок может вступить через /civ join ID общины."
                } else {
                    "Вы вступили в общину; задания и признание общие."
                }
                .into())
            }
            action @ ("leave" | "kick" | "transfer") => {
                let other = if action == "leave" {
                    None
                } else {
                    Some(
                        words
                            .next()
                            .ok_or("horizon.civilization.error.member_usage")?,
                    )
                };
                if words.next().is_some() {
                    return Err("horizon.civilization.error.member_usage".into());
                }
                let old = self.civilization.registry.clone();
                match action {
                    "leave" => self.civilization.registry.leave(&player)?,
                    "kick" => self
                        .civilization
                        .registry
                        .remove_member(&player, other.unwrap())?,
                    _ => self
                        .civilization
                        .registry
                        .transfer_leadership(&player, other.unwrap())?,
                }
                if let Err(error) = self.save_civilization() {
                    self.civilization.registry = old;
                    log::warn!("Civilization membership change was rolled back: {error}");
                    return Err("horizon.civilization.error.save_failed".into());
                }
                Ok(match action {
                    "leave" => "horizon.civilization.membership.left",
                    "kick" => "horizon.civilization.membership.removed",
                    _ => "horizon.civilization.membership.transferred",
                }
                .into())
            }
            _ => Err("Неизвестная команда. /civ help".into()),
        }
    }

    pub(super) fn civ_reply(&mut self, result: Result<String, String>) {
        let message = |value: String| {
            if value.starts_with("horizon.civilization.") {
                Text::tr(&value, &[])
            } else {
                Text::literal(value)
            }
        };
        match result {
            Ok(text) if text.is_empty() => {}
            Ok(text) => self.system_message(message(text)),
            Err(error) => self.command_failure(message(error)),
        }
    }

    pub(super) fn civilization_command(&mut self, command: &str) -> bool {
        if command != "civ" && !command.starts_with("civ ") {
            return false;
        }
        if self.options.execution_role == ExecutionRole::Client {
            return false;
        }
        if !self.civilization_enabled() {
            self.civ_reply(Err(self.civilization.error.clone().unwrap_or(
                "Режим выключен. Запустите этот мир с --civilization".into(),
            )));
        } else {
            let result = self.civ_execute(command);
            self.civ_reply(result);
        }
        true
    }
}
