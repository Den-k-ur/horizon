//! Server-owned Civilization progression. The runtime supplies authenticated player UUIDs,
//! observes the village, and removes physical items before acknowledging a quest.
//! This model deliberately grants no inventory, block or NPC authority of its own.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const VERSION: u32 = 1;
const MAX_PLAYERS: usize = 4096;
const MAX_CIVILIZATIONS: usize = 256;
const MAX_MEMBERS: usize = 64;
pub const TERRITORY_RADIUS: i32 = 32;
pub const GUIDE: &str = "Вы — посланник, а не правитель. Найдите деревню: колокол, не менее двух кроватей и один житель. Примените Хартию на колоколе, чтобы объявить ратушу. Поговорите с мастером, земледельцем и строителем; доставьте материалы по их заданиям. Три восстановленных объекта и 60 признания дадут звание старосты и откроют управление общими ресурсами. Ваши личные рецепты не ограничены технологиями цивилизации.";

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
    pub members: BTreeSet<String>,
    pub invitations: BTreeSet<String>,
    pub bell: Bell,
    pub recognition: u32,
    pub projects: u32,
    pub stage: Stage,
    pub quests: Vec<Quest>,
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
    civilizations: BTreeMap<String, Civilization>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_id: 1,
            introduced: BTreeSet::new(),
            civilizations: BTreeMap::new(),
        }
    }
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
    })
    .collect()
}

impl Registry {
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
        let civilization = Civilization {
            id: id.clone(),
            name: name.into(),
            founder: player.into(),
            members: BTreeSet::from([player.into()]),
            invitations: BTreeSet::new(),
            bell,
            recognition: 0,
            projects: 0,
            stage: Stage::Recognition,
            quests: quests(&id),
            resources: Resources {
                wood: 0,
                food: 0,
                housing: 0,
            },
        };
        self.next_id = next_id;
        self.civilizations.insert(id.clone(), civilization);
        self.clear_invitations(player);
        Ok(id)
    }

    pub fn invite(&mut self, actor: &str, other: &str) -> Result<(), String> {
        player_id(other)?;
        if self.civilization_for(other).is_some() {
            return Err("Игрок уже состоит в цивилизации".into());
        }
        let civ = self.civilization_for_mut(actor)?;
        if civ.founder != actor {
            return Err("Приглашать может только основатель общины".into());
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
        civ.members.insert(player.into());
        self.clear_invitations(player);
        Ok(())
    }

    fn clear_invitations(&mut self, player: &str) {
        for civilization in self.civilizations.values_mut() {
            civilization.invitations.remove(player);
        }
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
        let civ = self.civilization_for_mut(player)?;
        let quest = civ
            .quests
            .iter_mut()
            .find(|q| q.id == id)
            .ok_or("Задание не найдено")?;
        quest.completed_by = Some(player.into());
        civ.recognition += 20;
        civ.projects += 1;
        if civ.recognition >= 60 && civ.projects >= 3 {
            civ.stage = Stage::Elder;
            civ.resources.housing = 6;
        }
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
        let registry: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        registry.validate()?;
        Ok(registry)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION
            || self.next_id == 0
            || self.introduced.len() > MAX_PLAYERS
            || self.civilizations.len() > MAX_CIVILIZATIONS
        {
            return Err("Неподдерживаемое или повреждённое сохранение цивилизаций".into());
        }
        for player in &self.introduced {
            player_id(player)?;
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
                || !civ.members.contains(&civ.founder)
                || bells.iter().any(|b| overlaps(b, &civ.bell))
            {
                return Err("Повреждённые данные общины".into());
            }
            bells.push(civ.bell.clone());
            for member in &civ.members {
                if !self.introduced.contains(member) || !members.insert(member) {
                    return Err("Повторное или неизвестное членство".into());
                }
            }
            for invitation in &civ.invitations {
                player_id(invitation)?;
                if civ.members.contains(invitation) {
                    return Err("Участник не может иметь приглашение в свою общину".into());
                }
            }
            let canonical = quests(id);
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
                {
                    return Err("Несовместимое определение задания".into());
                }
                if let Some(by) = &actual.completed_by {
                    if !civ.members.contains(by) {
                        return Err("Неизвестный исполнитель задания".into());
                    }
                    completed += 1;
                }
            }
            let elder = completed == 3;
            if civ.projects != completed
                || civ.recognition != completed * 20
                || civ.stage
                    != if elder {
                        Stage::Elder
                    } else {
                        Stage::Recognition
                    }
                || civ.resources
                    != (Resources {
                        wood: 0,
                        food: 0,
                        housing: if elder { 6 } else { 0 },
                    })
            {
                return Err("Прогресс не соответствует выполненным заданиям".into());
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
            (60, 3, Stage::Elder)
        );
        assert_eq!(
            civ.resources,
            Resources {
                wood: 0,
                food: 0,
                housing: 6
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
        future["version"] = serde_json::json!(2);
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
}
