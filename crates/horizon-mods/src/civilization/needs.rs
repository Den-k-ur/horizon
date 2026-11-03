//! Versioned, earmarked settlement orders. Received goods belong to an order,
//! not to the future spendable treasury. Observations are supplied by the server.
use super::{Material, Quest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub rules: u32,
    pub population: u16,
    pub beds: u16,
}
impl Plan {
    pub fn new(population: u16, beds: u16) -> Result<Self, String> {
        if !(1..=32).contains(&population) || beds > 4096 {
            return Err("horizon.civilization.quest.invalid_observation".into());
        }
        Ok(Self {
            rules: 1,
            population,
            beds,
        })
    }
    pub fn housing_kits(&self) -> u32 {
        u32::from(self.population + 1).saturating_sub(u32::from(self.beds))
    }
    pub(super) fn valid(&self) -> bool {
        self.rules == 1 && Self::new(self.population, self.beds).is_ok()
    }
    pub(super) fn quests(&self, residents: &[String; 3]) -> Vec<Quest> {
        let mut definitions = vec![
            (
                "quarry",
                "mason",
                vec![
                    ("minecraft:cobblestone", u32::from(self.population) * 8),
                    ("minecraft:stone_pickaxe", 1),
                ],
            ),
            (
                "granary",
                "farmer",
                vec![("minecraft:bread", u32::from(self.population) * 4)],
            ),
        ];
        let kits = self.housing_kits();
        if kits > 0 {
            definitions.push((
                "housing",
                "builder",
                vec![
                    ("minecraft:oak_planks", kits * 3),
                    ("minecraft:white_wool", kits * 3),
                ],
            ));
        }
        definitions
            .into_iter()
            .enumerate()
            .map(|(i, (id, role, materials))| Quest {
                id: id.into(),
                title: format!("horizon.civilization.quest.{id}.title"),
                role: role.into(),
                npc_key: residents[i].clone(),
                materials: materials
                    .into_iter()
                    .map(|(item, count)| Material {
                        item: item.into(),
                        count,
                    })
                    .collect(),
                completed_by: None,
                delivery: Some(Delivery::default()),
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delivery {
    pub received: BTreeMap<String, u32>,
    pub contributors: BTreeSet<String>,
}
impl Quest {
    pub fn remaining(&self) -> Vec<Material> {
        self.materials
            .iter()
            .filter_map(|m| {
                let received = self
                    .delivery
                    .as_ref()
                    .and_then(|d| d.received.get(&m.item))
                    .copied()
                    .unwrap_or(0);
                let count = m.count.saturating_sub(received);
                (count > 0).then(|| Material {
                    item: m.item.clone(),
                    count,
                })
            })
            .collect()
    }
    pub(super) fn valid_delivery(
        &self,
        members: &BTreeSet<String>,
        former: &BTreeSet<String>,
    ) -> bool {
        let Some(d) = &self.delivery else {
            return false;
        };
        if d.received.iter().any(|(item, count)| {
            *count == 0
                || self
                    .materials
                    .iter()
                    .all(|m| m.item != *item || *count > m.count)
        }) || d
            .contributors
            .iter()
            .any(|p| !members.contains(p) && !former.contains(p))
            || d.received.is_empty() != d.contributors.is_empty()
        {
            return false;
        }
        let full = self.remaining().is_empty();
        full == self.completed_by.is_some()
            && self
                .completed_by
                .as_ref()
                .is_none_or(|p| d.contributors.contains(p))
    }
}

impl super::Registry {
    /// Called in the same transaction as a new foundation, never on legacy orders.
    pub fn activate_needs(
        &mut self,
        city_id: &str,
        population: u16,
        beds: u16,
    ) -> Result<(), String> {
        let plan = Plan::new(population, beds)?;
        let city = self
            .civilizations
            .get(city_id)
            .ok_or("horizon.civilization.quest.no_city")?;
        if city.needs.is_some() || city.projects != 0 {
            return Err("horizon.civilization.quest.plan_locked".into());
        }
        let town = self
            .settlements
            .get(&city.settlement_id)
            .ok_or("horizon.civilization.quest.no_city")?;
        let quests = plan.quests(&town.residents);
        let city = self.civilizations.get_mut(city_id).unwrap();
        city.needs = Some(plan);
        city.quests = quests;
        Ok(())
    }

    /// Actual removed stacks from the authoritative inventory adapter; not a client API.
    /// Validate the entire receipt before changing any progress.
    pub fn record_delivery(
        &mut self,
        player: &str,
        id: &str,
        receipt: &[Material],
    ) -> Result<bool, String> {
        let q = self.quest(player, id)?;
        let mut delivery = q
            .delivery
            .clone()
            .ok_or("horizon.civilization.quest.legacy_full_delivery")?;
        if receipt.is_empty() {
            return Err("horizon.civilization.quest.nothing_to_deliver".into());
        }
        let remaining: BTreeMap<_, _> = q
            .remaining()
            .into_iter()
            .map(|m| (m.item, m.count))
            .collect();
        let mut seen = BTreeSet::new();
        for m in receipt {
            if m.count == 0
                || !seen.insert(&m.item)
                || remaining.get(&m.item).is_none_or(|count| m.count > *count)
            {
                return Err("horizon.civilization.quest.invalid_receipt".into());
            }
            *delivery.received.entry(m.item.clone()).or_default() += m.count;
        }
        delivery.contributors.insert(player.into());
        let city = self.civilization_for_mut(player)?;
        if let Some(m) = receipt.iter().find(|m| m.item == "minecraft:bread") {
            city.economy.as_mut().unwrap().credit_bread(m.count)?;
        }
        let q = city.quests.iter_mut().find(|q| q.id == id).unwrap();
        q.delivery = Some(delivery);
        let complete = q.remaining().is_empty();
        if complete {
            q.completed_by = Some(player.into());
            city.recognition += 20;
            city.projects += 1;
        }
        Ok(complete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civilization::{Bell, Registry, Stage, VillageEvidence};
    fn city(beds: u16) -> Registry {
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
            VillageEvidence { beds, residents: 3 },
        )
        .unwrap();
        r.activate_needs("civ-1", 3, beds).unwrap();
        r
    }
    fn material(item: &str, count: u32) -> Material {
        Material {
            item: item.into(),
            count,
        }
    }

    #[test]
    fn orders_follow_population_and_real_bed_deficit() {
        let r = city(2);
        let c = r.civilization("civ-1").unwrap();
        assert_eq!(
            c.quests[0].materials,
            vec![
                material("minecraft:cobblestone", 24),
                material("minecraft:stone_pickaxe", 1)
            ]
        );
        assert_eq!(c.quests[1].materials, vec![material("minecraft:bread", 12)]);
        assert_eq!(
            c.quests[2].materials,
            vec![
                material("minecraft:oak_planks", 6),
                material("minecraft:white_wool", 6)
            ]
        );
        let r = city(4);
        let c = r.civilization("civ-1").unwrap();
        assert_eq!(c.quests.len(), 2);
        assert!(c.quests.iter().all(|q| q.id != "housing"));
        assert!(Plan::new(0, 2).is_err());
    }

    #[test]
    fn partial_cooperative_delivery_survives_departure_and_awards_only_the_last_receipt_once() {
        let mut r = city(2);
        r.begin_player("b").unwrap();
        r.invite("a", "b").unwrap();
        r.join("b", "civ-1").unwrap();
        assert!(
            !r.record_delivery("b", "quarry", &[material("minecraft:cobblestone", 10)])
                .unwrap()
        );
        assert_eq!(r.civilization("civ-1").unwrap().recognition, 0);
        r.leave("b").unwrap();
        let mut r = Registry::restore(&r.save().unwrap()).unwrap();
        assert!(
            r.record_delivery("b", "quarry", &[material("minecraft:stone_pickaxe", 1)])
                .is_err()
        );
        assert!(
            r.record_delivery(
                "a",
                "quarry",
                &[
                    material("minecraft:cobblestone", 14),
                    material("minecraft:stone_pickaxe", 1)
                ]
            )
            .unwrap()
        );
        let c = r.civilization("civ-1").unwrap();
        assert_eq!((c.recognition, c.projects), (20, 1));
        assert_eq!(c.quests[0].completed_by.as_deref(), Some("a"));
        assert_eq!(
            c.quests[0].delivery.as_ref().unwrap().contributors,
            BTreeSet::from(["a".into(), "b".into()])
        );
        let before = r.clone();
        assert!(
            r.record_delivery("a", "quarry", &[material("minecraft:cobblestone", 1)])
                .is_err()
        );
        assert_eq!(r, before);
    }

    #[test]
    fn mixed_invalid_overpaid_or_duplicate_receipts_are_rejected_atomically() {
        let mut r = city(2);
        let before = r.clone();
        for receipt in [
            vec![],
            vec![material("minecraft:cobblestone", 25)],
            vec![
                material("minecraft:cobblestone", 1),
                material("minecraft:diamond", 1),
            ],
            vec![
                material("minecraft:cobblestone", 1),
                material("minecraft:cobblestone", 1),
            ],
            vec![material("minecraft:stone_pickaxe", 0)],
        ] {
            assert!(r.record_delivery("a", "quarry", &receipt).is_err());
            assert_eq!(r, before);
        }
    }

    #[test]
    fn no_unnecessary_housing_order_or_artificial_three_order_gate() {
        let mut r = city(4);
        r.complete_quest("a", "quarry").unwrap();
        r.complete_quest("a", "granary").unwrap();
        assert_eq!(r.civilization("civ-1").unwrap().stage, Stage::Recognition);
        for tick in [0, 100, 200] {
            r.assess(
                "civ-1",
                crate::civilization::governance::Assessment::observation(
                    tick, 3, 4, true, true, true,
                ),
            )
            .unwrap();
        }
        let c = r.civilization("civ-1").unwrap();
        assert_eq!((c.stage, c.recognition, c.projects), (Stage::Elder, 40, 2));
        assert_eq!(c.resources.housing, 4);
        assert_eq!((c.resources.wood, c.resources.food), (0, 0));
        assert_eq!(Registry::restore(&r.save().unwrap()).unwrap(), r);
    }

    #[test]
    fn forged_receipts_need_consistent_demand_contributors_and_progress() {
        let mut r = city(2);
        r.record_delivery("a", "quarry", &[material("minecraft:cobblestone", 4)])
            .unwrap();
        let good = serde_json::to_value(r).unwrap();
        for (field, value) in [
            ("received", serde_json::json!({"minecraft:cobblestone":25})),
            ("contributors", serde_json::json!(["outsider"])),
        ] {
            let mut bad = good.clone();
            bad["civilizations"]["civ-1"]["quests"][0]["delivery"][field] = value;
            assert!(Registry::restore(&serde_json::to_vec(&bad).unwrap()).is_err());
        }
        let mut bad = good;
        bad["civilizations"]["civ-1"]["needs"]["population"] = 4.into();
        assert!(Registry::restore(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
}
