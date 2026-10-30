//! Physical motion for mod-owned actors, without vanilla entity gameplay.
//!
//! This deliberately bypasses `Mob::tick`: no despawning, goals, species
//! brains, trading, POIs, breeding or entity events enter the vanilla world.
use super::{Mob, MobEvent, MobWorld};
use horizon_core::geometry::Vec3;

pub fn tick(body: &mut Mob, world: &MobWorld) {
    body.previous = body.position;
    body.previous_yaw = body.yaw;
    body.previous_body_yaw = body.body_yaw;
    body.previous_head_yaw = body.head_yaw;
    body.previous_head_pitch = body.head_pitch;
    body.anim.begin_tick();
    body.age = body.age.wrapping_add(1);
    let mut events: Vec<MobEvent> = Vec::new();
    body.update_fluids(world, &mut events);
    body.ai.xxa *= 0.98;
    body.ai.zza *= 0.98;
    body.nav_tick(world);
    body.move_control_tick(world, &mut events);
    body.look_control_tick();
    body.jump_control_tick();
    body.handle_jumping(world);
    let input = Vec3::new(
        f64::from(body.ai.xxa),
        f64::from(body.ai.yya),
        f64::from(body.ai.zza),
    );
    body.travel(world, input, &mut events);
    body.body_rotation_tick();
    body.update_walk_animation();
    body.anim.end_tick(body.ai.swinging);
    super::ai::tick_swing(body);
    body.first_tick = false;
}
