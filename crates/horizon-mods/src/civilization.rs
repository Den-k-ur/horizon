//! Server-owned Civilization progression. The runtime supplies authenticated player UUIDs,
//! observes the village, and removes physical items before acknowledging a quest.
//! This model deliberately grants no inventory, block or NPC authority of its own.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub mod economy;
pub mod governance;
pub mod needs;
pub mod population;
pub mod races;
pub mod settlements;
pub mod storage;

const VERSION: u32 = 8;
const MAX_PLAYERS: usize = 4096;
const MAX_CIVILIZATIONS: usize = 256;
const MAX_MEMBERS: usize = 64;
pub const TERRITORY_RADIUS: i32 = 32;
pub const GUIDE: &str = "horizon.civilization.guide.summary";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bell {
    pub dimension: String,
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Observations gathered by the host from loaded blocks/entities, never client claims.
#[derive(Clone, Copy, Debug)]
pub struct VillageEvidence {
    pub beds: u16,
    pub residents: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Recognition,
    Elder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub item: String,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quest {
    pub id: String,
    pub title: String,
    pub role: String,
    pub npc_key: String,
    pub materials: Vec<Material>,
    pub completed_by: Option<String>,
    #[serde(default)]
    pub delivery: Option<needs::Delivery>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    pub wood: u32,
    pub food: u32,
    pub housing: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Civilization {
    pub id: String,
    pub name: String,
    pub founder: String,
    /// The founder is historical; authority can pass to another current member.
    #[serde(default)]
    pub leader: String,
    pub members: BTreeSet<String>,
    /// Preserve attribution when a contributor leaves, without retaining their rights.
    #[serde(default)]
    pub former_members: BTreeSet<String>,
    pub invitations: BTreeSet<String>,
    pub bell: Bell,
    #[serde(default)]
    pub settlement_id: String,
    pub recognition: u32,
    pub projects: u32,
    pub stage: Stage,
    pub quests: Vec<Quest>,
    #[serde(default)]
    pub needs: Option<needs::Plan>,
    #[serde(default)]
    pub governance: Option<governance::Governance>,
    #[serde(default)]
    pub economy: Option<economy::Store>,
    #[serde(default)]
    pub population: Option<population::Population>,
    /// Available only to the management stage; quest deliveries are spent rebuilding.
    /// They are not also credited as spendable treasury materials.
    pub resources: Resources,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    version: u32,
    next_id: u32,
    introduced: BTreeSet<String>,
    #[serde(default)]
    choices: BTreeMap<String, races::Choice>,
    #[serde(default)]
    settlements: BTreeMap<String, settlements::Settlement>,
    #[serde(default = "first_settlement")]
    next_settlement: u32,
    civilizations: BTreeMap<String, Civilization>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 1,
            introduced: BTreeSet::new(),
            choices: BTreeMap::new(),
            settlements: BTreeMap::new(),
            next_settlement: 1,
            civilizations: BTreeMap::new(),
        }
    }
}
fn first_settlement() -> u32 {
    1
}

fn player_id(player: &str) -> Result<(), String> {
    // The host serializes its UUID into a stable ASCII key. Display names are not identities.
    if player.is_empty()
        || player.len() > 128
        || !player
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:".contains(&b))
    {
        return Err("Некорректный постоянный идентификатор игрока".into());
    }
    Ok(())
}

fn valid_bell(bell: &Bell) -> bool {
    bell.dimension == "minecraft:overworld"
        && (-29_999_984..=29_999_984).contains(&bell.x)
        && (-29_999_984..=29_999_984).contains(&bell.z)
        && (-64..=319).contains(&bell.y)
}

fn overlaps(a: &Bell, b: &Bell) -> bool {
    let dx = i64::from(a.x) - i64::from(b.x);
    let dz = i64::from(a.z) - i64::from(b.z);
    a.dimension == b.dimension && dx * dx + dz * dz <= i64::from(TERRITORY_RADIUS * 2).pow(2)
}

fn quests(id: &str) -> Vec<Quest> {
    [
        (
            "quarry",
            "Восстановить каменоломню",
            "mason",
            vec![("minecraft:oak_log", 16), ("minecraft:cobblestone", 32)],
        ),
        (
            "granary",
            "Пополнить зернохранилище",
            "farmer",
            vec![("minecraft:wheat", 24)],
        ),
        (
            "housing",
            "Подготовить материалы для жилья",
            "builder",
            vec![("minecraft:oak_planks", 32)],
        ),
    ]
    .into_iter()
    .map(|(key, title, role, materials)| Quest {
        id: key.into(),
        title: title.into(),
        role: role.into(),
        npc_key: format!("{id}.{key}"),
        materials: materials
            .into_iter()
            .map(|(item, count)| Material {
                item: item.into(),
                count,
            })
            .collect(),
        completed_by: None,
        delivery: None,
    })
    .collect()
}

impl Registry {
    pub fn settlements(&self) -> impl Iterator<Item = &settlements::Settlement> {
        self.settlements.values()
    }
    pub fn settlement(&self, id: &str) -> Option<&settlements::Settlement> {
        self.settlements.get(id)
    }
    pub fn settlement_at(&self, bell: &Bell) -> Option<&settlements::Settlement> {
        self.settlements.values().find(|s| s.bell == *bell)
    }
    pub fn race_choices(&self) -> impl Iterator<Item = (&String, &races::Choice)> {
        self.choices.iter()
    }
    pub fn compass_target(&self, player: &str) -> Option<&Bell> {
        self.civilization_for(player).map(|c| &c.bell).or_else(|| {
            self.choices
                .get(player)
                .and_then(|c| c.settlement_id.as_ref())
                .and_then(|id| self.settlements.get(id))
                .map(|s| &s.bell)
        })
    }
    fn add_settlement(
        &mut self,
        bell: Bell,
        race: Option<races::Race>,
        biome: String,
        kind: settlements::Kind,
    ) -> Result<String, String> {
        if !valid_bell(&bell) {
            return Err("horizon.civilization.settlement.invalid".into());
        }
        if let Some(town) = self.settlement_at(&bell) {
            return Ok(town.id.clone());
        }
        if self.settlements.len() >= 512
            || self.settlements.values().any(|s| overlaps(&s.bell, &bell))
        {
            return Err("horizon.civilization.settlement.occupied".into());
        }
        if kind == settlements::Kind::Generated
            && self
                .settlements
                .values()
                .filter(|s| s.status == settlements::Status::Building)
                .count()
                >= 4
        {
            return Err("horizon.civilization.race.busy".into());
        }
        let next = self
            .next_settlement
            .checked_add(1)
            .ok_or("Settlement IDs exhausted")?;
        let id = format!("town-{}", self.next_settlement);
        let town = settlements::Settlement::new(id.clone(), bell, race, biome, kind);
        if !town.valid() {
            return Err("horizon.civilization.settlement.invalid".into());
        }
        self.settlements.insert(id.clone(), town);
        self.next_settlement = next;
        Ok(id)
    }
    pub fn register_settlement(&mut self, bell: Bell) -> Result<String, String> {
        self.add_settlement(bell, None, String::new(), settlements::Kind::Adopted)
    }
    pub fn start_building(&mut self, id: &str) {
        if let Some(t) = self.settlements.get_mut(id)
            && t.status == settlements::Status::Building
        {
            t.building_started = true;
        }
    }
    pub fn finish_building(&mut self, id: &str) {
        if let Some(t) = self.settlements.get_mut(id)
            && t.status == settlements::Status::Building
            && t.building_started
        {
            t.status = settlements::Status::Ready;
        }
    }
    pub fn block_settlement(&mut self, id: &str) {
        if let Some(t) = self.settlements.get_mut(id)
            && t.status == settlements::Status::Building
        {
            t.status = settlements::Status::Blocked;
        }
    }
    pub fn race_choice(&self, player: &str) -> Option<&races::Choice> {
        self.choices.get(player)
    }

    pub fn choose_race(
        &mut self,
        player: &str,
        race: races::Race,
        start: races::Start,
    ) -> Result<(), String> {
        if !self.has_started(player) || !start.valid(race) {
            return Err("horizon.civilization.race.invalid_start".into());
        }
        let mut arriving = true;
        if let Some(choice) = self.choices.get(player) {
            let retry = choice
                .settlement_id
                .as_ref()
                .and_then(|id| self.settlements.get(id))
                .is_some_and(|t| t.status == settlements::Status::Blocked);
            if choice.race != race || choice.settlement_id.is_some() && !retry {
                return Err("horizon.civilization.race.locked".into());
            }
            arriving = choice.arriving;
        }
        let town = self.add_settlement(
            start.settlement.clone(),
            Some(race),
            start.biome.clone(),
            settlements::Kind::Generated,
        )?;
        if self.settlements[&town].claimed_by.is_some()
            || self.settlements[&town].race != Some(race)
        {
            return Err("horizon.civilization.settlement.occupied".into());
        }
        self.choices.insert(
            player.into(),
            races::Choice {
                race,
                start,
                arriving,
                settlement_id: Some(town),
            },
        );
        Ok(())
    }

    pub fn finish_race_arrival(&mut self, player: &str) {
        if let Some(choice) = self.choices.get_mut(player) {
            choice.arriving = false;
        }
    }

    /// Returns true once per world/player. Persist only together with the granted starter kit.
    pub fn begin_player(&mut self, player: &str) -> Result<bool, String> {
        player_id(player)?;
        if self.introduced.contains(player) {
            return Ok(false);
        }
        if self.introduced.len() >= MAX_PLAYERS {
            return Err("Достигнут лимит игроков цивилизации".into());
        }
        self.introduced.insert(player.into());
        Ok(true)
    }

    pub fn has_started(&self, player: &str) -> bool {
        self.introduced.contains(player)
    }

    pub fn is_introduced(&self, player: &str) -> bool {
        self.has_started(player)
    }

    pub fn civilizations(&self) -> impl Iterator<Item = &Civilization> {
        self.civilizations.values()
    }

    pub fn civilization_for(&self, player: &str) -> Option<&Civilization> {
        self.civilizations
            .values()
            .find(|c| c.members.contains(player))
    }

    pub fn civilization(&self, id: &str) -> Option<&Civilization> {
        self.civilizations.get(id)
    }

    pub fn found(
        &mut self,
        player: &str,
        name: &str,
        bell: Bell,
        village: VillageEvidence,
    ) -> Result<String, String> {
        player_id(player)?;
        if !self.has_started(player) {
            return Err("Сначала получите Хартию и руководство".into());
        }
        if self.civilization_for(player).is_some() {
            return Err("Вы уже состоите в цивилизации".into());
        }
        if !valid_bell(&bell) || village.beds < 2 || village.residents < 1 {
            return Err("Нужна деревня в обычном мире: колокол, две кровати и житель".into());
        }
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 48 || name.chars().any(char::is_control) {
            return Err("Название должно содержать от 1 до 48 печатных символов".into());
        }
        if self.civilizations.len() >= MAX_CIVILIZATIONS {
            return Err("Достигнут лимит цивилизаций".into());
        }
        if self
            .civilizations
            .values()
            .any(|c| overlaps(&c.bell, &bell))
        {
            return Err("Эта деревня уже занята или слишком близка к другой ратуше".into());
        }
        let id = format!("civ-{}", self.next_id);
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Исчерпаны идентификаторы")?;
        let town_id = self.register_settlement(bell.clone())?;
        let town = &self.settlements[&town_id];
        if town.status != settlements::Status::Ready || town.claimed_by.is_some() {
            return Err("horizon.civilization.settlement.not_ready".into());
        }
        let mut city_quests = quests(&id);
        for (quest, key) in city_quests.iter_mut().zip(&town.residents) {
            quest.npc_key = key.clone();
        }
        let civilization = Civilization {
            id: id.clone(),
            name: name.into(),
            founder: player.into(),
            leader: player.into(),
            members: BTreeSet::from([player.into()]),
            former_members: BTreeSet::new(),
            invitations: BTreeSet::new(),
            bell,
            settlement_id: town_id.clone(),
            recognition: 0,
            projects: 0,
            stage: Stage::Recognition,
            quests: city_quests,
            needs: None,
            governance: Some(governance::Governance::default()),
            economy: Some(economy::Store::default()),
            population: Some(population::Population::new(&town.residents)),
            resources: Resources {
                wood: 0,
                food: 0,
                housing: 0,
            },
        };
        self.next_id = next_id;
        self.civilizations.insert(id.clone(), civilization);
        let town = self.settlements.get_mut(&town_id).unwrap();
        town.claimed_by = Some(id.clone());
        town.status = settlements::Status::PotentialCapital;
        if let Some(choice) = self.choices.get_mut(player) {
            choice.arriving = false;
        }
        self.clear_invitations(player);
        Ok(id)
    }

    pub fn invite(&mut self, actor: &str, other: &str) -> Result<(), String> {
        player_id(other)?;
        if self.civilization_for(other).is_some() {
            return Err("Игрок уже состоит в цивилизации".into());
        }
        let civ = self.civilization_for_mut(actor)?;
        if civ.leader != actor {
            return Err("horizon.civilization.error.leader_only".into());
        }
        if civ.invitations.contains(other) {
            return Ok(());
        }
        if civ.members.len() + civ.invitations.len() >= MAX_MEMBERS {
            return Err("Достигнут лимит участников и приглашений".into());
        }
        civ.invitations.insert(other.into());
        Ok(())
    }

    pub fn join(&mut self, player: &str, id: &str) -> Result<(), String> {
        player_id(player)?;
        if !self.has_started(player) || self.civilization_for(player).is_some() {
            return Err(
                "Сначала получите руководство; участник другой общины вступить не может".into(),
            );
        }
        let civ = self
            .civilizations
            .get_mut(id)
            .ok_or("Цивилизация не найдена")?;
        if !civ.invitations.contains(player) || civ.members.len() >= MAX_MEMBERS {
            return Err("Нет приглашения или община заполнена".into());
        }
        civ.invitations.remove(player);
        civ.former_members.remove(player);
        civ.members.insert(player.into());
        self.clear_invitations(player);
        Ok(())
    }

    fn clear_invitations(&mut self, player: &str) {
        for civilization in self.civilizations.values_mut() {
            civilization.invitations.remove(player);
        }
    }

    pub fn transfer_leadership(&mut self, actor: &str, other: &str) -> Result<(), String> {
        let civ = self.civilization_for_mut(actor)?;
        if civ.leader != actor {
            return Err("horizon.civilization.error.leader_only".into());
        }
        if !civ.members.contains(other) {
            return Err("horizon.civilization.error.current_member".into());
        }
        civ.leader = other.into();
        Ok(())
    }

    pub fn leave(&mut self, player: &str) -> Result<(), String> {
        let civ = self.civilization_for_mut(player)?;
        if civ.leader == player {
            return Err("horizon.civilization.error.transfer_first".into());
        }
        civ.members.remove(player);
        civ.former_members.insert(player.into());
        Ok(())
    }

    pub fn remove_member(&mut self, actor: &str, other: &str) -> Result<(), String> {
        let civ = self.civilization_for_mut(actor)?;
        if civ.leader != actor {
            return Err("horizon.civilization.error.leader_only".into());
        }
        if civ.leader == other {
            return Err("horizon.civilization.error.transfer_first".into());
        }
        if !civ.members.remove(other) {
            return Err("horizon.civilization.error.current_member".into());
        }
        civ.former_members.insert(other.into());
        Ok(())
    }

    pub fn quest(&self, player: &str, id: &str) -> Result<&Quest, String> {
        let civ = self
            .civilization_for(player)
            .ok_or("Вы не состоите в общине")?;
        let quest = civ
            .quests
            .iter()
            .find(|q| q.id == id)
            .ok_or("Задание не найдено")?;
        if quest.completed_by.is_some() {
            return Err("Задание уже выполнено".into());
        }
        Ok(quest)
    }

    /// The authoritative runtime must remove ALL listed physical materials first, atomically.
    /// Callers must not expose this acknowledgement directly to a network/client request.
    pub fn complete_quest(&mut self, player: &str, id: &str) -> Result<(), String> {
        self.quest(player, id)?;
        if self.quest(player, id)?.delivery.is_some() {
            let receipt = self.quest(player, id)?.remaining();
            return self.record_delivery(player, id, &receipt).map(|_| ());
        }
        let civ = self.civilization_for_mut(player)?;
        let quest = civ
            .quests
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or("Задание не найдено")?;
        quest.completed_by = Some(player.into());
        civ.recognition += 20;
        civ.projects += 1;
        Ok(())
    }

    fn civilization_for_mut(&mut self, player: &str) -> Result<&mut Civilization, String> {
        self.civilizations
            .values_mut()
            .find(|c| c.members.contains(player))
            .ok_or_else(|| "Вы не состоите в общине".into())
    }

    pub fn save(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        serde_json::to_vec_pretty(self).map_err(|e| e.to_string())
    }

    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("Сохранение цивилизаций слишком велико".into());
        }
        let mut registry: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if registry.version == 1 {
            for civ in registry.civilizations.values_mut() {
                // A legacy save cannot smuggle new authority or historical membership.
                if !civ.leader.is_empty() || !civ.former_members.is_empty() {
                    return Err("Invalid version 1 membership data".into());
                }
                civ.leader.clone_from(&civ.founder);
            }
            registry.version = 2;
        }
        if registry.version == 2 {
            if !registry.choices.is_empty() {
                return Err("Invalid version 2 race data".into());
            }
            registry.version = 3;
        }
        if registry.version == 3 {
            if !registry.settlements.is_empty()
                || registry.next_settlement != 1
                || registry.choices.values().any(|c| c.settlement_id.is_some())
                || registry
                    .civilizations
                    .values()
                    .any(|c| !c.settlement_id.is_empty())
            {
                return Err("Invalid version 3 settlement data".into());
            }
            for city in registry.civilizations.values_mut() {
                let town = settlements::Settlement::legacy(city);
                city.settlement_id = town.id.clone();
                registry.settlements.insert(town.id.clone(), town);
            }
            for (player, choice) in &mut registry.choices {
                if !choice.arriving
                    && let Some(city) = registry
                        .civilizations
                        .values()
                        .find(|c| c.members.contains(player))
                {
                    choice.settlement_id = Some(city.settlement_id.clone());
                }
            }
            registry.version = 4;
        }
        if registry.version == 4 {
            if registry
                .civilizations
                .values()
                .any(|c| c.needs.is_some() || c.quests.iter().any(|q| q.delivery.is_some()))
            {
                return Err("Invalid version 4 quest data".into());
            }
            registry.version = 5;
        }
        if registry.version == 5 {
            for c in registry.civilizations.values_mut() {
                let elder = c.quests.iter().all(|q| q.completed_by.is_some());
                if c.governance.is_some()
                    || (c.stage == Stage::Elder) != elder
                    || c.resources.housing
                        != if elder {
                            c.needs.as_ref().map_or(6, |p| u32::from(p.beds))
                        } else {
                            0
                        }
                {
                    return Err("Invalid version 5 title data".into());
                }
                c.governance = Some(governance::Governance {
                    legacy_title: elder,
                    ..Default::default()
                });
            }
            registry.version = 6;
        }
        if registry.version == 6 {
            for c in registry.civilizations.values_mut() {
                if c.economy.is_some()
                    || c.governance
                        .as_ref()
                        .and_then(|g| g.grant.as_ref())
                        .is_some_and(|g| g.food > 256)
                {
                    return Err("Invalid version 6 economy data".into());
                }
                c.economy = Some(economy::Store::migrate(c));
            }
            registry.version = 7;
        }
        if registry.version == 7 {
            for c in registry.civilizations.values_mut() {
                if c.population.is_some()
                    || c.economy.as_ref().is_none_or(|s| {
                        s.consumed.keys().any(|id| id != "minecraft:oak_log")
                            || s.produced.keys().any(|id| id != "minecraft:oak_planks")
                    })
                {
                    return Err("Invalid version 7 population data".into());
                }
                let town = registry
                    .settlements
                    .get(&c.settlement_id)
                    .ok_or("Invalid settlement")?;
                c.population = Some(population::Population::new(&town.residents));
            }
            registry.version = VERSION;
        }
        registry.validate()?;
        Ok(registry)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION
            || self.next_id == 0
            || self.introduced.len() > MAX_PLAYERS
            || self.civilizations.len() > MAX_CIVILIZATIONS
            || self.settlements.len() > 512
            || self.next_settlement == 0
        {
            return Err("Неподдерживаемое или повреждённое сохранение цивилизаций".into());
        }
        for player in &self.introduced {
            player_id(player)?;
        }
        for (player, choice) in &self.choices {
            if !self.introduced.contains(player) || !choice.start.valid(choice.race) {
                return Err("Invalid race choice or start".into());
            }
            if let Some(id) = &choice.settlement_id {
                let town = self
                    .settlements
                    .get(id)
                    .ok_or("Missing navigation settlement")?;
                if town.kind != settlements::Kind::Legacy && town.bell != choice.start.settlement {
                    return Err("Navigation settlement does not match start".into());
                }
                if town.race.is_some_and(|r| r != choice.race) {
                    return Err("Invalid settlement race".into());
                }
            }
        }
        let mut town_bells = Vec::new();
        for (id, town) in &self.settlements {
            if *id != town.id || !town.valid() || town_bells.iter().any(|b| overlaps(b, &town.bell))
            {
                return Err("Invalid settlement registry".into());
            }
            if town.kind != settlements::Kind::Legacy {
                let serial = id
                    .strip_prefix("town-")
                    .and_then(|v| v.parse::<u32>().ok())
                    .ok_or("Invalid settlement ID")?;
                if serial == 0 || serial >= self.next_settlement || *id != format!("town-{serial}")
                {
                    return Err("Invalid settlement sequence".into());
                }
            }
            if let Some(city) = &town.claimed_by {
                if self
                    .civilizations
                    .get(city)
                    .is_none_or(|c| c.settlement_id != *id || c.bell != town.bell)
                {
                    return Err("Invalid settlement claim".into());
                }
            }
            town_bells.push(town.bell.clone());
        }
        let mut members = BTreeSet::new();
        let mut bells = Vec::new();
        for (id, civ) in &self.civilizations {
            let serial = id
                .strip_prefix("civ-")
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or("Повреждённый идентификатор цивилизации")?;
            if serial == 0
                || serial >= self.next_id
                || *id != format!("civ-{serial}")
                || civ.id != *id
                || !valid_bell(&civ.bell)
                || civ.name.trim() != civ.name
                || civ.name.is_empty()
                || civ.name.chars().count() > 48
                || civ.name.chars().any(char::is_control)
                || civ.members.is_empty()
                || civ.members.len() + civ.invitations.len() > MAX_MEMBERS
                || !civ.members.contains(&civ.leader)
                || !civ.members.contains(&civ.founder) && !civ.former_members.contains(&civ.founder)
                || !civ.members.is_disjoint(&civ.former_members)
                || bells.iter().any(|b| overlaps(b, &civ.bell))
            {
                return Err("Повреждённые данные общины".into());
            }
            bells.push(civ.bell.clone());
            let town = self
                .settlements
                .get(&civ.settlement_id)
                .ok_or("Missing capital settlement")?;
            if town.claimed_by.as_ref() != Some(id) || town.bell != civ.bell {
                return Err("Invalid capital binding".into());
            }
            for member in &civ.members {
                if !self.introduced.contains(member) || !members.insert(member) {
                    return Err("Повторное или неизвестное членство".into());
                }
            }
            for former in &civ.former_members {
                if !self.introduced.contains(former) {
                    return Err("Unknown historical member".into());
                }
            }
            for invitation in &civ.invitations {
                player_id(invitation)?;
                if civ.members.contains(invitation) {
                    return Err("Участник не может иметь приглашение в свою общину".into());
                }
            }
            if civ.needs.as_ref().is_some_and(|plan| !plan.valid()) {
                return Err("Invalid settlement needs".into());
            }
            let mut canonical = civ
                .needs
                .as_ref()
                .map(|p| p.quests(&town.residents))
                .unwrap_or_else(|| quests(id));
            for (quest, key) in canonical.iter_mut().zip(&town.residents) {
                quest.npc_key = key.clone();
            }
            if canonical.len() != civ.quests.len() {
                return Err("Повреждённый набор заданий".into());
            }
            let mut completed = 0;
            for (expected, actual) in canonical.iter().zip(&civ.quests) {
                if expected.id != actual.id
                    || expected.title != actual.title
                    || expected.role != actual.role
                    || expected.npc_key != actual.npc_key
                    || expected.materials != actual.materials
                    || if civ.needs.is_some() {
                        !actual.valid_delivery(&civ.members, &civ.former_members)
                    } else {
                        actual.delivery.is_some()
                    }
                {
                    return Err("Несовместимое определение задания".into());
                }
                if let Some(by) = &actual.completed_by {
                    if !civ.members.contains(by) && !civ.former_members.contains(by) {
                        return Err("Неизвестный исполнитель задания".into());
                    }
                    completed += 1;
                }
            }
            if civ.projects != completed
                || civ.recognition != completed * 20
                || civ.resources.wood != 0
                || civ.resources.food != 0
                || !civ.valid_governance()
                || civ.economy.as_ref().is_none_or(|s| !s.valid())
                || civ
                    .population
                    .as_ref()
                    .is_none_or(|p| !p.valid(civ, &self.settlements[&civ.settlement_id].residents))
            {
                return Err(
                    "Прогресс не соответствует выполненным заданиям или подтверждённым условиям"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bell(x: i32) -> Bell {
        Bell {
            dimension: "minecraft:overworld".into(),
            x,
            y: 64,
            z: 0,
        }
    }
    fn village() -> VillageEvidence {
        VillageEvidence {
            beds: 2,
            residents: 1,
        }
    }
    fn founded() -> Registry {
        let mut state = Registry::default();
        assert!(state.begin_player("uuid-a").unwrap());
        state
            .found("uuid-a", "Рассвет", bell(0), village())
            .unwrap();
        state
    }

    #[test]
    fn starter_and_foundation_are_not_repeatable() {
        let mut state = Registry::default();
        assert!(state.found("uuid-a", "A", bell(0), village()).is_err());
        assert!(state.begin_player("uuid-a").unwrap());
        assert!(!state.begin_player("uuid-a").unwrap());
        assert!(
            state
                .found(
                    "uuid-a",
                    "A",
                    bell(0),
                    VillageEvidence {
                        beds: 0,
                        residents: 1
                    }
                )
                .is_err()
        );
        assert!(state.found("uuid-a", "A", bell(0), village()).is_ok());
        assert!(state.found("uuid-a", "B", bell(300), village()).is_err());
        state.begin_player("uuid-b").unwrap();
        assert!(state.found("uuid-b", "B", bell(0), village()).is_err());
        assert!(state.found("uuid-b", "B", bell(64), village()).is_err());
        assert!(state.found("uuid-b", "B", bell(65), village()).is_ok());
    }

    #[test]
    fn cooperative_work_needs_invitation_and_is_rewarded_once() {
        let mut state = founded();
        state.begin_player("uuid-b").unwrap();
        state.begin_player("uuid-c").unwrap();
        assert!(state.join("uuid-b", "civ-1").is_err());
        assert!(state.quest("uuid-b", "quarry").is_err());
        state.invite("uuid-a", "uuid-b").unwrap();
        state.join("uuid-b", "civ-1").unwrap();
        assert!(state.invite("uuid-b", "uuid-c").is_err());
        state.complete_quest("uuid-b", "quarry").unwrap();
        assert!(state.complete_quest("uuid-a", "quarry").is_err());
        let civ = state.civilization_for("uuid-a").unwrap();
        assert_eq!(
            (civ.recognition, civ.projects, civ.stage),
            (20, 1, Stage::Recognition)
        );
        assert_eq!(civ.quests[0].completed_by.as_deref(), Some("uuid-b"));
        state.complete_quest("uuid-a", "granary").unwrap();
        assert_eq!(
            state.civilization_for("uuid-b").unwrap().stage,
            Stage::Recognition
        );
        state.complete_quest("uuid-b", "housing").unwrap();
        let civ = state.civilization_for("uuid-b").unwrap();
        assert_eq!(
            (civ.recognition, civ.projects, civ.stage),
            (60, 3, Stage::Recognition)
        );
        assert_eq!(
            civ.resources,
            Resources {
                wood: 0,
                food: 0,
                housing: 0
            }
        );
    }

    #[test]
    fn reload_preserves_uuid_identity_progress_and_invites() {
        let mut state = founded();
        state.invite("uuid-a", "uuid-b").unwrap();
        state.complete_quest("uuid-a", "granary").unwrap();
        let mut restored = Registry::restore(&state.save().unwrap()).unwrap();
        assert_eq!(state, restored);
        assert!(!restored.begin_player("uuid-a").unwrap());
        assert!(restored.complete_quest("uuid-a", "granary").is_err());
        restored.begin_player("uuid-b").unwrap();
        restored.join("uuid-b", "civ-1").unwrap();
        assert_eq!(restored.civilization_for("uuid-b").unwrap().id, "civ-1");
    }

    #[test]
    fn accepting_or_founding_clears_all_pending_invitations_and_retries_are_idempotent() {
        let mut state = founded();
        state.begin_player("uuid-b").unwrap();
        state
            .found("uuid-b", "Другой", bell(300), village())
            .unwrap();
        state.begin_player("uuid-c").unwrap();
        for founder in ["uuid-a", "uuid-b"] {
            state.invite(founder, "uuid-c").unwrap();
        }
        state.join("uuid-c", "civ-1").unwrap();
        assert!(
            state
                .civilizations()
                .all(|c| !c.invitations.contains("uuid-c"))
        );
        state.begin_player("uuid-d").unwrap();
        state.invite("uuid-a", "uuid-d").unwrap();
        state.invite("uuid-b", "uuid-d").unwrap();
        state
            .found("uuid-d", "Новая", bell(600), village())
            .unwrap();
        assert!(
            state
                .civilizations()
                .all(|c| !c.invitations.contains("uuid-d"))
        );

        // The second civilization has its founder and 63 invitations. Repeating
        // a pending invitation consumes no extra slot and must still succeed.
        for i in 0..63 {
            state.invite("uuid-b", &format!("guest-{i}")).unwrap();
        }
        state.invite("uuid-b", "guest-0").unwrap();
        assert!(state.invite("uuid-b", "one-too-many").is_err());
        assert!(state.save().is_ok());
    }

    #[test]
    fn campaign_speaker_identities_are_accepted_by_the_owned_npc_registry() {
        use crate::owned_npcs::{Appearance, EnsureNpc, Position, Registry as NpcRegistry};
        let state = founded();
        let city = state.civilization_for("uuid-a").unwrap();
        let mut npcs = NpcRegistry::default();
        for quest in &city.quests {
            let request = EnsureNpc {
                stable_key: quest.npc_key.clone(),
                definition: "civilization:representative".into(),
                city: city.id.clone(),
                profession: quest.role.clone(),
                appearance: Appearance {
                    model: "minecraft:entity/villager".into(),
                    texture: "minecraft:entity/villager/villager".into(),
                },
                dimension: city.bell.dimension.clone(),
                spawn: Position {
                    x: 0.5,
                    y: 64.0,
                    z: 0.5,
                },
            };
            let instance = npcs
                .ensure("civilization", request.clone())
                .unwrap()
                .clone();
            assert_eq!(instance.stable_key, quest.npc_key);
            assert_eq!(instance.city, city.id);
            assert_eq!(npcs.ensure("civilization", request).unwrap(), &instance);
        }
        assert_eq!(npcs.records().count(), 3);
        assert!(NpcRegistry::restore(&npcs.save().unwrap()).is_ok());
    }

    #[test]
    fn damaged_or_forged_progress_is_not_silently_accepted() {
        let state = founded();
        let valid = serde_json::to_value(&state).unwrap();
        for (field, replacement) in [
            ("recognition", serde_json::json!(60)),
            ("projects", serde_json::json!(3)),
            ("stage", serde_json::json!("elder")),
        ] {
            let mut forged = valid.clone();
            forged["civilizations"]["civ-1"][field] = replacement;
            assert!(Registry::restore(&serde_json::to_vec(&forged).unwrap()).is_err());
        }
        let mut future = valid.clone();
        future["version"] = serde_json::json!(VERSION + 1);
        assert!(Registry::restore(&serde_json::to_vec(&future).unwrap()).is_err());
        let mut bad_cost = valid.clone();
        bad_cost["civilizations"]["civ-1"]["quests"][0]["materials"][0]["count"] =
            serde_json::json!(0);
        assert!(Registry::restore(&serde_json::to_vec(&bad_cost).unwrap()).is_err());
        let mut duplicate = state.clone();
        let mut other = duplicate.civilizations["civ-1"].clone();
        other.id = "civ-2".into();
        other.bell = bell(1000);
        other.quests = quests("civ-2");
        duplicate.civilizations.insert("civ-2".into(), other);
        duplicate.next_id = 3;
        assert!(duplicate.save().is_err());
    }

    #[test]
    fn leadership_and_departure_preserve_history_but_revoke_authority() {
        let mut state = founded();
        state.begin_player("uuid-b").unwrap();
        state.invite("uuid-a", "uuid-b").unwrap();
        state.join("uuid-b", "civ-1").unwrap();
        state.complete_quest("uuid-a", "quarry").unwrap();
        assert!(state.leave("uuid-a").is_err());
        state.transfer_leadership("uuid-a", "uuid-b").unwrap();
        state.leave("uuid-a").unwrap();
        let mut restored = Registry::restore(&state.save().unwrap()).unwrap();
        let city = restored.civilization("civ-1").unwrap();
        assert_eq!(city.founder, "uuid-a");
        assert_eq!(city.leader, "uuid-b");
        assert_eq!(city.quests[0].completed_by.as_deref(), Some("uuid-a"));
        assert_eq!((city.recognition, city.projects), (20, 1));
        assert!(restored.civilization_for("uuid-a").is_none());
        assert!(restored.invite("uuid-a", "uuid-c").is_err());
        assert!(restored.complete_quest("uuid-a", "granary").is_err());
        restored.invite("uuid-b", "uuid-a").unwrap();
        restored.join("uuid-a", "civ-1").unwrap();
        assert!(
            !restored
                .civilization("civ-1")
                .unwrap()
                .former_members
                .contains("uuid-a")
        );
        assert!(restored.complete_quest("uuid-a", "quarry").is_err());
        restored.remove_member("uuid-b", "uuid-a").unwrap();
        assert_eq!(
            Registry::restore(&restored.save().unwrap()).unwrap(),
            restored
        );
    }

    #[test]
    fn membership_changes_cannot_target_other_civilizations_or_the_last_leader() {
        let mut state = founded();
        state.begin_player("uuid-b").unwrap();
        state.found("uuid-b", "B", bell(300), village()).unwrap();
        let before = state.clone();
        assert!(state.transfer_leadership("uuid-a", "uuid-b").is_err());
        assert!(state.remove_member("uuid-a", "uuid-b").is_err());
        assert!(state.remove_member("uuid-a", "uuid-a").is_err());
        assert!(state.leave("uuid-a").is_err());
        assert_eq!(state, before);
        let mut restored = Registry::restore(&state.save().unwrap()).unwrap();
        restored.begin_player("uuid-c").unwrap();
        assert_eq!(
            restored.found("uuid-c", "C", bell(600), village()).unwrap(),
            "civ-3"
        );
        assert_eq!(restored.civilizations().count(), 3);
        assert_eq!(restored.civilization_for("uuid-b").unwrap().name, "B");
    }

    #[test]
    fn forged_leadership_and_historical_membership_are_rejected() {
        let valid = serde_json::to_value(founded()).unwrap();
        for (field, replacement) in [
            ("leader", serde_json::json!("unknown")),
            ("former_members", serde_json::json!(["unknown"])),
            ("former_members", serde_json::json!(["uuid-a"])),
        ] {
            let mut forged = valid.clone();
            forged["civilizations"]["civ-1"][field] = replacement;
            assert!(Registry::restore(&serde_json::to_vec(&forged).unwrap()).is_err());
        }
        let mut mislabeled = valid;
        mislabeled["version"] = 1.into();
        assert!(Registry::restore(&serde_json::to_vec(&mislabeled).unwrap()).is_err());
    }

    fn desert_start() -> races::Start {
        races::Start {
            settlement: bell(300),
            structure: "minecraft:village_desert".into(),
            position: [332, 65, 0],
            biome: "minecraft:desert".into(),
        }
    }

    #[test]
    fn race_choice_survives_reload_and_departure_and_cannot_be_overwritten() {
        let mut state = founded();
        state
            .choose_race("uuid-a", races::Race::DesertCat, desert_start())
            .unwrap();
        let mut state = Registry::restore(&state.save().unwrap()).unwrap();
        assert!(state.race_choice("uuid-a").unwrap().arriving);
        let before = state.clone();
        assert!(
            state
                .choose_race("uuid-a", races::Race::DesertCat, desert_start())
                .is_err()
        );
        assert_eq!(state, before);
        state.finish_race_arrival("uuid-a");
        state.begin_player("uuid-b").unwrap();
        state.invite("uuid-a", "uuid-b").unwrap();
        state.join("uuid-b", "civ-1").unwrap();
        state.transfer_leadership("uuid-a", "uuid-b").unwrap();
        state.leave("uuid-a").unwrap();
        let restored = Registry::restore(&state.save().unwrap()).unwrap();
        let choice = restored.race_choice("uuid-a").unwrap();
        assert_eq!(choice.race, races::Race::DesertCat);
        assert!(!choice.arriving);
        assert_eq!(choice.start.position, [332, 65, 0]);
        assert!(restored.race_choice("uuid-b").is_none());
    }

    #[test]
    fn race_start_rejects_wrong_habitat_coordinates_and_unknown_players() {
        let mut state = founded();
        let before = state.clone();
        assert!(
            state
                .choose_race("unknown", races::Race::DesertCat, desert_start())
                .is_err()
        );
        assert!(
            state
                .choose_race("uuid-a", races::Race::Human, desert_start())
                .is_err()
        );
        let mut start = desert_start();
        start.position[0] = i32::MAX;
        assert!(
            state
                .choose_race("uuid-a", races::Race::DesertCat, start)
                .is_err()
        );
        assert_eq!(state, before);
    }
}
