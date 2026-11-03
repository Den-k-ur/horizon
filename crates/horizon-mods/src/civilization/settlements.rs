//! Persistent settlements exist before civilizations and retain their own citizen identities.
use super::{Bell, races::Race};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RADIUS: i32 = 10;
pub const ROLES: [&str; 3] = ["quarry", "granary", "housing"];
/// Deliberately small terrain allow-list. Structures, containers, fluids and trees are protected.
pub fn natural(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:grass_block"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:stone"
            | "minecraft:sand"
            | "minecraft:sandstone"
            | "minecraft:short_grass"
            | "minecraft:tall_grass"
            | "minecraft:short_dry_grass"
            | "minecraft:tall_dry_grass"
            | "minecraft:dead_bush"
            | "minecraft:dandelion"
            | "minecraft:poppy"
    )
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Generated,
    Adopted,
    Legacy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Building,
    Ready,
    PotentialCapital,
    Blocked,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settlement {
    pub id: String,
    pub bell: Bell,
    pub race: Option<Race>,
    pub biome: String,
    pub kind: Kind,
    pub status: Status,
    pub claimed_by: Option<String>,
    pub npc_owner: String,
    pub npc_city: String,
    pub residents: [String; 3],
    pub blueprint: u32,
    pub building_started: bool,
}
impl Settlement {
    pub(super) fn new(
        id: String,
        bell: Bell,
        race: Option<Race>,
        biome: String,
        kind: Kind,
    ) -> Self {
        Self {
            npc_owner: format!("civilization.{id}"),
            npc_city: id.clone(),
            residents: ROLES.map(|role| format!("{id}.{role}")),
            id,
            bell,
            race,
            biome,
            kind,
            status: if kind == Kind::Generated {
                Status::Building
            } else {
                Status::Ready
            },
            claimed_by: None,
            blueprint: u32::from(kind == Kind::Generated),
            building_started: false,
        }
    }
    pub(super) fn legacy(city: &super::Civilization) -> Self {
        let mut town = Self::new(
            format!("legacy-{}", city.id),
            city.bell.clone(),
            None,
            String::new(),
            Kind::Legacy,
        );
        town.status = Status::PotentialCapital;
        town.claimed_by = Some(city.id.clone());
        town.npc_owner = "civilization".into();
        town.npc_city = city.id.clone();
        town.residents = ROLES.map(|role| format!("{}.{role}", city.id));
        town
    }
    pub(super) fn valid(&self) -> bool {
        if !super::valid_bell(&self.bell) {
            return false;
        }
        if self.claimed_by.is_some() != (self.status == Status::PotentialCapital) {
            return false;
        }
        if self.kind == Kind::Legacy {
            let Some(city) = &self.claimed_by else {
                return false;
            };
            return self.id == format!("legacy-{city}")
                && self.npc_owner == "civilization"
                && self.npc_city == *city
                && self.residents == ROLES.map(|r| format!("{city}.{r}"))
                && self.blueprint == 0
                && !self.building_started;
        }
        self.npc_owner == format!("civilization.{}", self.id)
            && self.npc_city == self.id
            && self.residents == ROLES.map(|r| format!("{}.{r}", self.id))
            && match self.kind {
                Kind::Generated => {
                    self.blueprint == 1
                        && (64..=287).contains(&self.bell.y)
                        && self.race.is_some_and(|r| r.habitat(&self.biome))
                        && (self.status != Status::Ready && self.status != Status::PotentialCapital
                            || self.building_started)
                }
                Kind::Adopted => self.blueprint == 0 && !self.building_started,
                Kind::Legacy => false,
            }
    }
}

/// Original, versioned starting camp: three shelters, beds, a square and a bell.
/// Building settlements can replay this patch; Ready settlements are never rebuilt.
pub fn blueprint(town: &Settlement) -> BTreeMap<[i32; 3], &'static str> {
    let [bx, by, bz] = [town.bell.x, town.bell.y, town.bell.z];
    let desert = town.race == Some(Race::DesertCat);
    let floor = if desert {
        "minecraft:cut_sandstone"
    } else {
        "minecraft:cobblestone"
    };
    let wall = if desert {
        "minecraft:smooth_sandstone"
    } else {
        "minecraft:oak_planks"
    };
    let roof = if desert {
        "minecraft:sandstone"
    } else {
        "minecraft:oak_log"
    };
    let mut cells = BTreeMap::new();
    for x in -RADIUS..=RADIUS {
        for z in -RADIUS..=RADIUS {
            for y in -2..=4 {
                cells.insert(
                    [bx + x, by + y, bz + z],
                    if y < 0 { floor } else { "minecraft:air" },
                );
            }
        }
    }
    for (x, z, north_door) in [(-8, -8, false), (4, -8, false), (-2, 4, true)] {
        for dx in 0..5 {
            for dz in 0..5 {
                for y in 0..3 {
                    let edge = dx == 0 || dx == 4 || dz == 0 || dz == 4;
                    cells.insert(
                        [bx + x + dx, by + y, bz + z + dz],
                        if edge { wall } else { "minecraft:air" },
                    );
                }
                cells.insert([bx + x + dx, by + 3, bz + z + dz], roof);
            }
        }
        let door_z = z + if north_door { 0 } else { 4 };
        for y in 0..2 {
            cells.insert([bx + x + 2, by + y, bz + door_z], "minecraft:air");
        }
        cells.insert(
            [bx + x + 1, by, bz + z + 1],
            "minecraft:red_bed[facing=south,part=foot,occupied=false]",
        );
        cells.insert(
            [bx + x + 1, by, bz + z + 2],
            "minecraft:red_bed[facing=south,part=head,occupied=false]",
        );
    }
    cells.insert(
        [bx, by, bz],
        "minecraft:bell[attachment=floor,facing=north,powered=false]",
    );
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civilization::{Registry, Stage, VillageEvidence, races::Start};
    fn start(x: i32) -> Start {
        Start {
            settlement: Bell {
                dimension: "minecraft:overworld".into(),
                x,
                y: 70,
                z: 0,
            },
            structure: "civilization:settlement".into(),
            position: [x, 70, -2],
            biome: "minecraft:plains".into(),
        }
    }
    fn evidence() -> VillageEvidence {
        VillageEvidence {
            beds: 3,
            residents: 3,
        }
    }

    #[test]
    fn unclaimed_settlement_becomes_capital_without_replacing_residents_or_granting_elder() {
        let mut state = Registry::default();
        for p in ["a", "b"] {
            state.begin_player(p).unwrap();
            state.choose_race(p, Race::Human, start(0)).unwrap();
        }
        assert_eq!(state.settlements().count(), 1);
        assert!(
            state
                .found("a", "A", start(0).settlement, evidence())
                .is_err()
        );
        state.start_building("town-1");
        state.finish_building("town-1");
        let before = state.settlement("town-1").unwrap().clone();
        state
            .found("a", "A", start(0).settlement, evidence())
            .unwrap();
        let town = state.settlement("town-1").unwrap();
        assert_eq!(town.residents, before.residents);
        assert_eq!(town.npc_owner, before.npc_owner);
        assert_eq!(town.status, Status::PotentialCapital);
        let city = state.civilization("civ-1").unwrap();
        assert_eq!(
            (city.stage, city.recognition, city.projects),
            (Stage::Recognition, 0, 0)
        );
        assert_eq!(city.founder, "a");
        assert_eq!(city.quests[0].npc_key, before.residents[0]);
        assert!(
            state
                .found("b", "B", start(0).settlement, evidence())
                .is_err()
        );
        assert_eq!(Registry::restore(&state.save().unwrap()).unwrap(), state);
    }

    #[test]
    fn blocked_retry_retains_fixed_race_and_never_rebuilds_a_finished_town() {
        let mut state = Registry::default();
        state.begin_player("a").unwrap();
        state.choose_race("a", Race::Human, start(0)).unwrap();
        state.block_settlement("town-1");
        assert!(state.choose_race("a", Race::DesertCat, start(300)).is_err());
        state.choose_race("a", Race::Human, start(300)).unwrap();
        state.start_building("town-2");
        state.finish_building("town-2");
        state.block_settlement("town-2");
        assert_eq!(state.settlement("town-2").unwrap().status, Status::Ready);
        assert_eq!(state.compass_target("a"), Some(&start(300).settlement));
        assert_eq!(Registry::restore(&state.save().unwrap()).unwrap(), state);
    }

    #[test]
    fn legacy_versions_keep_authority_quests_npc_keys_and_never_request_construction() {
        let mut state = Registry::default();
        state.begin_player("a").unwrap();
        state
            .found("a", "A", start(0).settlement, evidence())
            .unwrap();
        state.complete_quest("a", "quarry").unwrap();
        for version in 1..=3 {
            let mut legacy = serde_json::to_value(&state).unwrap();
            legacy["version"] = version.into();
            let obj = legacy.as_object_mut().unwrap();
            obj.remove("settlements");
            obj.remove("next_settlement");
            obj.remove("choices");
            let city = legacy["civilizations"]["civ-1"].as_object_mut().unwrap();
            city.remove("settlement_id");
            city.remove("governance");
            city.remove("economy");
            city.remove("population");
            if version == 1 {
                city.remove("leader");
                city.remove("former_members");
            }
            for q in city["quests"].as_array_mut().unwrap() {
                q["npc_key"] = format!("civ-1.{}", q["id"].as_str().unwrap()).into();
            }
            let restored = Registry::restore(&serde_json::to_vec(&legacy).unwrap()).unwrap();
            let town = restored.settlement("legacy-civ-1").unwrap();
            assert_eq!(town.kind, Kind::Legacy);
            assert!(!town.building_started);
            assert_eq!(town.npc_owner, "civilization");
            let city = restored.civilization("civ-1").unwrap();
            assert_eq!(city.leader, "a");
            assert_eq!(city.recognition, 20);
            assert_eq!(city.quests[0].npc_key, "civ-1.quarry");
            assert_eq!(city.quests[0].completed_by.as_deref(), Some("a"));
        }
    }

    #[test]
    fn forged_citizen_owner_claim_and_blueprint_cannot_load() {
        let mut state = Registry::default();
        state.begin_player("a").unwrap();
        state.choose_race("a", Race::Human, start(0)).unwrap();
        let valid = serde_json::to_value(&state).unwrap();
        for (field, value) in [
            ("npc_owner", serde_json::json!("other")),
            ("blueprint", serde_json::json!(99)),
            ("claimed_by", serde_json::json!("missing")),
            ("status", serde_json::json!("ready")),
        ] {
            let mut bad = valid.clone();
            bad["settlements"]["town-1"][field] = value;
            assert!(Registry::restore(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
    }

    #[test]
    fn old_race_choices_bind_members_without_replaying_completed_arrivals() {
        let mut state = Registry::default();
        for p in ["a", "b", "c"] {
            state.begin_player(p).unwrap();
        }
        state
            .found("a", "A", start(0).settlement, evidence())
            .unwrap();
        for (p, x) in [("a", 300), ("b", 600), ("c", 900)] {
            let mut s = start(x);
            s.biome = "minecraft:desert".into();
            s.structure = "minecraft:village_desert".into();
            state.choose_race(p, Race::DesertCat, s).unwrap();
        }
        state.finish_race_arrival("a");
        state.finish_race_arrival("c");
        let mut old = serde_json::to_value(state).unwrap();
        old["version"] = 3.into();
        old.as_object_mut().unwrap().remove("settlements");
        old.as_object_mut().unwrap().remove("next_settlement");
        let city = old["civilizations"]["civ-1"].as_object_mut().unwrap();
        city.remove("settlement_id");
        city.remove("governance");
        city.remove("economy");
        city.remove("population");
        for q in city["quests"].as_array_mut().unwrap() {
            q["npc_key"] = format!("civ-1.{}", q["id"].as_str().unwrap()).into();
        }
        for choice in old["choices"].as_object_mut().unwrap().values_mut() {
            choice.as_object_mut().unwrap().remove("settlement_id");
        }
        let mut restored = Registry::restore(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(
            restored.race_choice("a").unwrap().settlement_id.as_deref(),
            Some("legacy-civ-1")
        );
        assert!(!restored.race_choice("a").unwrap().arriving);
        assert!(restored.race_choice("b").unwrap().arriving);
        assert!(restored.race_choice("b").unwrap().settlement_id.is_none());
        assert!(!restored.race_choice("c").unwrap().arriving);
        assert!(restored.race_choice("c").unwrap().settlement_id.is_none());
        let mut s = start(1200);
        s.biome = "minecraft:desert".into();
        restored.choose_race("c", Race::DesertCat, s).unwrap();
        assert!(!restored.race_choice("c").unwrap().arriving);
        assert_eq!(
            Registry::restore(&restored.save().unwrap()).unwrap(),
            restored
        );
    }
}
