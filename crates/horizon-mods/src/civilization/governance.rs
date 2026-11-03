//! Recognition is earned from help; the title additionally requires observed essentials.
use super::{Civilization, Registry, Stage};
use serde::{Deserialize, Serialize};

pub const RECOGNITION_REQUIRED: u32 = 40;
pub const SAMPLE_INTERVAL: i64 = 100;
pub const STABLE_SAMPLES: u8 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub tick: i64,
    pub sample_tick: i64,
    pub population: u16,
    pub beds: u16,
    pub bell: bool,
    pub crafting_table: bool,
    pub furnace: bool,
    pub stable_samples: u8,
}
impl Assessment {
    pub fn observation(
        tick: i64,
        population: u16,
        beds: u16,
        bell: bool,
        crafting_table: bool,
        furnace: bool,
    ) -> Self {
        Self {
            tick,
            sample_tick: tick,
            population,
            beds,
            bell,
            crafting_table,
            furnace,
            stable_samples: 0,
        }
    }
    fn valid(&self) -> bool {
        self.tick >= 0
            && (0..=self.tick).contains(&self.sample_tick)
            && self.population <= 32
            && self.beds <= 4096
            && self.stable_samples <= STABLE_SAMPLES
    }
    pub fn food_target(&self) -> u32 {
        u32::from(self.population) * 4
    }
    pub fn essentials(&self, food: u32) -> bool {
        self.population > 0
            && self.bell
            && self.crafting_table
            && self.furnace
            && self.beds >= self.population
            && food >= self.food_target()
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grant {
    pub observation: Assessment,
    pub food: u32,
    pub recognition: u32,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Governance {
    pub rules: u32,
    pub legacy_title: bool,
    pub reserve_bread: u32,
    pub assessment: Option<Assessment>,
    pub grant: Option<Grant>,
}
impl Default for Governance {
    fn default() -> Self {
        Self {
            rules: 1,
            legacy_title: false,
            reserve_bread: 0,
            assessment: None,
            grant: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Metrics {
    pub recognition: u32,
    pub wellbeing: u32,
    pub development: u32,
    pub stability: u32,
    pub food: u32,
    pub food_target: u32,
    pub population: u16,
    pub beds: u16,
    pub housing_free: u16,
    pub housing_occupied: u16,
    pub observed_at: i64,
}
fn coverage(have: u32, need: u32) -> u32 {
    if need == 0 {
        0
    } else {
        (have.saturating_mul(100) / need).min(100)
    }
}
impl Civilization {
    pub fn reserved_item(&self, item: &str) -> u32 {
        self.quests
            .iter()
            .filter_map(|q| q.delivery.as_ref())
            .filter_map(|d| d.received.get(item))
            .copied()
            .sum()
    }
    pub fn food_reserve(&self) -> u32 {
        self.economy.as_ref().map_or_else(
            || {
                self.reserved_item("minecraft:bread")
                    + self.governance.as_ref().map_or(0, |g| g.reserve_bread)
            },
            |s| s.count("minecraft:bread"),
        )
    }
    pub fn metrics(&self) -> Option<Metrics> {
        let a = self.governance.as_ref()?.assessment.as_ref()?;
        let food = self.food_reserve();
        let housing = coverage(u32::from(a.beds), u32::from(a.population));
        Some(Metrics {
            recognition: self.recognition,
            wellbeing: (coverage(food, a.food_target()) + housing) / 2,
            development: (u32::from(a.bell) * 100
                + housing
                + u32::from(a.crafting_table) * 100
                + u32::from(a.furnace) * 100)
                / 4,
            stability: u32::from(a.stable_samples) * 100 / u32::from(STABLE_SAMPLES),
            food,
            food_target: a.food_target(),
            population: a.population,
            beds: a.beds,
            housing_free: a.beds.saturating_sub(a.population),
            housing_occupied: a.beds.min(a.population),
            observed_at: a.tick,
        })
    }
    pub(super) fn valid_governance(&self) -> bool {
        let Some(g) = &self.governance else {
            return false;
        };
        if g.rules != 1 || g.reserve_bread > 128 {
            return false;
        }
        if let Some(a) = &g.assessment {
            if !a.valid() || a.stable_samples > 0 && !a.essentials(self.food_reserve()) {
                return false;
            }
        }
        if let Some(grant) = &g.grant {
            if g.legacy_title
                || !grant.observation.valid()
                || grant.observation.stable_samples != STABLE_SAMPLES
                || !grant.observation.essentials(grant.food)
                || grant.food > super::economy::CAPACITY
                || grant.recognition < RECOGNITION_REQUIRED
                || grant.recognition > self.recognition
            {
                return false;
            }
        }
        if g.legacy_title
            && (g.grant.is_some() || self.quests.iter().any(|q| q.completed_by.is_none()))
        {
            return false;
        }
        let elder = g.legacy_title || g.grant.is_some();
        self.stage
            == (if elder {
                Stage::Elder
            } else {
                Stage::Recognition
            })
            && self.resources.housing
                == if !elder {
                    0
                } else if let Some(grant) = &g.grant {
                    u32::from(grant.observation.beds)
                } else {
                    self.needs.as_ref().map_or(6, |p| u32::from(p.beds))
                }
    }
}
impl Registry {
    /// Only authoritative world observations. Caller persists this with the resulting title.
    pub fn assess(&mut self, id: &str, mut observation: Assessment) -> Result<bool, String> {
        if !observation.valid() || observation.stable_samples != 0 {
            return Err("horizon.civilization.council.invalid_observation".into());
        }
        let c = self
            .civilizations
            .get_mut(id)
            .ok_or("horizon.civilization.quest.no_city")?;
        let food = c.food_reserve();
        let g = c
            .governance
            .as_mut()
            .ok_or("horizon.civilization.council.invalid_observation")?;
        if observation.essentials(food) {
            observation.stable_samples = 1;
            if let Some(old) = &g.assessment
                && old.essentials(food)
                && old.tick <= observation.tick
            {
                if observation.tick.saturating_sub(old.sample_tick) >= SAMPLE_INTERVAL {
                    observation.stable_samples =
                        old.stable_samples.saturating_add(1).min(STABLE_SAMPLES);
                } else {
                    observation.stable_samples = old.stable_samples;
                    observation.sample_tick = old.sample_tick;
                }
            }
        }
        let granted = c.stage != Stage::Elder
            && c.recognition >= RECOGNITION_REQUIRED
            && observation.stable_samples == STABLE_SAMPLES;
        if granted {
            g.grant = Some(Grant {
                observation: observation.clone(),
                food,
                recognition: c.recognition,
            });
            c.stage = Stage::Elder;
            c.resources.housing = u32::from(observation.beds);
        }
        g.assessment = Some(observation);
        Ok(granted)
    }
    pub fn provision_bread(&mut self, player: &str, count: u32) -> Result<(), String> {
        let c = self.civilization_for_mut(player)?;
        let target = c
            .governance
            .as_ref()
            .and_then(|g| g.assessment.as_ref())
            .ok_or("horizon.civilization.council.no_survey")?
            .food_target();
        let missing = target.saturating_sub(c.food_reserve());
        if count == 0 || count > missing {
            return Err("horizon.civilization.quest.invalid_receipt".into());
        }
        c.economy.as_mut().unwrap().credit_bread(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civilization::{Bell, VillageEvidence};
    fn city() -> Registry {
        let mut r = Registry::default();
        r.begin_player("a").unwrap();
        r.found(
            "a",
            "A",
            Bell {
                dimension: "minecraft:overworld".into(),
                x: 0,
                y: 70,
                z: 0,
            },
            VillageEvidence {
                beds: 2,
                residents: 3,
            },
        )
        .unwrap();
        r.activate_needs("civ-1", 3, 2).unwrap();
        r
    }
    fn observation(tick: i64, beds: u16) -> Assessment {
        Assessment::observation(tick, 3, beds, true, true, true)
    }
    fn help(r: &mut Registry) {
        r.complete_quest("a", "quarry").unwrap();
        r.complete_quest("a", "granary").unwrap();
    }

    #[test]
    fn supplies_and_recognition_alone_do_not_grant_a_title() {
        let mut r = city();
        help(&mut r);
        r.complete_quest("a", "housing").unwrap();
        for tick in [0, 100, 200, 300] {
            assert!(!r.assess("civ-1", observation(tick, 2)).unwrap());
        }
        let c = r.civilization("civ-1").unwrap();
        assert_eq!(c.stage, Stage::Recognition);
        let m = c.metrics().unwrap();
        assert_eq!((m.wellbeing, m.development, m.stability), (83, 91, 0));
        assert_eq!(
            (m.food, m.food_target, m.population, m.beds),
            (12, 12, 3, 2)
        );
    }

    #[test]
    fn repeated_requests_cannot_accelerate_stability_and_optional_order_is_not_a_gate() {
        let mut r = city();
        help(&mut r);
        for tick in 0..100 {
            assert!(!r.assess("civ-1", observation(tick, 3)).unwrap());
        }
        assert_eq!(
            r.civilization("civ-1")
                .unwrap()
                .metrics()
                .unwrap()
                .stability,
            33
        );
        assert!(!r.assess("civ-1", observation(100, 3)).unwrap());
        let mut r = Registry::restore(&r.save().unwrap()).unwrap();
        assert!(!r.assess("civ-1", observation(199, 3)).unwrap());
        assert!(r.assess("civ-1", observation(200, 3)).unwrap());
        let c = r.civilization("civ-1").unwrap();
        assert_eq!((c.stage, c.recognition, c.projects), (Stage::Elder, 40, 2));
        assert!(c.quests[2].completed_by.is_none());
        assert_eq!(c.resources.housing, 3);
        assert!(!r.assess("civ-1", observation(300, 3)).unwrap());
        assert_eq!(Registry::restore(&r.save().unwrap()).unwrap(), r);
    }

    #[test]
    fn a_crisis_resets_confirmation_but_does_not_erase_an_earned_title() {
        let mut r = city();
        help(&mut r);
        r.assess("civ-1", observation(0, 3)).unwrap();
        r.assess("civ-1", observation(100, 3)).unwrap();
        let mut bad = observation(200, 3);
        bad.furnace = false;
        assert!(!r.assess("civ-1", bad.clone()).unwrap());
        for tick in [300, 400] {
            assert!(!r.assess("civ-1", observation(tick, 3)).unwrap());
        }
        assert!(r.assess("civ-1", observation(500, 3)).unwrap());
        bad.tick = 600;
        bad.sample_tick = 600;
        r.assess("civ-1", bad).unwrap();
        let c = r.civilization("civ-1").unwrap();
        assert_eq!(c.stage, Stage::Elder);
        assert_eq!(c.metrics().unwrap().stability, 0);
        assert_eq!(Registry::restore(&r.save().unwrap()).unwrap(), r);
    }

    #[test]
    fn provisions_are_bounded_and_do_not_mint_recognition_or_double_count_food() {
        let mut r = city();
        assert!(r.provision_bread("a", 1).is_err());
        r.assess("civ-1", observation(0, 3)).unwrap();
        r.provision_bread("a", 8).unwrap();
        let before = r.clone();
        assert!(r.provision_bread("a", 5).is_err());
        assert_eq!(r, before);
        r.provision_bread("a", 4).unwrap();
        assert!(r.provision_bread("a", 1).is_err());
        let c = r.civilization("civ-1").unwrap();
        assert_eq!((c.food_reserve(), c.recognition, c.projects), (12, 0, 0));
        assert_eq!((c.resources.food, c.resources.wood), (0, 0));
        assert!(r.provision_bread("outsider", 1).is_err());
    }

    #[test]
    fn migration_keeps_a_legitimate_old_title_but_rejects_smuggled_authority() {
        let mut r = city();
        for id in ["quarry", "granary", "housing"] {
            r.complete_quest("a", id).unwrap();
        }
        let mut old = serde_json::to_value(&r).unwrap();
        old["version"] = 5.into();
        let c = old["civilizations"]["civ-1"].as_object_mut().unwrap();
        c.remove("governance");
        c.remove("economy");
        c.remove("population");
        c["stage"] = "elder".into();
        c["resources"]["housing"] = 2.into();
        let restored = Registry::restore(&serde_json::to_vec(&old).unwrap()).unwrap();
        let c = restored.civilization("civ-1").unwrap();
        assert_eq!(c.stage, Stage::Elder);
        assert!(c.governance.as_ref().unwrap().legacy_title);
        assert!(c.metrics().is_none());
        old["civilizations"]["civ-1"]["quests"][0]["completed_by"] = serde_json::Value::Null;
        assert!(Registry::restore(&serde_json::to_vec(&old).unwrap()).is_err());
        let mut forged = serde_json::to_value(city()).unwrap();
        forged["civilizations"]["civ-1"]["stage"] = "elder".into();
        assert!(Registry::restore(&serde_json::to_vec(&forged).unwrap()).is_err());
    }
}
