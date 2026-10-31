//! Opt-in, server-owned first campaign slice. No client-supplied inventory or progress.
use super::{Game, execution_role::ExecutionRole};
use horizon_client::text::{ClickEvent, Style, Text};
use horizon_mods::{
    civilization::{Bell, Civilization, Registry, Stage, VillageEvidence},
    owned_npcs::{Appearance, EnsureNpc, Position, Presence},
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[derive(Default)]
pub(super) struct Campaign {
    registry: Registry,
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
        let registry = match fs::metadata(&path) {
            Ok(meta) if meta.len() > 4 * 1024 * 1024 => Err("Campaign exceeds 4 MiB".into()),
            Ok(_) => fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| Registry::restore(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
            Err(e) => Err(e.to_string()),
        };
        match registry {
            Ok(registry) => {
                self.civilization = Campaign {
                    registry,
                    path: Some(path),
                    error: None,
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
        let bytes = self.civilization.registry.save()?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
        fs::rename(temporary, path).map_err(|e| e.to_string())
    }

    pub(super) fn civilization_status(&self) -> Value {
        let mut status = json!({"enabled": self.civilization_enabled(),
            "campaign": self.civilization.registry});
        if let Some(error) = &self.civilization.error {
            status["error"] = error.clone().into();
        }
        status
    }

    pub(super) fn tick_civilization(&mut self) {
        if !self.civilization_enabled() || !self.ticks.is_multiple_of(20) {
            return;
        }
        self.for_each_player(|g| {
            let _ = g.civ_introduce();
        });
        let cities: Vec<_> = self
            .civilization
            .registry
            .civilizations()
            .filter(|c| c.bell.dimension == self.dim.id())
            .cloned()
            .collect();
        for city in cities {
            if let Err(error) = self.civ_ensure_speakers(&city) {
                log::debug!("Civilization speakers: {error}");
            }
        }
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

    fn civ_ensure_speakers(&mut self, city: &Civilization) -> Result<(), String> {
        if !self.world.loaded(city.bell.x, city.bell.z) {
            return Ok(());
        }
        for (index, quest) in city.quests.iter().enumerate() {
            if self.owned_npc_state("civilization", &quest.npc_key).is_ok() {
                continue;
            }
            // Only stand on solid ground with two air blocks. Never clear village blocks.
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
                            let [x, y, z] = [city.bell.x + dx, city.bell.y + dy, city.bell.z + dz];
                            if !self.world.loaded(x, z) {
                                continue;
                            }
                            let facts = self.res.facts();
                            if facts.is_air(self.world.block(x, y, z))
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
            let spawn =
                spawn.ok_or("Нет безопасного места для представителя общины рядом с ратушей")?;
            self.ensure_owned_npc(
                "civilization",
                EnsureNpc {
                    stable_key: quest.npc_key.clone(),
                    definition: "civilization:representative".into(),
                    city: city.id.clone(),
                    profession: quest.role.clone(),
                    appearance: Appearance {
                        model: "minecraft:entity/villager".into(),
                        texture: "minecraft:entity/villager/villager".into(),
                    },
                    dimension: city.bell.dimension.clone(),
                    spawn,
                },
            )?;
        }
        Ok(())
    }

    fn civ_found(&mut self, pos: [i32; 3], name: &str) -> Result<String, String> {
        if !self.civ_has_charter() {
            return Err("Нужна учредительная грамота из стартового набора".into());
        }
        let (beds, residents) = self.civ_village_evidence(pos)?;
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
        let keys = self
            .civilization
            .registry
            .civilization(&id)
            .unwrap()
            .quests
            .iter()
            .map(|q| q.npc_key.clone())
            .collect::<Vec<_>>();
        let result = self
            .preflight_owned_npc_keys("civilization", &keys)
            .and_then(|_| self.civ_consume_charter())
            .and_then(|_| self.save_civilization());
        if let Err(error) = result {
            self.civilization.registry = old;
            self.inventory = inventory;
            self.offhand = offhand;
            return Err(error);
        }
        let city = self
            .civilization
            .registry
            .civilization(&id)
            .unwrap()
            .clone();
        // Pending speakers retry on the server tick if a chunk is not ready.
        let _ = self.civ_ensure_speakers(&city);
        Ok(format!(
            "Община «{name}» основана ({id}). Колокол объявлен ратушей. Вы пока гость: заслужите признание жителей. /civ quests"
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

    fn civ_near_speaker(&mut self, key: &str) -> Result<(), String> {
        let npc = self.owned_npc_state("civilization", key)?;
        let p = self.player.as_ref().ok_or("Нет игрока")?.position;
        if npc.dimension != self.dim.id()
            || npc.presence != Presence::Loaded
            || (p.x - npc.position.x).powi(2)
                + (p.y - npc.position.y).powi(2)
                + (p.z - npc.position.z).powi(2)
                > 64.0
        {
            return Err(format!("Подойдите к жителю {key} ближе 8 блоков"));
        }
        Ok(())
    }

    fn civ_execute(&mut self, command: &str) -> Result<String, String> {
        self.civ_introduce()?;
        let player = self.civ_player_id();
        let mut words = command.split_whitespace();
        words.next();
        match words.next().unwrap_or("status") {
            "help" | "guide" => Ok(format!(
                "{}\n/civ quests — задания; /civ talk quarry|granary|housing — разговор рядом с жителем; /civ deliver ID — передать все материалы; /civ status — признание; /civ invite UUID; /civ join ID. Ваш UUID: {player}",
                horizon_mods::civilization::GUIDE
            )),
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
            "status" | "resources" => {
                let Some(city) = self.civilization.registry.civilization_for(&player) else {
                    return Ok("Найдите деревню с колоколом, двумя кроватями и жителем. Используйте на колоколе учредительную грамоту. /civ help".into());
                };
                let status = format!(
                    "{} [{}] — {}. Признание {}/60; общественные проекты {}/3. Участников: {}.",
                    city.name,
                    city.id,
                    if city.stage == Stage::Elder {
                        "Староста"
                    } else {
                        "Гость общины"
                    },
                    city.recognition,
                    city.projects,
                    city.members.len()
                );
                if city.stage == Stage::Elder {
                    Ok(format!(
                        "{status}\nОбщий склад: дерево {}, еда {}; жилых мест {}. Производство будет следующим этапом.",
                        city.resources.wood, city.resources.food, city.resources.housing
                    ))
                } else {
                    Ok(format!(
                        "{status}\nГородской учёт откроется после признания. /civ quests"
                    ))
                }
            }
            "quests" => {
                let city = self
                    .civilization
                    .registry
                    .civilization_for(&player)
                    .ok_or("Сначала основать общину или вступить по приглашению")?
                    .clone();
                for quest in &city.quests {
                    let text = if quest.completed_by.is_some() {
                        format!("✓ {} — выполнено", quest.title)
                    } else {
                        let pos = self
                            .owned_npc_state("civilization", &quest.npc_key)
                            .ok()
                            .map(|n| {
                                format!(
                                    " [{:.0} {:.0} {:.0}]",
                                    n.position.x, n.position.y, n.position.z
                                )
                            })
                            .unwrap_or_default();
                        format!("{}: {}{} — [поговорить]", quest.id, quest.title, pos)
                    };
                    let style = Style {
                        click: Some(ClickEvent::RunCommand(format!("/civ talk {}", quest.id))),
                        ..Default::default()
                    };
                    self.system_message(Text::literal(text).with_style(&style));
                }
                Ok("Материалы передаются только рядом с соответствующим жителем; повторная награда исключена.".into())
            }
            "talk" | "deliver" => {
                let operation = command.split_whitespace().nth(1).unwrap();
                let id = words.next().ok_or("Укажите quarry, granary или housing")?;
                let quest = self.civilization.registry.quest(&player, id)?.clone();
                self.civ_near_speaker(&quest.npc_key)?;
                if operation == "talk" {
                    return Ok(format!(
                        "{}: {}. Нужно: {}. Награда: 20 признания и 1 общественный проект. Передать: /civ deliver {}",
                        quest.role,
                        quest.title,
                        quest
                            .materials
                            .iter()
                            .map(|m| format!("{} ×{}", m.item, m.count))
                            .collect::<Vec<_>>()
                            .join(", "),
                        id
                    ));
                }
                let old = self.civilization.registry.clone();
                let inventory = self.inventory.clone();
                let offhand = self.offhand.clone();
                self.civ_take_materials(
                    &quest
                        .materials
                        .iter()
                        .map(|m| (m.item.clone(), m.count))
                        .collect::<Vec<_>>(),
                )?;
                let result = self
                    .civilization
                    .registry
                    .complete_quest(&player, id)
                    .and_then(|_| self.save_civilization());
                if let Err(error) = result {
                    self.civilization.registry = old;
                    self.inventory = inventory;
                    self.offhand = offhand;
                    return Err(error);
                }
                let elder = self
                    .civilization
                    .registry
                    .civilization_for(&player)
                    .unwrap()
                    .stage
                    == Stage::Elder;
                Ok(format!(
                    "{}: материалы приняты. +20 признания. {}",
                    quest.title,
                    if elder {
                        "Община признала вас Старостой! Открыт городской учёт: /civ resources"
                    } else {
                        "/civ status"
                    }
                ))
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
            _ => Err("Неизвестная команда. /civ help".into()),
        }
    }

    fn civ_reply(&mut self, result: Result<String, String>) {
        match result {
            Ok(text) => self.system_message(Text::literal(text)),
            Err(error) => self.command_failure(Text::literal(error)),
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
