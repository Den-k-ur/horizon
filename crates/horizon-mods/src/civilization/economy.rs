//! Spendable items, separate from immutable order receipts. No currency or idle production.
use super::{Civilization, Registry, Stage};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CAPACITY: u32 = 1_000_000;
const MAX_TOTAL: u64 = 1_000_000_000;
pub const ITEMS: &[(&str, &str)] = &[
    ("minecraft:oak_log", "basic"),
    ("minecraft:oak_planks", "basic"),
    ("minecraft:spruce_log", "basic"),
    ("minecraft:spruce_planks", "basic"),
    ("minecraft:birch_log", "basic"),
    ("minecraft:birch_planks", "basic"),
    ("minecraft:acacia_log", "basic"),
    ("minecraft:acacia_planks", "basic"),
    ("minecraft:cobblestone", "basic"),
    ("minecraft:stone", "basic"),
    ("minecraft:bread", "basic"),
    ("minecraft:wheat", "basic"),
    ("minecraft:white_wool", "basic"),
    ("minecraft:iron_ingot", "industrial"),
    ("minecraft:copper_ingot", "industrial"),
    ("minecraft:coal", "industrial"),
    ("minecraft:charcoal", "industrial"),
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    pub revision: u64,
    /// Actual available bread in v6, imported exactly once; old building orders were spent.
    pub imported_food: u32,
    pub balance: BTreeMap<String, u32>,
    pub deposited: BTreeMap<String, u64>,
    pub withdrawn: BTreeMap<String, u64>,
    pub consumed: BTreeMap<String, u64>,
    pub produced: BTreeMap<String, u64>,
}
#[derive(Clone, Copy)]
pub enum Operation {
    Deposit,
    Withdraw,
    Planks,
}
fn error(key: &str) -> String {
    format!("horizon.civilization.stock.{key}")
}
pub fn supported(item: &str) -> bool {
    ITEMS.iter().any(|(id, _)| *id == item)
}
fn increment(map: &mut BTreeMap<String, u64>, item: &str, count: u32) -> Result<(), String> {
    let value = map
        .get(item)
        .copied()
        .unwrap_or(0)
        .checked_add(u64::from(count))
        .filter(|n| *n <= MAX_TOTAL)
        .ok_or_else(|| error("limit"))?;
    map.insert(item.into(), value);
    Ok(())
}
impl Store {
    pub(super) fn migrate(c: &Civilization) -> Self {
        let food = c.reserved_item("minecraft:bread")
            + c.governance.as_ref().map_or(0, |g| g.reserve_bread);
        let mut s = Self {
            imported_food: food,
            ..Default::default()
        };
        if food > 0 {
            s.balance.insert("minecraft:bread".into(), food);
        }
        s
    }
    pub fn count(&self, item: &str) -> u32 {
        self.balance.get(item).copied().unwrap_or(0)
    }
    fn add(&mut self, item: &str, count: u32) -> Result<(), String> {
        let next = self
            .count(item)
            .checked_add(count)
            .filter(|n| *n <= CAPACITY)
            .ok_or_else(|| error("limit"))?;
        self.balance.insert(item.into(), next);
        Ok(())
    }
    fn take(&mut self, item: &str, count: u32) -> Result<(), String> {
        let next = self
            .count(item)
            .checked_sub(count)
            .ok_or_else(|| error("shortage"))?;
        if next == 0 {
            self.balance.remove(item);
        } else {
            self.balance.insert(item.into(), next);
        }
        Ok(())
    }
    pub(super) fn credit_bread(&mut self, count: u32) -> Result<(), String> {
        let mut next = self.clone();
        next.add("minecraft:bread", count)?;
        increment(&mut next.deposited, "minecraft:bread", count)?;
        next.advance()?;
        *self = next;
        Ok(())
    }
    fn advance(&mut self) -> Result<(), String> {
        self.revision = self
            .revision
            .checked_add(1)
            .filter(|n| *n <= MAX_TOTAL)
            .ok_or_else(|| error("limit"))?;
        Ok(())
    }
    pub(super) fn process_material(
        &mut self,
        input: &str,
        count: u32,
        output: &str,
        amount: u32,
    ) -> Result<(), String> {
        let mut next = self.clone();
        next.take(input, count)?;
        next.add(output, amount)?;
        increment(&mut next.consumed, input, count)?;
        increment(&mut next.produced, output, amount)?;
        next.advance()?;
        *self = next;
        Ok(())
    }
    pub(super) fn consume_rations(&mut self, count: u32) -> Result<(), String> {
        let mut next = self.clone();
        next.take("minecraft:bread", count)?;
        increment(&mut next.consumed, "minecraft:bread", count)?;
        next.advance()?;
        *self = next;
        Ok(())
    }
    pub(super) fn valid(&self) -> bool {
        if self.imported_food > 256 || self.revision > MAX_TOTAL {
            return false;
        }
        if self
            .balance
            .iter()
            .any(|(id, n)| !supported(id) || *n == 0 || *n > CAPACITY)
            || [
                &self.deposited,
                &self.withdrawn,
                &self.consumed,
                &self.produced,
            ]
            .iter()
            .any(|map| {
                map.iter()
                    .any(|(id, n)| !supported(id) || *n == 0 || *n > MAX_TOTAL)
            })
        {
            return false;
        }
        for (id, _) in ITEMS {
            let read = |map: &BTreeMap<String, u64>| map.get(*id).copied().unwrap_or(0);
            let incoming = read(&self.deposited)
                + read(&self.produced)
                + if *id == "minecraft:bread" {
                    u64::from(self.imported_food)
                } else {
                    0
                };
            let outgoing = read(&self.withdrawn) + read(&self.consumed);
            if incoming.checked_sub(outgoing) != Some(u64::from(self.count(id))) {
                return false;
            }
            if read(&self.consumed) > 0
                && !matches!(
                    *id,
                    "minecraft:oak_log" | "minecraft:wheat" | "minecraft:bread"
                )
                || read(&self.produced) > 0
                    && !matches!(*id, "minecraft:oak_planks" | "minecraft:bread")
            {
                return false;
            }
        }
        let wheat = self.consumed.get("minecraft:wheat").copied().unwrap_or(0);
        wheat % 3 == 0
            && self.produced.get("minecraft:bread").copied().unwrap_or(0) == wheat / 3
            && self
                .produced
                .get("minecraft:oak_planks")
                .copied()
                .unwrap_or(0)
                == self.consumed.get("minecraft:oak_log").copied().unwrap_or(0) * 4
    }
}
impl Civilization {
    pub(super) fn reset_food_stability(&mut self) {
        let food = self.food_reserve();
        if let Some(a) = self.governance.as_mut().and_then(|g| g.assessment.as_mut())
            && !a.essentials(food)
        {
            a.stable_samples = 0;
        }
    }
}
impl Registry {
    /// Call only after real inventory removal/space staging, then persist both or roll back.
    /// The acting player's current membership/authority and the form revision are rechecked.
    pub fn stock_operation(
        &mut self,
        player: &str,
        city_id: &str,
        revision: u64,
        operation: Operation,
        item: &str,
        count: u32,
    ) -> Result<(), String> {
        let c = self.civilization_for_mut(player)?;
        if c.id != city_id {
            return Err(error("wrong_city"));
        }
        if !matches!(operation, Operation::Deposit)
            && (c.leader != player || c.stage != Stage::Elder)
        {
            return Err(error("leader_only"));
        }
        if !(1..=64).contains(&count)
            || !supported(item)
            || matches!(operation, Operation::Planks) && (item != "minecraft:oak_log" || count != 1)
        {
            return Err(error("invalid"));
        }
        let old = c.economy.as_ref().ok_or_else(|| error("invalid"))?;
        if old.revision != revision {
            return Err(error("stale"));
        }
        let mut next = old.clone();
        match operation {
            Operation::Deposit => {
                next.add(item, count)?;
                increment(&mut next.deposited, item, count)?;
            }
            Operation::Withdraw => {
                next.take(item, count)?;
                increment(&mut next.withdrawn, item, count)?;
            }
            Operation::Planks => {
                next.take(item, 1)?;
                next.add("minecraft:oak_planks", 4)?;
                increment(&mut next.consumed, item, 1)?;
                increment(&mut next.produced, "minecraft:oak_planks", 4)?;
            }
        }
        next.advance()?;
        c.economy = Some(next);
        c.reset_food_stability();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civilization::{Bell, VillageEvidence, governance::Assessment};
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
                beds: 3,
                residents: 3,
            },
        )
        .unwrap();
        r.activate_needs("civ-1", 3, 3).unwrap();
        r.complete_quest("a", "quarry").unwrap();
        r.complete_quest("a", "granary").unwrap();
        for tick in [100, 200, 300] {
            r.assess(
                "civ-1",
                Assessment::observation(tick, 3, 3, true, true, true),
            )
            .unwrap();
        }
        assert_eq!(r.civilization("civ-1").unwrap().stage, Stage::Elder);
        r
    }
    fn store(r: &Registry) -> &Store {
        r.civilization("civ-1").unwrap().economy.as_ref().unwrap()
    }
    fn run(r: &mut Registry, op: Operation, item: &str, n: u32) {
        r.stock_operation("a", "civ-1", store(r).revision, op, item, n)
            .unwrap();
    }
    #[test]
    fn real_item_accounting_survives_processing_withdrawals_and_reload() {
        let mut r = city();
        for (id, _) in ITEMS {
            run(&mut r, Operation::Deposit, id, 8);
        }
        run(&mut r, Operation::Planks, "minecraft:oak_log", 1);
        run(&mut r, Operation::Withdraw, "minecraft:oak_planks", 10);
        assert_eq!(store(&r).count("minecraft:oak_log"), 7);
        assert_eq!(store(&r).count("minecraft:oak_planks"), 2);
        assert_eq!(store(&r).count("minecraft:bread"), 20);
        assert_eq!(store(&r).consumed["minecraft:oak_log"], 1);
        assert_eq!(store(&r).produced["minecraft:oak_planks"], 4);
        let restored = Registry::restore(&r.save().unwrap()).unwrap();
        assert_eq!(r, restored);
    }
    #[test]
    fn replay_and_competing_forms_cannot_overdraw_or_double_deposit() {
        let mut r = city();
        let rev = store(&r).revision;
        r.stock_operation(
            "a",
            "civ-1",
            rev,
            Operation::Deposit,
            "minecraft:oak_log",
            2,
        )
        .unwrap();
        let before = r.clone();
        assert!(
            r.stock_operation(
                "a",
                "civ-1",
                rev,
                Operation::Deposit,
                "minecraft:oak_log",
                2
            )
            .is_err()
        );
        assert_eq!(r, before);
        let rev = store(&r).revision;
        r.stock_operation(
            "a",
            "civ-1",
            rev,
            Operation::Withdraw,
            "minecraft:oak_log",
            2,
        )
        .unwrap();
        let before = r.clone();
        assert!(
            r.stock_operation(
                "a",
                "civ-1",
                rev,
                Operation::Withdraw,
                "minecraft:oak_log",
                2
            )
            .is_err()
        );
        assert_eq!(r, before);
        assert_eq!(store(&r).count("minecraft:oak_log"), 0);
    }
    #[test]
    fn membership_and_current_title_authorize_each_operation() {
        let mut r = city();
        r.begin_player("b").unwrap();
        r.invite("a", "b").unwrap();
        r.join("b", "civ-1").unwrap();
        let rev = store(&r).revision;
        r.stock_operation("b", "civ-1", rev, Operation::Deposit, "minecraft:stone", 2)
            .unwrap();
        let rev = store(&r).revision;
        let before = r.clone();
        assert!(
            r.stock_operation("b", "civ-1", rev, Operation::Withdraw, "minecraft:stone", 1)
                .is_err()
        );
        assert!(
            r.stock_operation("a", "civ-2", rev, Operation::Withdraw, "minecraft:stone", 1)
                .is_err()
        );
        assert!(
            r.stock_operation(
                "outsider",
                "civ-1",
                rev,
                Operation::Deposit,
                "minecraft:stone",
                1
            )
            .is_err()
        );
        assert_eq!(r, before);
        r.transfer_leadership("a", "b").unwrap();
        assert!(
            r.stock_operation("a", "civ-1", rev, Operation::Withdraw, "minecraft:stone", 1)
                .is_err()
        );
        r.stock_operation("b", "civ-1", rev, Operation::Withdraw, "minecraft:stone", 1)
            .unwrap();
    }
    #[test]
    fn allocation_affects_live_food_without_erasing_receipts_or_earned_title() {
        let mut r = city();
        let history = r.civilization("civ-1").unwrap().quests.clone();
        run(&mut r, Operation::Withdraw, "minecraft:bread", 4);
        let c = r.civilization("civ-1").unwrap();
        assert_eq!(c.food_reserve(), 8);
        assert_eq!(c.metrics().unwrap().stability, 0);
        assert_eq!(c.stage, Stage::Elder);
        assert_eq!(c.quests, history);
        assert!(r.save().is_ok());
        r.provision_bread("a", 4).unwrap();
        assert_eq!(r.civilization("civ-1").unwrap().food_reserve(), 12);
        assert_eq!(r.civilization("civ-1").unwrap().quests, history);
        let before = r.clone();
        assert!(r.complete_quest("a", "granary").is_err());
        assert_eq!(r, before);
    }
    #[test]
    fn v6_imports_only_available_food_once_and_rejects_injected_or_unbalanced_stock() {
        let r = city();
        let mut old = serde_json::to_value(&r).unwrap();
        old["version"] = 6.into();
        old["civilizations"]["civ-1"]
            .as_object_mut()
            .unwrap()
            .remove("population");
        old["civilizations"]["civ-1"]
            .as_object_mut()
            .unwrap()
            .remove("economy");
        let mut restored = Registry::restore(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(store(&restored).imported_food, 12);
        assert_eq!(store(&restored).count("minecraft:cobblestone"), 0);
        run(&mut restored, Operation::Withdraw, "minecraft:bread", 5);
        for _ in 0..3 {
            restored = Registry::restore(&restored.save().unwrap()).unwrap();
        }
        assert_eq!(store(&restored).count("minecraft:bread"), 7);
        old["civilizations"]["civ-1"]["economy"] = serde_json::to_value(Store::default()).unwrap();
        assert!(Registry::restore(&serde_json::to_vec(&old).unwrap()).is_err());
        let mut forged = serde_json::to_value(&restored).unwrap();
        forged["civilizations"]["civ-1"]["economy"]["balance"]["minecraft:bread"] = 8.into();
        assert!(Registry::restore(&serde_json::to_vec(&forged).unwrap()).is_err());
    }
    #[test]
    fn invalid_amounts_shortage_and_capacity_failure_leave_model_unchanged() {
        let mut r = city();
        for (op, item, n) in [
            (Operation::Deposit, "minecraft:diamond", 1),
            (Operation::Deposit, "minecraft:bread", 0),
            (Operation::Deposit, "minecraft:bread", 65),
            (Operation::Withdraw, "minecraft:stone", 1),
            (Operation::Planks, "minecraft:oak_log", 1),
            (Operation::Planks, "minecraft:bread", 1),
        ] {
            let before = r.clone();
            assert!(
                r.stock_operation("a", "civ-1", store(&r).revision, op, item, n)
                    .is_err()
            );
            assert_eq!(r, before);
        }
        // A valid full ledger can be restored, but no further item can be deposited.
        let s = r
            .civilizations
            .get_mut("civ-1")
            .unwrap()
            .economy
            .as_mut()
            .unwrap();
        s.balance.insert("minecraft:stone".into(), CAPACITY);
        s.deposited
            .insert("minecraft:stone".into(), u64::from(CAPACITY));
        assert!(r.save().is_ok());
        let before = r.clone();
        assert!(
            r.stock_operation(
                "a",
                "civ-1",
                store(&r).revision,
                Operation::Deposit,
                "minecraft:stone",
                1
            )
            .is_err()
        );
        assert_eq!(r, before);
    }
}
