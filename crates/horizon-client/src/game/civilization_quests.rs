//! Server-owned order dialogue and delivery; clients only request an operation.
use super::Game;
use horizon_client::text::{ClickEvent, Style, Text};
use horizon_mods::civilization::Quest;

fn title(q: &Quest) -> Text {
    Text::tr(
        &if q.delivery.is_some() {
            q.title.clone()
        } else {
            format!("horizon.civilization.quest.{}.legacy_title", q.id)
        },
        &[],
    )
}
fn item_name(item: &str) -> Text {
    let kind = if matches!(
        item,
        "minecraft:bread" | "minecraft:wheat" | "minecraft:stone_pickaxe"
    ) {
        "item"
    } else {
        "block"
    };
    Text::tr(&format!("{kind}.{}", item.replace(':', ".")), &[])
}
fn button(key: &str, command: String) -> Text {
    Text::tr(key, &[]).with_style(&Style {
        click: Some(ClickEvent::RunCommand(command)),
        underlined: Some(true),
        ..Default::default()
    })
}

impl Game<'_> {
    pub(super) fn civ_click_charter(&mut self) -> bool {
        if self.remote.is_none() || !self.civ_holds_charter() {
            return false;
        }
        let Some(hit) = self.pick() else {
            return false;
        };
        if self
            .res
            .facts()
            .get(self.world.block(hit.pos[0], hit.pos[1], hit.pos[2]))
            .name
            .as_ref()
            != "minecraft:bell"
        {
            return false;
        }
        self.send_command(&format!(
            "civ charter {} {} {}",
            hit.pos[0], hit.pos[1], hit.pos[2]
        ));
        true
    }
    pub(super) fn civ_list_quests(&mut self) -> Result<String, String> {
        let player = self.civ_player_id();
        let city = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?
            .clone();
        let owner = self
            .civilization
            .registry
            .settlement(&city.settlement_id)
            .unwrap()
            .npc_owner
            .clone();
        if let Some(plan) = &city.needs {
            self.system_message(Text::tr(
                "horizon.civilization.quest.plan",
                &[&plan.population, &plan.beds],
            ));
        }
        for q in &city.quests {
            let state = if q.completed_by.is_some() {
                "horizon.civilization.quest.completed"
            } else {
                "horizon.civilization.quest.open"
            };
            self.system_message(
                title(q)
                    .append(Text::literal(" — "))
                    .append(Text::tr(state, &[]))
                    .append(Text::literal(" "))
                    .append(button(
                        "horizon.civilization.quest.talk_button",
                        format!("/civ talk {}", q.id),
                    )),
            );
            if let Ok(npc) = self.owned_npc_state(&owner, &q.npc_key) {
                self.system_message(Text::tr(
                    "horizon.civilization.quest.location",
                    &[
                        &npc.position.x.round(),
                        &npc.position.y.round(),
                        &npc.position.z.round(),
                    ],
                ));
            }
        }
        Ok("horizon.civilization.quest.list_hint".into())
    }

    pub(super) fn civ_talk_resident(&mut self, key: &str) -> Result<String, String> {
        let player = self.civ_player_id();
        let city = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?
            .clone();
        let town = self
            .civilization
            .registry
            .settlement(&city.settlement_id)
            .unwrap()
            .clone();
        if !town.residents.iter().any(|r| r == key) {
            return Err("horizon.civilization.quest.foreign_resident".into());
        }
        self.civ_near_speaker(&town.npc_owner, key)?;
        if let Some(q) = city.quests.iter().find(|q| q.npc_key == key) {
            self.civ_talk_quest(&q.id)
        } else {
            Ok("horizon.civilization.quest.no_order".into())
        }
    }

    pub(super) fn civ_talk_quest(&mut self, id: &str) -> Result<String, String> {
        let player = self.civ_player_id();
        let city = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?
            .clone();
        let q = city
            .quests
            .iter()
            .find(|q| q.id == id)
            .ok_or("horizon.civilization.quest.unknown")?;
        let owner = self
            .civilization
            .registry
            .settlement(&city.settlement_id)
            .unwrap()
            .npc_owner
            .clone();
        self.civ_near_speaker(&owner, &q.npc_key)?;
        self.system_message(title(q));
        if let Some(plan) = &city.needs {
            let reason = Text::tr(
                &format!("horizon.civilization.quest.{}.reason", q.id),
                &[&plan.population, &plan.beds, &plan.housing_kits()],
            );
            self.system_message(reason);
        }
        for m in &q.materials {
            let received = q
                .delivery
                .as_ref()
                .and_then(|d| d.received.get(&m.item))
                .copied()
                .unwrap_or(if q.completed_by.is_some() { m.count } else { 0 });
            self.system_message(Text::translate(
                "horizon.civilization.quest.material",
                &[
                    item_name(&m.item),
                    Text::literal(received.to_string()),
                    Text::literal(m.count.to_string()),
                ],
            ));
        }
        if let Some(d) = &q.delivery
            && !d.contributors.is_empty()
        {
            self.system_message(Text::tr(
                "horizon.civilization.quest.contributors",
                &[&d.contributors.len()],
            ));
        }
        if q.completed_by.is_some() {
            return Ok("horizon.civilization.quest.completed".into());
        }
        self.system_message(button(
            "horizon.civilization.quest.deliver_button",
            format!("/civ deliver {}", q.id),
        ));
        Ok(if q.delivery.is_some() {
            "horizon.civilization.quest.partial_hint"
        } else {
            "horizon.civilization.quest.legacy_hint"
        }
        .into())
    }

    pub(super) fn civ_deliver_quest(&mut self, id: &str) -> Result<String, String> {
        let player = self.civ_player_id();
        let q = self.civilization.registry.quest(&player, id)?.clone();
        let city = self
            .civilization
            .registry
            .civilization_for(&player)
            .unwrap();
        let owner = self
            .civilization
            .registry
            .settlement(&city.settlement_id)
            .unwrap()
            .npc_owner
            .clone();
        self.civ_near_speaker(&owner, &q.npc_key)?;
        let old = self.civilization.registry.clone();
        let inventory = self.inventory.clone();
        let offhand = self.offhand.clone();
        let result = if q.delivery.is_some() {
            self.civ_take_available_materials(
                &q.remaining()
                    .iter()
                    .map(|m| (m.item.clone(), m.count))
                    .collect::<Vec<_>>(),
            )
            .and_then(|receipt| {
                self.civilization
                    .registry
                    .record_delivery(&player, id, &receipt)
            })
        } else {
            self.civ_take_materials(
                &q.materials
                    .iter()
                    .map(|m| (m.item.clone(), m.count))
                    .collect::<Vec<_>>(),
            )
            .and_then(|_| self.civilization.registry.complete_quest(&player, id))
            .map(|_| true)
        };
        let result = result.and_then(|complete| self.save_civilization().map(|_| complete));
        let complete = match result {
            Ok(c) => c,
            Err(e) => {
                self.civilization.registry = old;
                self.inventory = inventory;
                self.offhand = offhand;
                return Err(e);
            }
        };
        if !complete {
            self.civ_talk_quest(id)?;
            return Ok("horizon.civilization.quest.partial_saved".into());
        }
        self.system_message(Text::translate(
            "horizon.civilization.quest.reward",
            &[title(&q)],
        ));
        self.system_message(button(
            "horizon.civilization.council.open",
            "/civ panel".into(),
        ));
        Ok("horizon.civilization.quest.completed_hint".into())
    }

    /// Mirrors are only a local hit target. The ordinary authenticated command
    /// goes through the server's membership, dimension, distance and visibility checks.
    pub(super) fn civ_click_resident(&mut self) -> bool {
        if self.remote.is_none() || self.guest_use.is_some() {
            return false;
        }
        let Some((origin, dir)) = self.aim() else {
            return false;
        };
        let origin = horizon_core::geometry::Vec3::new(origin[0], origin[1], origin[2]);
        let reach = self.entity_reach;
        let end = horizon_core::geometry::Vec3::new(
            origin.x + dir[0] * reach,
            origin.y + dir[1] * reach,
            origin.z + dir[2] * reach,
        );
        let target = self
            .owned_npc_mirrors
            .iter()
            .filter(|m| {
                m.npc.owner == "civilization" || m.npc.owner.starts_with("civilization.town-")
            })
            .filter_map(|m| {
                let p = m.body.position;
                let (width, height, _) = m.body.dims();
                let w = f64::from(width) * 0.5;
                let bounds = horizon_core::geometry::Aabb::new(
                    horizon_core::geometry::Vec3::new(p.x - w, p.y, p.z - w),
                    horizon_core::geometry::Vec3::new(p.x + w, p.y + f64::from(height), p.z + w),
                );
                bounds.clip(origin, end).map(|hit| {
                    (
                        (hit.x - origin.x).powi(2)
                            + (hit.y - origin.y).powi(2)
                            + (hit.z - origin.z).powi(2),
                        m.npc.stable_key.clone(),
                    )
                })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        let Some((distance, key)) = target else {
            return false;
        };
        if self
            .clip_ray([origin.x, origin.y, origin.z], dir, reach)
            .is_some_and(|h| {
                (h.point[0] - origin.x).powi(2)
                    + (h.point[1] - origin.y).powi(2)
                    + (h.point[2] - origin.z).powi(2)
                    < distance
            })
        {
            return false;
        }
        self.send_command(&format!("civ resident {key}"));
        true
    }
}
