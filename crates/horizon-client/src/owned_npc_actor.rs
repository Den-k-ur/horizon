//! Native body adapter for independently owned NPC identities and orders.
//! Bodies stay outside vanilla mob collections, targeting and persistence.
use crate::mobs::{
    Anim, Mob, MobType, MobWorld, Species, control::MoveControl, nav::Navigation, owned_npc_motion,
};
use horizon_core::geometry::Vec3;
use horizon_mods::owned_npcs::{Appearance, Npc, Order, OrderStatus, Position, Presence};
use std::{
    hash::{Hash, Hasher},
    sync::Arc,
};

const REPATH_TICKS: u32 = 20;
const STUCK_TICKS: u32 = 60;

/// Prepared geometry resources supported by the initial native backend.
/// A geometry resource does not register a vanilla entity or its AI.
pub fn geometry_template(appearance: &Appearance) -> Result<&'static str, String> {
    match appearance.model.as_str() {
        "minecraft:entity/player" => Ok("minecraft:mannequin"),
        "minecraft:entity/villager" => Ok("minecraft:villager"),
        _ => Err(format!(
            "Unsupported owned NPC geometry: {}",
            appearance.model
        )),
    }
}

/// Convert a prepared Minecraft texture resource into its atlas lookup key.
/// The caller must also check that this key exists in the current atlas.
pub fn texture_key(appearance: &Appearance) -> Result<String, String> {
    let path = appearance
        .texture
        .strip_prefix("minecraft:")
        .ok_or("Owned NPC skins currently require a prepared minecraft texture")?;
    let path = path.strip_prefix("textures/").unwrap_or(path);
    let path = path.strip_suffix(".png").unwrap_or(path);
    if path.is_empty()
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || !path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_/.-".contains(&b))
    {
        return Err("Invalid owned NPC texture resource".into());
    }
    Ok(path.into())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    pub position: Position,
    pub revision: u64,
    /// False means the actor's current chunk is unavailable; do not respawn it.
    pub loaded: bool,
    pub arrived: bool,
    pub blocked: bool,
    /// Completion of one explicit Work order; never changes blocks/resources.
    pub work_finished: bool,
}

/// A collision/render body plus an order driver, not a vanilla entity.
pub struct Actor {
    pub body: Mob,
    pub appearance: Appearance,
    pub revision: u64,
    pub work_seconds: f64,
    pub distance_walked: f64,
    owner: String,
    stable_key: String,
    order: Order,
    work: WorkClock,
    repath: u32,
    stationary: u32,
    path_failed: bool,
    completed: bool,
}

#[derive(Default)]
struct WorkClock {
    ticks: u32,
    reported: bool,
}

impl WorkClock {
    fn restore(seconds: f64, complete: bool) -> Self {
        Self {
            ticks: (seconds * 20.0).round() as u32,
            reported: complete,
        }
    }

    fn step(&mut self, seconds: f64) -> bool {
        if self.reported {
            return false;
        }
        self.ticks = self.ticks.saturating_add(1);
        if f64::from(self.ticks) >= (seconds * 20.0).ceil() {
            self.reported = true;
            true
        } else {
            false
        }
    }
}

fn point(position: Vec3) -> Position {
    Position {
        x: position.x,
        y: position.y,
        z: position.z,
    }
}

fn separation_squared(a: Position, b: Position) -> f64 {
    (a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)
}

fn finite_position(position: Position) -> bool {
    [position.x, position.y, position.z]
        .into_iter()
        .all(f64::is_finite)
        && position.x.abs() <= 29_999_984.0
        && position.z.abs() <= 29_999_984.0
        && (-64.0..=319.0).contains(&position.y)
}

fn body_kind(npc: &Npc, template: Arc<MobType>) -> Result<Arc<MobType>, String> {
    let expected = geometry_template(&npc.appearance)?;
    if template.id != expected {
        return Err(format!(
            "NPC geometry requires {expected}, received {}",
            template.id
        ));
    }
    let mut kind = (*template).clone();
    kind.id = format!("owned_npc:{}", npc.definition.replace(':', "/"));
    // Only an inert body classification; no mannequin tick is called either.
    kind.species = Species::Mannequin;
    kind.anim = if expected == "minecraft:villager" {
        Anim::Villager
    } else {
        Anim::Humanoid { zombie_arms: false }
    };
    kind.texture = texture_key(&npc.appearance)?;
    kind.category = "misc".into();
    kind.speed = 0.25;
    kind.follow_range = 48.0;
    Ok(Arc::new(kind))
}

impl Actor {
    /// New identities start at spawn; restored identities use their saved position.
    pub fn new(npc: &Npc, template: Arc<MobType>) -> Result<Self, String> {
        if npc.presence == Presence::Despawned || !finite_position(npc.position) {
            return Err("Cannot create an owned NPC body at this position/presence".into());
        }
        let kind = body_kind(npc, template)?;
        let mut identity = std::collections::hash_map::DefaultHasher::new();
        (&npc.owner, &npc.stable_key).hash(&mut identity);
        let mut body = Mob::new(
            identity.finish() as u32,
            kind,
            Vec3::new(npc.position.x, npc.position.y, npc.position.z),
            0.0,
        );
        body.no_ai = true;
        body.persistent = true;
        body.invulnerable_flag = true;
        body.set_required_path_length(48.0);
        let work_seconds = if matches!(npc.order, Order::Work { .. }) {
            npc.work_seconds
        } else {
            0.0
        };
        Ok(Self {
            body,
            appearance: npc.appearance.clone(),
            revision: npc.order_revision,
            work_seconds,
            distance_walked: 0.0,
            owner: npc.owner.clone(),
            stable_key: npc.stable_key.clone(),
            order: npc.order.clone(),
            work: WorkClock::restore(work_seconds, npc.order_status == OrderStatus::Complete),
            repath: 0,
            stationary: 0,
            path_failed: false,
            completed: npc.order_status == OrderStatus::Complete,
        })
    }

    /// Changing geometry never moves or replaces the actor identity.
    pub fn set_geometry(&mut self, npc: &Npc, template: Arc<MobType>) -> Result<(), String> {
        self.check_identity(npc)?;
        let kind = body_kind(npc, template)?;
        self.body.kind = kind;
        self.appearance = npc.appearance.clone();
        Ok(())
    }

    fn check_identity(&self, npc: &Npc) -> Result<(), String> {
        if self.owner != npc.owner || self.stable_key != npc.stable_key {
            Err("Owned NPC actor identity mismatch".into())
        } else {
            Ok(())
        }
    }

    fn stop_navigation(&mut self) {
        // nav_stop alone retains a delayed recompute and its old target.
        self.body.ai.nav = Navigation::default();
        self.body.set_required_path_length(48.0);
        self.body.ai.mov = MoveControl::default();
        self.body.ai.xxa = 0.0;
        self.body.ai.yya = 0.0;
        self.body.ai.zza = 0.0;
        self.body.ai.jump = false;
        self.body.ai.jumping = false;
        self.body.velocity.x = 0.0;
        self.body.velocity.z = 0.0;
    }

    /// Called once per running server tick with the current authenticated order.
    pub fn tick(&mut self, world: &MobWorld, npc: &Npc) -> Observation {
        let mut observation = Observation {
            position: point(self.body.position),
            revision: self.revision,
            loaded: world.world.loaded(
                self.body.position.x.floor() as i32,
                self.body.position.z.floor() as i32,
            ),
            arrived: false,
            blocked: false,
            work_finished: false,
        };
        if self.check_identity(npc).is_err()
            || npc.order_revision < self.revision
            || npc.presence == Presence::Despawned
        {
            self.stop_navigation();
            observation.blocked = true;
            return observation;
        }
        if npc.order_revision != self.revision || npc.order != self.order {
            self.stop_navigation();
            self.revision = npc.order_revision;
            self.order = npc.order.clone();
            self.work = WorkClock::default();
            self.work_seconds = 0.0;
            self.repath = 0;
            self.stationary = 0;
            self.path_failed = false;
            self.completed = false;
        }
        observation.revision = self.revision;
        if !observation.loaded {
            return observation;
        }
        let before = point(self.body.position);
        let active = !self.completed
            && !matches!(
                npc.order_status,
                OrderStatus::Complete | OrderStatus::Cancelled
            );
        let order = self.order.clone();
        match &order {
            Order::MoveTo {
                target,
                arrival_radius,
            } if active => {
                if separation_squared(before, *target) <= arrival_radius.powi(2) {
                    self.stop_navigation();
                    self.completed = true;
                    observation.arrived = true;
                } else if self.repath == 0 {
                    let path = self.body.create_path_to(
                        world,
                        [
                            target.x.floor() as i32,
                            target.y.floor() as i32,
                            target.z.floor() as i32,
                        ],
                        0,
                    );
                    self.path_failed = !self.body.nav_move_to_path(path, 0.65);
                    self.repath = REPATH_TICKS;
                }
                self.repath = self.repath.saturating_sub(1);
            }
            Order::Work { seconds, .. } if active => {
                self.stop_navigation();
                observation.work_finished = self.work.step(*seconds);
                self.work_seconds = f64::from(self.work.ticks) / 20.0;
                self.completed = observation.work_finished;
                if self.work.ticks.is_multiple_of(12) {
                    self.body.swing();
                }
            }
            _ => self.stop_navigation(),
        }
        owned_npc_motion::tick(&mut self.body, world);
        let after = point(self.body.position);
        if !finite_position(after) {
            self.stop_navigation();
            observation.blocked = true;
            return observation;
        }
        let moved = separation_squared(before, after).sqrt();
        self.distance_walked += moved;
        observation.position = after;
        if let Order::MoveTo {
            target,
            arrival_radius,
        } = &order
            && active
            && !self.completed
        {
            if separation_squared(after, *target) <= arrival_radius.powi(2) {
                self.stop_navigation();
                self.completed = true;
                observation.arrived = true;
            } else {
                self.stationary = if moved < 0.001 {
                    self.stationary.saturating_add(1)
                } else {
                    0
                };
                observation.blocked =
                    self.path_failed || self.body.ai.nav.stuck || self.stationary >= STUCK_TICKS;
            }
        }
        observation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_finishes_once_on_the_exact_tick_and_restart_requires_new_clock() {
        let mut work = WorkClock::default();
        for _ in 0..99 {
            assert!(!work.step(5.0));
        }
        assert!(work.step(5.0));
        for _ in 0..100 {
            assert!(!work.step(5.0));
        }
        let mut replacement = WorkClock::default();
        assert!(replacement.step(0.05));
    }

    #[test]
    fn fractional_duration_never_completes_before_requested_time() {
        let mut work = WorkClock::default();
        assert!(!work.step(0.075));
        assert!(work.step(0.075));
    }

    #[test]
    fn work_resumes_without_losing_elapsed_time_or_repeating_completion() {
        let mut resumed = WorkClock::restore(4.95, false);
        assert!(resumed.step(5.0));
        assert_eq!(resumed.ticks, 100);
        let mut finished = WorkClock::restore(5.0, true);
        assert!(!finished.step(5.0));
        assert_eq!(finished.ticks, 100);
    }

    #[test]
    fn backend_rejects_unsupported_or_traversing_appearance_resources() {
        let mut appearance = Appearance {
            model: "minecraft:entity/player".into(),
            texture: "minecraft:textures/entity/player/wide/steve.png".into(),
        };
        assert_eq!(
            geometry_template(&appearance).unwrap(),
            "minecraft:mannequin"
        );
        assert_eq!(
            texture_key(&appearance).unwrap(),
            "entity/player/wide/steve"
        );
        appearance.texture = "civilization:entity/worker".into();
        assert!(texture_key(&appearance).is_err());
        appearance.texture = "minecraft:entity/../worker".into();
        assert!(texture_key(&appearance).is_err());
        appearance.model = "civilization:entity/worker".into();
        assert!(geometry_template(&appearance).is_err());
    }
}
