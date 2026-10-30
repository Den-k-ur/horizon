//! Engine-facing foundation for mod-owned NPCs, independent of vanilla mobs.
//! The host supplies the authenticated mod ID; callers never choose an owner.
//! This registry persists unloaded identities and emits orders, not teleports.
use serde::{Deserialize, Serialize};

pub const MAX_PER_OWNER: usize = 32;
const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Position {
    fn validate(self) -> Result<(), String> {
        if [self.x, self.y, self.z].into_iter().any(|n| !n.is_finite())
            || self.x.abs() > 29_999_984.0
            || self.z.abs() > 29_999_984.0
            || !(-64.0..=319.0).contains(&self.y)
        {
            return Err("NPC position is outside the supported world".into());
        }
        Ok(())
    }
    fn distance_squared(self, other: Self) -> f64 {
        (self.x - other.x).powi(2) + (self.y - other.y).powi(2) + (self.z - other.z).powi(2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub model: String,
    pub texture: String,
}

impl Appearance {
    fn validate(&self) -> Result<(), String> {
        resource(&self.model)?;
        resource(&self.texture)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Order {
    Idle,
    MoveTo {
        target: Position,
        arrival_radius: f64,
    },
    Work {
        action: String,
        seconds: f64,
    },
}

impl Order {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Idle => Ok(()),
            Self::MoveTo {
                target,
                arrival_radius,
            } => {
                target.validate()?;
                if !arrival_radius.is_finite() || !(0.1..=2.0).contains(arrival_radius) {
                    return Err("NPC arrival radius must be between 0.1 and 2 blocks".into());
                }
                Ok(())
            }
            Self::Work { action, seconds } => {
                identifier(action)?;
                if !seconds.is_finite() || !(0.05..=3600.0).contains(seconds) {
                    return Err("NPC work duration must be between 0.05 and 3600 seconds".into());
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Presence {
    PendingSpawn,
    Loaded,
    Unloaded,
    Despawned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OrderStatus {
    Pending,
    Running,
    Complete,
    Cancelled,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Npc {
    pub owner: String,
    pub stable_key: String,
    pub definition: String,
    pub city: String,
    pub profession: String,
    pub appearance: Appearance,
    pub dimension: String,
    pub position: Position,
    pub presence: Presence,
    pub order: Order,
    pub order_revision: u64,
    pub order_status: OrderStatus,
    #[serde(default)]
    pub work_seconds: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnsureNpc {
    pub stable_key: String,
    pub definition: String,
    pub city: String,
    pub profession: String,
    pub appearance: Appearance,
    pub dimension: String,
    pub spawn: Position,
}

impl EnsureNpc {
    fn validate(&self) -> Result<(), String> {
        for value in [&self.stable_key, &self.city, &self.profession] {
            identifier(value)?;
        }
        resource(&self.definition)?;
        resource(&self.dimension)?;
        self.appearance.validate()?;
        self.spawn.validate()
    }
}

/// The world-owned index must be restored before any chunk/NPC is loaded.
/// Dropping an actor from memory must call `mark_unloaded`, not remove its key.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    version: u32,
    records: Vec<Npc>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            version: VERSION,
            records: Vec::new(),
        }
    }
}

impl Registry {
    pub fn ensure(&mut self, owner: &str, request: EnsureNpc) -> Result<&Npc, String> {
        identifier(owner)?;
        request.validate()?;
        if let Some(index) = self
            .records
            .iter()
            .position(|n| n.owner == owner && n.stable_key == request.stable_key)
        {
            let existing = &self.records[index];
            if existing.definition != request.definition
                || existing.city != request.city
                || existing.dimension != request.dimension
            {
                return Err(
                    "NPC key already belongs to a different definition, city or dimension".into(),
                );
            }
            // In particular, Unloaded is an existing identity, never a spawn request.
            return Ok(existing);
        }
        if self.records.len() >= 4096
            || self.records.iter().filter(|n| n.owner == owner).count() >= MAX_PER_OWNER
        {
            return Err("NPC ownership quota exceeded".into());
        }
        self.records.push(Npc {
            owner: owner.into(),
            stable_key: request.stable_key,
            definition: request.definition,
            city: request.city,
            profession: request.profession,
            appearance: request.appearance,
            dimension: request.dimension,
            position: request.spawn,
            presence: Presence::PendingSpawn,
            order: Order::Idle,
            order_revision: 0,
            order_status: OrderStatus::Complete,
            work_seconds: 0.0,
        });
        Ok(self.records.last().expect("just inserted"))
    }

    pub fn state(&self, owner: &str, key: &str) -> Result<&Npc, String> {
        self.records
            .iter()
            .find(|n| n.owner == owner && n.stable_key == key)
            .ok_or_else(|| "Unknown owned NPC".into())
    }

    fn state_mut(&mut self, owner: &str, key: &str) -> Result<&mut Npc, String> {
        self.records
            .iter_mut()
            .find(|n| n.owner == owner && n.stable_key == key)
            .ok_or_else(|| "Unknown owned NPC".into())
    }

    pub fn set_order(&mut self, owner: &str, key: &str, order: Order) -> Result<u64, String> {
        order.validate()?;
        let npc = self.state_mut(owner, key)?;
        if npc.presence == Presence::Despawned {
            return Err("NPC was despawned".into());
        }
        let revision = npc
            .order_revision
            .checked_add(1)
            .ok_or("NPC revision exhausted")?;
        npc.order_revision = revision;
        npc.order = order;
        npc.order_status = OrderStatus::Pending;
        npc.work_seconds = 0.0;
        Ok(revision)
    }

    pub fn cancel(&mut self, owner: &str, key: &str) -> Result<u64, String> {
        let revision = self.set_order(owner, key, Order::Idle)?;
        self.state_mut(owner, key)?.order_status = OrderStatus::Cancelled;
        Ok(revision)
    }

    pub fn set_appearance(
        &mut self,
        owner: &str,
        key: &str,
        appearance: Appearance,
    ) -> Result<(), String> {
        appearance.validate()?;
        self.state_mut(owner, key)?.appearance = appearance;
        Ok(())
    }

    /// Reassignment keeps identity/position and invalidates the former task.
    pub fn set_profession(
        &mut self,
        owner: &str,
        key: &str,
        profession: &str,
    ) -> Result<u64, String> {
        identifier(profession)?;
        let revision = self.cancel(owner, key)?;
        self.state_mut(owner, key)?.profession = profession.into();
        Ok(revision)
    }

    /// Native backend observation. A finished/failed path alone is not arrival.
    pub fn observe(
        &mut self,
        owner: &str,
        key: &str,
        revision: u64,
        position: Position,
        arrived: bool,
    ) -> Result<(), String> {
        position.validate()?;
        let npc = self.state_mut(owner, key)?;
        if revision != npc.order_revision || npc.presence == Presence::Despawned {
            return Err("Stale NPC observation".into());
        }
        if arrived {
            let Order::MoveTo {
                target,
                arrival_radius,
            } = &npc.order
            else {
                return Err("NPC has no movement order".into());
            };
            if position.distance_squared(*target) > arrival_radius.powi(2) {
                return Err("NPC has not reached its target".into());
            }
        }
        npc.position = position;
        npc.presence = Presence::Loaded;
        npc.order_status = if arrived {
            OrderStatus::Complete
        } else {
            OrderStatus::Running
        };
        Ok(())
    }

    pub fn mark_unloaded(&mut self, owner: &str, key: &str) -> Result<(), String> {
        let npc = self.state_mut(owner, key)?;
        if npc.presence != Presence::Despawned {
            npc.presence = Presence::Unloaded;
        }
        Ok(())
    }

    /// All persistent world identities, including unloaded actors and tombstones.
    pub fn records(&self) -> impl Iterator<Item = &Npc> {
        self.records.iter()
    }

    /// Trusted native navigation/physics observation with a persistent work timer.
    pub fn update_runtime(
        &mut self,
        owner: &str,
        key: &str,
        revision: u64,
        position: Position,
        status: OrderStatus,
        work_seconds: f64,
    ) -> Result<(), String> {
        position.validate()?;
        if !work_seconds.is_finite() || !(0.0..=3600.1).contains(&work_seconds) {
            return Err("Invalid native work progress".into());
        }
        let npc = self.state_mut(owner, key)?;
        if npc.order_revision != revision || npc.presence == Presence::Despawned {
            return Err("Stale NPC observation".into());
        }
        if status == OrderStatus::Complete && npc.order_status != OrderStatus::Complete {
            match npc.order {
                Order::MoveTo {
                    target,
                    arrival_radius,
                } if position.distance_squared(target) > arrival_radius.powi(2) => {
                    return Err("NPC has not reached its target".into());
                }
                Order::Work { seconds, .. } if work_seconds + 0.000001 < seconds => {
                    return Err("NPC work is not complete".into());
                }
                _ => {}
            }
        }
        npc.position = position;
        npc.presence = Presence::Loaded;
        npc.order_status = status;
        npc.work_seconds = work_seconds;
        Ok(())
    }

    /// Keep a tombstone so an unloaded old actor is removed when its chunk loads.
    pub fn despawn(&mut self, owner: &str, key: &str) -> Result<(), String> {
        self.cancel(owner, key)?;
        self.state_mut(owner, key)?.presence = Presence::Despawned;
        Ok(())
    }

    pub fn save(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(self).map_err(|e| e.to_string())
    }

    pub fn restore(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("NPC index exceeds 8 MiB".into());
        }
        let index: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if index.version != VERSION || index.records.len() > 4096 {
            return Err("Invalid NPC index version or size".into());
        }
        let mut keys = std::collections::HashSet::new();
        let mut quotas = std::collections::HashMap::<&str, usize>::new();
        for npc in &index.records {
            identifier(&npc.owner)?;
            EnsureNpc {
                stable_key: npc.stable_key.clone(),
                definition: npc.definition.clone(),
                city: npc.city.clone(),
                profession: npc.profession.clone(),
                appearance: npc.appearance.clone(),
                dimension: npc.dimension.clone(),
                spawn: npc.position,
            }
            .validate()?;
            npc.order.validate()?;
            if !npc.work_seconds.is_finite() || !(0.0..=3600.1).contains(&npc.work_seconds) {
                return Err("Invalid saved work progress".into());
            }
            if !keys.insert((&npc.owner, &npc.stable_key)) {
                return Err("Duplicate NPC identity in index".into());
            }
            let count = quotas.entry(&npc.owner).or_default();
            *count += 1;
            if *count > MAX_PER_OWNER {
                return Err("NPC ownership quota exceeded".into());
            }
        }
        Ok(index)
    }
}

fn identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
    {
        return Err("Invalid NPC identifier".into());
    }
    Ok(())
}

fn resource(value: &str) -> Result<(), String> {
    let Some((namespace, path)) = value.split_once(':') else {
        return Err("Expected namespace:path resource".into());
    };
    identifier(namespace)?;
    if path.is_empty()
        || path.len() > 128
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_/.-".contains(&b))
    {
        return Err("Invalid NPC resource path".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(x: f64) -> Position {
        Position { x, y: 64.0, z: 0.0 }
    }
    fn request() -> EnsureNpc {
        EnsureNpc {
            stable_key: "citizen_1".into(),
            definition: "civilization:worker".into(),
            city: "city_1".into(),
            profession: "wood".into(),
            appearance: Appearance {
                model: "minecraft:entity/player".into(),
                texture: "civilization:entity/worker".into(),
            },
            dimension: "minecraft:overworld".into(),
            spawn: point(0.0),
        }
    }
    #[test]
    fn ensure_after_unload_and_restore_does_not_respawn_or_reposition() {
        let mut index = Registry::default();
        index.ensure("city_mod", request()).unwrap();
        index
            .observe("city_mod", "citizen_1", 0, point(2.0), false)
            .unwrap();
        index.mark_unloaded("city_mod", "citizen_1").unwrap();
        let mut index = Registry::restore(&index.save().unwrap()).unwrap();
        let record = index.ensure("city_mod", request()).unwrap();
        assert_eq!(record.presence, Presence::Unloaded);
        assert_eq!(record.position, point(2.0));
        assert_eq!(index.records.len(), 1);
    }
    #[test]
    fn owners_are_isolated_even_when_keys_match() {
        let mut index = Registry::default();
        index.ensure("one", request()).unwrap();
        assert!(index.cancel("two", "citizen_1").is_err());
        index.ensure("two", request()).unwrap();
        index.cancel("two", "citizen_1").unwrap();
        assert_eq!(index.state("one", "citizen_1").unwrap().order_revision, 0);
    }
    #[test]
    fn cancelling_rejects_late_arrival_and_movement_requires_proximity() {
        let mut index = Registry::default();
        index.ensure("one", request()).unwrap();
        let revision = index
            .set_order(
                "one",
                "citizen_1",
                Order::MoveTo {
                    target: point(8.0),
                    arrival_radius: 0.75,
                },
            )
            .unwrap();
        assert!(
            index
                .observe("one", "citizen_1", revision, point(1.0), true)
                .is_err()
        );
        assert_eq!(
            index.state("one", "citizen_1").unwrap().position,
            point(0.0)
        );
        index.cancel("one", "citizen_1").unwrap();
        assert!(
            index
                .observe("one", "citizen_1", revision, point(8.0), true)
                .is_err()
        );
    }
    #[test]
    fn invalid_data_and_excess_population_are_rejected() {
        let mut index = Registry::default();
        let mut bad = request();
        bad.spawn.x = f64::NAN;
        assert!(index.ensure("one", bad).is_err());
        let mut bad = request();
        bad.appearance.texture = "civilization:../escape".into();
        assert!(index.ensure("one", bad).is_err());
        for i in 0..MAX_PER_OWNER {
            let mut item = request();
            item.stable_key = format!("citizen_{i}");
            index.ensure("one", item).unwrap();
        }
        let mut extra = request();
        extra.stable_key = "extra".into();
        assert!(index.ensure("one", extra).is_err());
    }
    #[test]
    fn despawn_retains_a_tombstone_and_rejects_new_orders() {
        let mut index = Registry::default();
        index.ensure("one", request()).unwrap();
        index.despawn("one", "citizen_1").unwrap();
        assert_eq!(
            index.ensure("one", request()).unwrap().presence,
            Presence::Despawned
        );
        assert!(index.set_order("one", "citizen_1", Order::Idle).is_err());
    }
    #[test]
    fn corrupt_or_duplicate_index_is_rejected() {
        let mut index = Registry::default();
        index.ensure("one", request()).unwrap();
        index.records.push(index.records[0].clone());
        assert!(Registry::restore(&index.save().unwrap()).is_err());
        assert!(Registry::restore(br#"{"version":900,"records":[]}"#).is_err());
    }

    #[test]
    fn profession_and_appearance_are_independent_of_identity_and_movement() {
        let mut index = Registry::default();
        index.ensure("one", request()).unwrap();
        let original = index.state("one", "citizen_1").unwrap().clone();
        let revision = index.set_profession("one", "citizen_1", "builder").unwrap();
        let npc = index.state("one", "citizen_1").unwrap();
        assert_eq!(npc.order_revision, revision);
        assert_eq!(npc.order_status, OrderStatus::Cancelled);
        assert_eq!(npc.profession, "builder");
        assert_eq!(npc.position, original.position);
        assert_eq!(npc.appearance, original.appearance);
        assert_eq!(npc.stable_key, original.stable_key);
    }

    #[test]
    fn native_work_progress_survives_save_and_cannot_finish_early() {
        let mut index = Registry::default();
        index.ensure("one", request()).unwrap();
        let revision = index
            .set_order(
                "one",
                "citizen_1",
                Order::Work {
                    action: "gather_wood".into(),
                    seconds: 5.0,
                },
            )
            .unwrap();
        index
            .update_runtime(
                "one",
                "citizen_1",
                revision,
                point(0.0),
                OrderStatus::Running,
                4.95,
            )
            .unwrap();
        let mut index = Registry::restore(&index.save().unwrap()).unwrap();
        assert_eq!(index.state("one", "citizen_1").unwrap().work_seconds, 4.95);
        assert!(
            index
                .update_runtime(
                    "one",
                    "citizen_1",
                    revision,
                    point(0.0),
                    OrderStatus::Complete,
                    4.95
                )
                .is_err()
        );
        index
            .update_runtime(
                "one",
                "citizen_1",
                revision,
                point(0.0),
                OrderStatus::Complete,
                5.0,
            )
            .unwrap();
    }
}
