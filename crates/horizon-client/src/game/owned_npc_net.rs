//! Render-only replication for independently owned NPCs on the local server.
//!
//! The custom channel carries complete, bounded snapshots. It never registers
//! vanilla entities or invokes a client-side controller, navigation or physics.
use super::Game;
use horizon_client::{
    lan::hub::Hub,
    mesher::Vertex,
    mobs::{self, Mob},
    owned_npc_actor::{self, Actor},
};
use horizon_mods::owned_npcs::{Npc, Presence, Registry};
use horizon_protocol::packets::{configuration::CustomPayload, play::Clientbound};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, time::Instant};

pub(super) const CHANNEL: &str = "horizon:owned_npcs";
const VERSION: u32 = 1;
const MAX_NPCS: usize = 128;
const MAX_BYTES: usize = 256 * 1024;
const SNAPSHOT_INTERVAL: u64 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WireNpc {
    pub npc: Npc,
    pub yaw: f32,
    pub body_yaw: f32,
    pub head_yaw: f32,
    pub head_pitch: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    dimension: String,
    tick: u64,
    npcs: Vec<WireNpc>,
}

impl Snapshot {
    fn validate(&self) -> Result<(), String> {
        if self.version != VERSION || self.npcs.len() > MAX_NPCS {
            return Err("Invalid owned NPC snapshot version or size".into());
        }
        if !matches!(
            self.dimension.as_str(),
            "minecraft:overworld" | "minecraft:the_nether" | "minecraft:the_end"
        ) {
            return Err("Invalid owned NPC snapshot dimension".into());
        }
        let mut identities = HashSet::new();
        for actor in &self.npcs {
            if actor.npc.dimension != self.dimension
                || actor.npc.presence != Presence::Loaded
                || ![actor.yaw, actor.body_yaw, actor.head_yaw, actor.head_pitch]
                    .into_iter()
                    .all(f32::is_finite)
                || !identities.insert((&actor.npc.owner, &actor.npc.stable_key))
            {
                return Err("Invalid owned NPC snapshot actor".into());
            }
        }
        // Reuse the authoritative registry's complete identity, resource, order,
        // coordinate and per-owner quota checks. No world actors are created.
        let records: Vec<&Npc> = self.npcs.iter().map(|actor| &actor.npc).collect();
        let index = serde_json::to_vec(&serde_json::json!({"version": 1, "records": records}))
            .map_err(|error| error.to_string())?;
        Registry::restore(&index)?;
        Ok(())
    }

    fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("Owned NPC snapshot exceeds 256 KiB".into());
        }
        Ok(bytes)
    }

    fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Owned NPC snapshot exceeds 256 KiB".into());
        }
        let snapshot: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn newer_than(&self, dimension: &str, previous: Option<&(String, u64)>) -> bool {
        self.dimension == dimension
            && previous.is_none_or(|(level, tick)| level != dimension || self.tick > *tick)
    }
}

/// A body used only by the rendering pass, outside the vanilla entity list.
pub(super) struct MirrorNpc {
    pub body: Mob,
    pub npc: Npc,
    pub tick: u64,
    received: Instant,
    interpolation_seconds: f64,
}

impl MirrorNpc {
    fn fraction(&self, now: Instant) -> f64 {
        (now.duration_since(self.received).as_secs_f64() / self.interpolation_seconds)
            .clamp(0.0, 1.0)
    }
}

impl Game<'_> {
    /// Every second server tick, send the current level only to loaded guests.
    /// The local integrated player is a guest on the same transport.
    pub(super) fn send_owned_npc_snapshot(&self, hub: &Hub) {
        if self.remote.is_some()
            || !self.has_owned_npc_backend()
            || !self.ticks.is_multiple_of(SNAPSHOT_INTERVAL)
        {
            return;
        }
        let snapshot = Snapshot {
            version: VERSION,
            dimension: self.dim.id().into(),
            tick: self.ticks,
            npcs: self.owned_npc_wire(),
        };
        let bytes = match snapshot.encode() {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("Owned NPC snapshot was not sent: {error}");
                return;
            }
        };
        let packet = Clientbound::CustomPayload(CustomPayload {
            channel: CHANNEL.into(),
            data: bytes,
        });
        for guest in hub
            .guests
            .iter()
            .filter(|guest| hub.here(guest) && guest.loaded)
        {
            guest.send(&packet);
        }
    }

    /// Unknown channels are left alone. Malformed packets retain the last good
    /// mirror set; this initial backend accepts only our own integrated host.
    pub(super) fn handle_owned_npc_payload(&mut self, payload: &CustomPayload) -> bool {
        if payload.channel != CHANNEL {
            return false;
        }
        if self.integrated.is_none() || self.remote.is_none() {
            return true;
        }
        if let Err(error) = self.apply_owned_npc_payload(&payload.data) {
            log::warn!("Ignored owned NPC snapshot: {error}");
        }
        true
    }

    fn apply_owned_npc_payload(&mut self, bytes: &[u8]) -> Result<(), String> {
        let snapshot = Snapshot::decode(bytes)?;
        if !snapshot.newer_than(self.dim.id(), self.owned_npc_snapshot_tick.as_ref()) {
            return Ok(());
        }
        let now = Instant::now();
        let mut mirrors = Vec::with_capacity(snapshot.npcs.len());
        for wire in snapshot.npcs {
            let texture = owned_npc_actor::texture_key(&wire.npc.appearance)?;
            if !self.gui.atlas.sprites.contains_key(&texture) {
                return Err(format!(
                    "Missing owned NPC appearance texture in atlas: {texture}"
                ));
            }
            let template = self.owned_npc_template(&wire.npc.appearance)?;
            // Actor::new constructs a geometry body. Its server controller is
            // immediately dropped and is never ticked by the client mirror.
            let mut body = Actor::new(&wire.npc, template)?.body;
            let old = self.owned_npc_mirrors.iter().find(|old| {
                old.npc.owner == wire.npc.owner && old.npc.stable_key == wire.npc.stable_key
            });
            let interpolation_seconds = if let Some(old) = old {
                let delta = snapshot.tick.saturating_sub(old.tick);
                body.previous = old.body.previous.lerp(old.body.position, old.fraction(now));
                body.previous_yaw = old.body.yaw;
                body.previous_body_yaw = old.body.body_yaw;
                body.previous_head_yaw = old.body.head_yaw;
                body.previous_head_pitch = old.body.head_pitch;
                let dx = body.position.x - body.previous.x;
                let dz = body.position.z - body.previous.z;
                let distance = (dx * dx + dz * dz).sqrt() as f32;
                body.limb_position = old.body.limb_position + distance * 4.0;
                body.previous_limb_speed = old.body.limb_speed;
                body.limb_speed = (distance * 4.0).min(1.0);
                (delta as f64 * 0.05).clamp(0.05, 0.5)
            } else {
                body.previous_yaw = wire.yaw;
                body.previous_body_yaw = wire.body_yaw;
                body.previous_head_yaw = wire.head_yaw;
                body.previous_head_pitch = wire.head_pitch;
                0.1
            };
            body.yaw = wire.yaw;
            body.body_yaw = wire.body_yaw;
            body.head_yaw = wire.head_yaw;
            body.head_pitch = wire.head_pitch;
            mirrors.push(MirrorNpc {
                body,
                npc: wire.npc,
                tick: snapshot.tick,
                received: now,
                interpolation_seconds,
            });
        }
        // Swap only after every record and geometry resource has been checked.
        // An empty complete snapshot also removes all previous visible bodies.
        self.owned_npc_mirrors = mirrors;
        self.owned_npc_snapshot_tick = Some((snapshot.dimension, snapshot.tick));
        Ok(())
    }

    pub(super) fn clear_owned_npc_mirrors(&mut self) {
        self.owned_npc_mirrors.clear();
        self.owned_npc_snapshot_tick = None;
    }

    /// Draw the explicit appearance texture with the body's prepared model.
    /// There is no call to tick, AI, physics, navigation or vanilla mob layers.
    pub(super) fn owned_npc_mirror_geometry(
        &self,
        camera: glam::DVec3,
        _alpha: f64,
        out: &mut Vec<Vertex>,
    ) {
        let now = Instant::now();
        for mirror in &self.owned_npc_mirrors {
            let mob = &mirror.body;
            let Ok(texture) = owned_npc_actor::texture_key(&mirror.npc.appearance) else {
                continue;
            };
            let Some(rect) = self.gui.atlas.sprites.get(&texture) else {
                continue;
            };
            let distance =
                glam::DVec3::new(mob.position.x, mob.position.y, mob.position.z) - camera;
            let (width, height, _) = mob.dims();
            let range = (width + height + width) / 3.0 * 64.0 * self.view_scale();
            if distance.length_squared() >= range * range {
                continue;
            }
            let light = self.world.light(
                mob.position.x.floor() as i32,
                (mob.position.y + mob.eye_height()).floor() as i32,
                mob.position.z.floor() as i32,
            );
            let packed = (u32::from(light >> 4) * 16) | ((u32::from(light & 15) * 16) << 8);
            let start = out.len();
            self.mark_entity(start, &mob.kind.id);
            mobs::render(
                mob,
                mirror.fraction(now) as f32,
                camera,
                [rect.u0, rect.v0, rect.u1, rect.v1],
                packed,
                out,
            );
            self.cull_entity_vertices(out, start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_mods::owned_npcs::{Appearance, EnsureNpc, Position};

    fn snapshot() -> Snapshot {
        let mut registry = Registry::default();
        let npc = registry
            .ensure(
                "civilization",
                EnsureNpc {
                    stable_key: "citizen_1".into(),
                    definition: "civilization:worker".into(),
                    city: "city_1".into(),
                    profession: "wood".into(),
                    appearance: Appearance {
                        model: "minecraft:entity/player".into(),
                        texture: "minecraft:entity/player/wide/steve".into(),
                    },
                    dimension: "minecraft:overworld".into(),
                    spawn: Position {
                        x: 0.5,
                        y: 65.0,
                        z: 0.5,
                    },
                },
            )
            .unwrap()
            .clone();
        let mut npc = npc;
        npc.presence = Presence::Loaded;
        Snapshot {
            version: VERSION,
            dimension: npc.dimension.clone(),
            tick: 20,
            npcs: vec![WireNpc {
                npc,
                yaw: 0.0,
                body_yaw: 0.0,
                head_yaw: 0.0,
                head_pitch: 0.0,
            }],
        }
    }

    #[test]
    fn snapshot_round_trip_preserves_identity_and_explicit_appearance() {
        let snapshot = snapshot();
        let decoded = Snapshot::decode(&snapshot.encode().unwrap()).unwrap();
        assert_eq!(decoded, snapshot);
        assert_eq!(decoded.npcs[0].npc.owner, "civilization");
        assert_eq!(
            decoded.npcs[0].npc.appearance.model,
            "minecraft:entity/player"
        );
    }

    #[test]
    fn transporting_an_order_does_not_execute_it_or_move_the_actor() {
        use horizon_mods::owned_npcs::{Order, OrderStatus};
        let mut snapshot = snapshot();
        let position = snapshot.npcs[0].npc.position;
        snapshot.npcs[0].npc.order = Order::MoveTo {
            target: Position {
                x: 10.5,
                y: 65.0,
                z: 0.5,
            },
            arrival_radius: 0.5,
        };
        snapshot.npcs[0].npc.order_status = OrderStatus::Pending;
        let decoded = Snapshot::decode(&snapshot.encode().unwrap()).unwrap();
        assert_eq!(decoded.npcs[0].npc.position, position);
        assert_eq!(decoded.npcs[0].npc.order_status, OrderStatus::Pending);
        assert!(matches!(decoded.npcs[0].npc.order, Order::MoveTo { .. }));
    }

    #[test]
    fn malformed_oversized_and_non_finite_snapshots_are_rejected() {
        assert!(Snapshot::decode(b"{not-json}").is_err());
        assert!(Snapshot::decode(&vec![b' '; MAX_BYTES + 1]).is_err());
        let mut snapshot = snapshot();
        snapshot.npcs[0].yaw = f32::NAN;
        assert!(snapshot.validate().is_err());
        snapshot.npcs[0].yaw = 0.0;
        snapshot.npcs[0].npc.position.x = f64::INFINITY;
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn invalid_resources_quotas_duplicates_and_wrong_level_are_rejected() {
        let mut snapshot = snapshot();
        snapshot.npcs[0].npc.appearance.texture = "minecraft:../secret".into();
        assert!(snapshot.validate().is_err());
        snapshot = self::snapshot();
        snapshot.npcs.push(snapshot.npcs[0].clone());
        assert!(snapshot.validate().is_err());
        snapshot.npcs = vec![snapshot.npcs[0].clone(); MAX_NPCS + 1];
        assert!(snapshot.validate().is_err());
        snapshot = self::snapshot();
        snapshot.npcs[0].npc.dimension = "minecraft:the_nether".into();
        assert!(snapshot.validate().is_err());
        snapshot = self::snapshot();
        snapshot.version += 1;
        assert!(snapshot.validate().is_err());
    }

    #[test]
    fn ordering_keeps_the_empty_snapshot_barrier_and_rejects_other_dimensions() {
        let mut snapshot = snapshot();
        let level = "minecraft:overworld";
        let last = (level.to_string(), snapshot.tick);
        assert!(!snapshot.newer_than(level, Some(&last)));
        snapshot.tick -= 1;
        assert!(!snapshot.newer_than(level, Some(&last)));
        snapshot.tick += 2;
        snapshot.npcs.clear();
        let empty = Snapshot::decode(&snapshot.encode().unwrap()).unwrap();
        assert!(empty.newer_than(level, Some(&last)));
        let barrier = (level.to_string(), empty.tick);
        assert!(!self::snapshot().newer_than(level, Some(&barrier)));
        assert!(!empty.newer_than("minecraft:the_nether", None));
    }
}
