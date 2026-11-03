//! Persistent occupations and active-settlement needs. No resource nodes or idle income.
use super::{Bell, Civilization, Registry, Stage};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const WORK_TICKS: u32 = 200;
pub const MEAL_TICKS: u32 = 24_000;
const LIMIT: u64 = 1_000_000_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Profession {
    Baker,
    Carpenter,
}
impl Profession {
    pub fn key(self) -> &'static str {
        match self {
            Self::Baker => "baker",
            Self::Carpenter => "carpenter",
        }
    }
    pub fn input(self) -> (&'static str, u32) {
        match self {
            Self::Baker => ("minecraft:wheat", 3),
            Self::Carpenter => ("minecraft:oak_log", 1),
        }
    }
    pub fn output(self) -> (&'static str, u32) {
        match self {
            Self::Baker => ("minecraft:bread", 1),
            Self::Carpenter => ("minecraft:oak_planks", 4),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub profession: Profession,
    pub station: [i32; 3],
}
impl Job {
    pub fn in_territory(&self, b: &Bell) -> bool {
        let dx = i64::from(self.station[0]) - i64::from(b.x);
        let dz = i64::from(self.station[2]) - i64::from(b.z);
        dx.abs() <= 32
            && dz.abs() <= 32
            && dx * dx + dz * dz <= 32 * 32
            && (i64::from(self.station[1]) - i64::from(b.y)).abs() <= 12
            && (-64..=319).contains(&self.station[1])
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Idle,
    Walking,
    Working,
    NoMaterials,
    NoHousing,
    Hungry,
    Unavailable,
    NoWorkplace,
    Blocked,
    Full,
    Unknown,
}
impl Status {
    pub fn key(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Walking => "walking",
            Self::Working => "working",
            Self::NoMaterials => "no_materials",
            Self::NoHousing => "no_housing",
            Self::Hungry => "hungry",
            Self::Unavailable => "unavailable",
            Self::NoWorkplace => "no_workplace",
            Self::Blocked => "blocked",
            Self::Full => "full",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worker {
    pub job: Option<Job>,
    pub progress: u32,
    pub completed: u64,
    pub status: Status,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Population {
    /// Configuration revision; ordinary simulation does not invalidate a form every second.
    pub revision: u64,
    pub workers: BTreeMap<String, Worker>,
    pub last_tick: Option<i64>,
    pub active_ticks: u64,
    pub meal_in: u32,
    pub hungry: bool,
    pub rations: u64,
    pub baked: u64,
    pub carpentered: u64,
    pub observed_population: u16,
    pub observed_beds: u16,
}
#[derive(Clone, Copy, Default)]
pub struct Observation {
    pub available: bool,
    pub workplace: bool,
    pub arrived: bool,
    pub blocked: bool,
}
impl Population {
    pub(super) fn new(keys: &[String; 3]) -> Self {
        Self {
            revision: 0,
            workers: keys
                .iter()
                .map(|key| {
                    (
                        key.clone(),
                        Worker {
                            job: None,
                            progress: 0,
                            completed: 0,
                            status: Status::Idle,
                        },
                    )
                })
                .collect(),
            last_tick: None,
            active_ticks: 0,
            meal_in: MEAL_TICKS,
            hungry: false,
            rations: 0,
            baked: 0,
            carpentered: 0,
            observed_population: 0,
            observed_beds: 0,
        }
    }
    pub(super) fn valid(&self, c: &Civilization, keys: &[String; 3]) -> bool {
        let Some(s) = &c.economy else { return false };
        if self.workers.len() != 3
            || keys.iter().any(|k| !self.workers.contains_key(k))
            || self.revision > LIMIT
            || self.active_ticks > LIMIT
            || self.rations > LIMIT
            || self.baked > LIMIT
            || self.carpentered > LIMIT
            || self.last_tick.is_some_and(|t| t < 0)
            || self.meal_in > MEAL_TICKS
            || self.hungry && self.meal_in != 0
            || self.observed_population > 3
            || self.observed_beds > 4096
        {
            return false;
        }
        let mut sites = BTreeSet::new();
        let mut completed = 0;
        for w in self.workers.values() {
            if w.progress > WORK_TICKS || w.completed > LIMIT {
                return false;
            }
            completed += w.completed;
            if let Some(j) = &w.job {
                if !j.in_territory(&c.bell) || !sites.insert(j.station) {
                    return false;
                }
            } else if w.progress != 0 {
                return false;
            }
        }
        completed == self.baked + self.carpentered
            && self.rations == s.consumed.get("minecraft:bread").copied().unwrap_or(0)
            && self.baked == s.produced.get("minecraft:bread").copied().unwrap_or(0)
            && self.carpentered <= s.consumed.get("minecraft:oak_log").copied().unwrap_or(0)
    }
}
impl Registry {
    pub fn assign_worker(
        &mut self,
        player: &str,
        city: &str,
        revision: u64,
        key: &str,
        job: Option<Job>,
    ) -> Result<(), String> {
        let c = self.civilization_for_mut(player)?;
        if c.id != city || c.leader != player || c.stage != Stage::Elder {
            return Err("horizon.civilization.population.leader_only".into());
        }
        let p = c.population.as_mut().unwrap();
        if p.revision != revision {
            return Err("horizon.civilization.population.stale".into());
        }
        if !p.workers.contains_key(key) || job.as_ref().is_some_and(|j| !j.in_territory(&c.bell)) {
            return Err("horizon.civilization.population.invalid".into());
        }
        if let Some(j) = &job
            && p.workers.iter().any(|(k, w)| {
                k != key && w.job.as_ref().is_some_and(|old| old.station == j.station)
            })
        {
            return Err("horizon.civilization.population.occupied".into());
        }
        let next = p
            .revision
            .checked_add(1)
            .filter(|r| *r <= LIMIT)
            .ok_or("horizon.civilization.stock.limit")?;
        let w = p.workers.get_mut(key).unwrap();
        w.job = job;
        w.progress = 0;
        w.status = if w.job.is_some() {
            Status::Walking
        } else {
            Status::Idle
        };
        p.revision = next;
        Ok(())
    }
    /// Internal server evidence. At most one second is credited per observation: no unloaded catch-up.
    /// All material changes and needs are staged together, including simultaneous workers.
    pub fn tick_population(
        &mut self,
        city: &str,
        now: i64,
        beds: u16,
        known: bool,
        observations: &BTreeMap<String, Observation>,
    ) -> Result<(), String> {
        let c = self
            .civilizations
            .get(city)
            .ok_or("horizon.civilization.quest.no_city")?;
        if c.stage != Stage::Elder {
            return Ok(());
        }
        if now < 0
            || beds > 4096
            || observations
                .keys()
                .any(|k| !c.population.as_ref().unwrap().workers.contains_key(k))
        {
            return Err("horizon.civilization.population.invalid".into());
        }
        let mut next = c.clone();
        let p = next.population.as_mut().unwrap();
        if p.last_tick
            .is_some_and(|last| now >= last && now - last < 20)
        {
            return Ok(());
        }
        let delta = p
            .last_tick
            .map_or(0, |last| now.saturating_sub(last).clamp(0, 20) as u32);
        p.last_tick = Some(now);
        let live = observations.values().filter(|o| o.available).count() as u16;
        if known {
            p.observed_population = live;
            p.observed_beds = beds;
            if live > 0 {
                p.active_ticks = p
                    .active_ticks
                    .checked_add(u64::from(delta))
                    .filter(|n| *n <= LIMIT)
                    .ok_or("horizon.civilization.stock.limit")?;
                p.meal_in = p.meal_in.saturating_sub(delta);
                if p.meal_in == 0 {
                    if next.economy.as_ref().unwrap().count("minecraft:bread") >= u32::from(live) {
                        next.economy
                            .as_mut()
                            .unwrap()
                            .consume_rations(u32::from(live))?;
                        p.rations = p
                            .rations
                            .checked_add(u64::from(live))
                            .filter(|n| *n <= LIMIT)
                            .ok_or("horizon.civilization.stock.limit")?;
                        p.hungry = false;
                        p.meal_in = MEAL_TICKS;
                    } else {
                        p.hungry = true;
                    }
                }
            }
        }
        for (key, w) in &mut p.workers {
            let o = observations.get(key).copied().unwrap_or_default();
            let Some(job) = &w.job else {
                w.status = if o.available {
                    Status::Idle
                } else {
                    Status::Unavailable
                };
                continue;
            };
            let reason = if !known {
                Some(Status::Unknown)
            } else if !o.available {
                Some(Status::Unavailable)
            } else if p.hungry {
                Some(Status::Hungry)
            } else if beds < live {
                Some(Status::NoHousing)
            } else if !o.workplace {
                Some(Status::NoWorkplace)
            } else if !o.arrived {
                Some(if o.blocked {
                    Status::Blocked
                } else {
                    Status::Walking
                })
            } else {
                None
            };
            if let Some(reason) = reason {
                w.status = reason;
                continue;
            }
            let s = next.economy.as_mut().unwrap();
            let (input, count) = job.profession.input();
            let (output, amount) = job.profession.output();
            if s.count(input) < count {
                w.status = Status::NoMaterials;
                continue;
            }
            if s.count(output) > super::economy::CAPACITY - amount {
                w.status = Status::Full;
                continue;
            }
            w.status = Status::Working;
            w.progress = (w.progress + delta).min(WORK_TICKS);
            if w.progress == WORK_TICKS {
                s.process_material(input, count, output, amount)?;
                w.completed = w
                    .completed
                    .checked_add(1)
                    .filter(|n| *n <= LIMIT)
                    .ok_or("horizon.civilization.stock.limit")?;
                match job.profession {
                    Profession::Baker => p.baked += 1,
                    Profession::Carpenter => p.carpentered += 1,
                }
                w.progress = 0;
            }
        }
        if p.baked > LIMIT || p.carpentered > LIMIT {
            return Err("horizon.civilization.stock.limit".into());
        }
        next.reset_food_stability();
        self.civilizations.insert(city.into(), next);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::civilization::{VillageEvidence, economy::Operation, governance::Assessment};
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
        for t in [100, 200, 300] {
            r.assess("civ-1", Assessment::observation(t, 3, 3, true, true, true))
                .unwrap();
        }
        r
    }
    fn c(r: &Registry) -> &Civilization {
        r.civilization("civ-1").unwrap()
    }
    fn p(r: &Registry) -> &Population {
        c(r).population.as_ref().unwrap()
    }
    fn item(r: &Registry, id: &str) -> u32 {
        c(r).economy.as_ref().unwrap().count(id)
    }
    fn deposit(r: &mut Registry, id: &str, n: u32) {
        r.stock_operation(
            "a",
            "civ-1",
            c(r).economy.as_ref().unwrap().revision,
            Operation::Deposit,
            id,
            n,
        )
        .unwrap();
    }
    fn observations(r: &Registry) -> BTreeMap<String, Observation> {
        p(r).workers
            .keys()
            .map(|k| {
                (
                    k.clone(),
                    Observation {
                        available: true,
                        workplace: true,
                        arrived: true,
                        blocked: false,
                    },
                )
            })
            .collect()
    }
    fn assign(r: &mut Registry, key: &str, profession: Profession, x: i32) {
        r.assign_worker(
            "a",
            "civ-1",
            p(r).revision,
            key,
            Some(Job {
                profession,
                station: [x, 70, 0],
            }),
        )
        .unwrap();
    }
    fn tick(r: &mut Registry, t: i64) {
        let o = observations(r);
        r.tick_population("civ-1", t, 3, true, &o).unwrap();
    }
    #[test]
    fn occupations_convert_actual_inputs_only_after_a_full_cycle() {
        let mut r = city();
        deposit(&mut r, "minecraft:wheat", 6);
        deposit(&mut r, "minecraft:oak_log", 2);
        for t in (0..=200).step_by(20) {
            tick(&mut r, t);
        }
        assert_eq!(
            (
                item(&r, "minecraft:wheat"),
                item(&r, "minecraft:oak_log"),
                p(&r).baked
            ),
            (6, 2, 0)
        );
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        assign(&mut r, "town-1.granary", Profession::Carpenter, -8);
        for t in (220..=380).step_by(20) {
            tick(&mut r, t);
        }
        assert_eq!(p(&r).baked, 0);
        assert_eq!(item(&r, "minecraft:oak_log"), 2);
        tick(&mut r, 400);
        assert_eq!((p(&r).baked, p(&r).carpentered), (1, 1));
        assert_eq!(
            (
                item(&r, "minecraft:wheat"),
                item(&r, "minecraft:bread"),
                item(&r, "minecraft:oak_log"),
                item(&r, "minecraft:oak_planks")
            ),
            (3, 13, 1, 4)
        );
        assert_eq!(Registry::restore(&r.save().unwrap()).unwrap(), r);
    }
    #[test]
    fn shared_materials_are_spent_once_by_competing_workers() {
        let mut r = city();
        deposit(&mut r, "minecraft:wheat", 3);
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        assign(&mut r, "town-1.granary", Profession::Baker, -8);
        for t in (0..=200).step_by(20) {
            tick(&mut r, t);
        }
        assert_eq!(
            (
                p(&r).baked,
                item(&r, "minecraft:wheat"),
                item(&r, "minecraft:bread")
            ),
            (1, 0, 13)
        );
        let before = r.clone();
        tick(&mut r, 200);
        assert_eq!(before, r);
        assert!(
            p(&r)
                .workers
                .values()
                .any(|w| w.status == Status::NoMaterials)
        );
    }
    #[test]
    fn housing_hunger_unknown_workplace_and_unloaded_workers_pause_progress() {
        let mut r = city();
        deposit(&mut r, "minecraft:wheat", 3);
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        tick(&mut r, 0);
        tick(&mut r, 20);
        let progress = p(&r).workers["town-1.quarry"].progress;
        for (time, beds, known, changed, expected) in [
            (40, 2, true, None, Status::NoHousing),
            (60, 3, false, None, Status::Unknown),
            (80, 3, true, Some("workplace"), Status::NoWorkplace),
            (100, 3, true, Some("available"), Status::Unavailable),
            (120, 3, true, Some("blocked"), Status::Blocked),
            (140, 3, true, Some("walking"), Status::Walking),
        ] {
            let mut o = observations(&r);
            let n = o.get_mut("town-1.quarry").unwrap();
            match changed {
                Some("workplace") => n.workplace = false,
                Some("available") => n.available = false,
                Some("blocked") => {
                    n.arrived = false;
                    n.blocked = true
                }
                Some("walking") => n.arrived = false,
                _ => {}
            }
            r.tick_population("civ-1", time, beds, known, &o).unwrap();
            assert_eq!(p(&r).workers["town-1.quarry"].status, expected);
            assert_eq!(p(&r).workers["town-1.quarry"].progress, progress);
        }
        assert_eq!(item(&r, "minecraft:wheat"), 3);
        assert_eq!(p(&r).baked, 0);
    }
    #[test]
    fn residents_eat_once_per_active_day_and_restoking_recovers_hunger() {
        let mut r = city();
        tick(&mut r, 0);
        for t in (20..24_000).step_by(20) {
            tick(&mut r, t);
        }
        assert_eq!(item(&r, "minecraft:bread"), 12);
        tick(&mut r, 24_000);
        assert_eq!(
            (p(&r).rations, item(&r, "minecraft:bread"), p(&r).hungry),
            (3, 9, false)
        );
        let before = r.clone();
        tick(&mut r, 24_000);
        assert_eq!(r, before);
        r.stock_operation(
            "a",
            "civ-1",
            c(&r).economy.as_ref().unwrap().revision,
            Operation::Withdraw,
            "minecraft:bread",
            9,
        )
        .unwrap();
        for t in (24_020..=48_000).step_by(20) {
            tick(&mut r, t);
        }
        assert!(p(&r).hungry);
        assert_eq!(p(&r).rations, 3);
        deposit(&mut r, "minecraft:wheat", 3);
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        tick(&mut r, 48_020);
        assert_eq!(p(&r).workers["town-1.quarry"].status, Status::Hungry);
        deposit(&mut r, "minecraft:bread", 3);
        tick(&mut r, 48_040);
        assert!(!p(&r).hungry);
        assert_eq!(p(&r).rations, 6);
        assert_eq!(item(&r, "minecraft:bread"), 0);
        assert_eq!(c(&r).stage, Stage::Elder);
        assert_eq!(
            c(&r).quests[1].delivery.as_ref().unwrap().received["minecraft:bread"],
            12
        );
        assert!(r.save().is_ok());
    }
    #[test]
    fn partial_work_survives_restart_and_unobserved_time_creates_no_income() {
        let mut r = city();
        deposit(&mut r, "minecraft:wheat", 3);
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        for t in (0..=100).step_by(20) {
            tick(&mut r, t);
        }
        let mut r = Registry::restore(&r.save().unwrap()).unwrap();
        tick(&mut r, 100_100);
        assert_eq!(p(&r).workers["town-1.quarry"].progress, 120);
        assert_eq!(item(&r, "minecraft:bread"), 12);
        assert_eq!(p(&r).active_ticks, 120);
        for t in (100_120..=100_180).step_by(20) {
            tick(&mut r, t);
        }
        assert_eq!(p(&r).baked, 1);
        assert_eq!(item(&r, "minecraft:bread"), 13);
        let r = Registry::restore(&r.save().unwrap()).unwrap();
        assert_eq!(p(&r).baked, 1);
    }
    #[test]
    fn assignment_rights_versions_territory_and_exclusive_sites_are_enforced() {
        let mut r = city();
        r.begin_player("b").unwrap();
        r.invite("a", "b").unwrap();
        r.join("b", "civ-1").unwrap();
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        let rev = p(&r).revision;
        let before = r.clone();
        for (actor, city, key, version, job) in [
            ("b", "civ-1", "town-1.granary", rev, None),
            ("a", "civ-2", "town-1.granary", rev, None),
            ("a", "civ-1", "town-1.quarry", rev - 1, None),
            ("a", "civ-1", "foreign", rev, None),
            (
                "a",
                "civ-1",
                "town-1.granary",
                rev,
                Some(Job {
                    profession: Profession::Baker,
                    station: [8, 70, 0],
                }),
            ),
            (
                "a",
                "civ-1",
                "town-1.granary",
                rev,
                Some(Job {
                    profession: Profession::Baker,
                    station: [i32::MAX, 70, i32::MAX],
                }),
            ),
        ] {
            assert!(r.assign_worker(actor, city, version, key, job).is_err());
            assert_eq!(r, before);
        }
        r.transfer_leadership("a", "b").unwrap();
        assert!(
            r.assign_worker("a", "civ-1", rev, "town-1.quarry", None)
                .is_err()
        );
        r.assign_worker("b", "civ-1", rev, "town-1.quarry", None)
            .unwrap();
        assert!(p(&r).workers["town-1.quarry"].job.is_none());
    }
    #[test]
    fn migration_keeps_v7_stock_but_never_injects_workers_or_new_recipes() {
        let r = city();
        let mut old = serde_json::to_value(&r).unwrap();
        old["version"] = 7.into();
        old["civilizations"]["civ-1"]
            .as_object_mut()
            .unwrap()
            .remove("population");
        let restored = Registry::restore(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(c(&r).economy, c(&restored).economy);
        assert!(p(&restored).workers.values().all(|w| w.job.is_none()));
        assert_eq!(p(&restored).rations, 0);
        old["civilizations"]["civ-1"]["population"] = serde_json::to_value(p(&r)).unwrap();
        assert!(Registry::restore(&serde_json::to_vec(&old).unwrap()).is_err());
        let mut forged = serde_json::to_value(&r).unwrap();
        forged["civilizations"]["civ-1"]["population"]["rations"] = 3.into();
        assert!(Registry::restore(&serde_json::to_vec(&forged).unwrap()).is_err());
    }
    #[test]
    fn incomplete_work_release_and_capacity_do_not_lose_or_duplicate_materials() {
        let mut r = city();
        deposit(&mut r, "minecraft:wheat", 3);
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        tick(&mut r, 0);
        tick(&mut r, 20);
        r.assign_worker("a", "civ-1", p(&r).revision, "town-1.quarry", None)
            .unwrap();
        assert_eq!(item(&r, "minecraft:wheat"), 3);
        assert_eq!(p(&r).workers["town-1.quarry"].progress, 0);
        assign(&mut r, "town-1.quarry", Profession::Baker, 8);
        let s = r
            .civilizations
            .get_mut("civ-1")
            .unwrap()
            .economy
            .as_mut()
            .unwrap();
        s.balance.insert("minecraft:bread".into(), 1_000_000);
        s.deposited.insert("minecraft:bread".into(), 1_000_000);
        tick(&mut r, 40);
        assert_eq!(p(&r).workers["town-1.quarry"].status, Status::Full);
        assert_eq!(item(&r, "minecraft:wheat"), 3);
        assert!(r.save().is_ok());
    }
}
