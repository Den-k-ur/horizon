//! World and inventory adapters for Civilization's first village chapter.
//! All helpers run with the authoritative acting player's state in place.
use super::{Dim, Game, execution_role::ExecutionRole};
use horizon_client::{
    item_data::{StackData, WrittenBook},
    items::ItemStack,
    mobs::Species,
};
use horizon_world::nbt::Tag;
use std::collections::BTreeMap;

fn charter(stack: &ItemStack) -> bool {
    stack.item.as_ref() == "minecraft:paper"
        && stack.count > 0
        && stack.data().is_some_and(|data| {
            data.other.iter().any(|(key, value)| {
                key == "minecraft:custom_data"
                    && value.get("horizon_civ_charter") == Some(&Tag::Byte(1))
            })
        })
}

fn start_items() -> [ItemStack; 2] {
    let mut book = ItemStack::new("minecraft:written_book", 1);
    book.extra = Some(Box::new(StackData {
        written: Some(WrittenBook {
            title: "Путь общины".into(),
            author: "Совет жителей".into(),
            pages: vec![
                "Добро пожаловать! Найди деревню: колокол, минимум две кровати и один житель рядом. Возьми Грамоту общины и примени её на колокол. Здесь появится ратуша.".into(),
                "Ты пока гость. Помоги каменщику, хлебопашцу и строителю. /civ — состояние; /civ quests — просьбы. Рядом с нужным советником: /civ talk quarry, granary или housing.".into(),
                "Принеси обычные материалы без названий и улучшений. Рядом с советником: /civ deliver quarry, granary или housing. Все участники общины могут помогать. Просьбы и признание сохраняются.".into(),
                "После трёх проектов жители признают тебя старостой. Откроется обзор запасов и развития. Личные знания и крафт не ограничены технологиями общины. Армии и новые эпохи появятся позже.".into(),
            ],
            ..Default::default()
        }),
        ..Default::default()
    }));
    let mut paper = ItemStack::new("minecraft:paper", 1);
    paper.extra = Some(Box::new(StackData {
        custom_name: Some(Tag::String("Грамота общины".into())),
        other: vec![(
            "minecraft:custom_data".into(),
            Tag::compound([("horizon_civ_charter", Tag::Byte(1))]),
        )],
        ..Default::default()
    }));
    [book, paper]
}

/// Stage all removals on a copy. Missing materials never consume a partial cost.
fn take_materials(
    inventory: &mut [Option<ItemStack>],
    offhand: &mut Option<ItemStack>,
    requirements: &[(String, u32)],
) -> Result<(), String> {
    let mut totals = BTreeMap::<&str, u32>::new();
    for (item, count) in requirements {
        if !item.starts_with("minecraft:") || *count == 0 {
            return Err("Invalid project material requirement".into());
        }
        let total = totals.entry(item).or_default();
        *total = total.checked_add(*count).ok_or("Material count overflow")?;
    }
    let mut staged = inventory.to_vec();
    let mut hand = offhand.clone();
    for (item, required) in totals {
        let mut remaining = required;
        for slot in staged.iter_mut().chain(std::iter::once(&mut hand)) {
            if let Some(stack) = slot
                && stack.item.as_ref() == item
                && stack.damage == 0
                && stack.extra.is_none()
            {
                let removed = remaining.min(stack.count);
                remaining -= removed;
                stack.count -= removed;
                if stack.count == 0 {
                    *slot = None;
                }
            }
            if remaining == 0 {
                break;
            }
        }
        if remaining > 0 {
            return Err(format!(
                "Не хватает {item}: ещё {remaining}. Нужны обычные предметы без особых компонентов."
            ));
        }
    }
    inventory.clone_from_slice(&staged);
    *offhand = hand;
    Ok(())
}

impl Game<'_> {
    fn civ_require_server(&self) -> Result<(), String> {
        if self.options.execution_role == ExecutionRole::Client || self.remote.is_some() {
            return Err("Civilization changes require the authoritative server".into());
        }
        if self.player.is_none() {
            return Err("No acting player".into());
        }
        Ok(())
    }

    pub(super) fn civ_player_id(&mut self) -> String {
        self.acting_player_uuid()
            .map(|word| format!("{:08x}", word as u32))
            .concat()
    }

    /// A full inventory defers the starter grant; there is no world-drop loss.
    /// The caller persists the one-time grant only after this succeeds.
    pub(super) fn civ_give_start_items(&mut self) -> Result<(), String> {
        self.civ_require_server()?;
        let empty: Vec<usize> = self
            .inventory
            .iter()
            .take(36)
            .enumerate()
            .filter_map(|(i, stack)| stack.is_none().then_some(i))
            .take(2)
            .collect();
        if empty.len() < 2 {
            return Err("Освободи два места для книги и Грамоты общины".into());
        }
        for (slot, stack) in empty.into_iter().zip(start_items()) {
            self.inventory[slot] = Some(stack);
        }
        Ok(())
    }

    pub(super) fn civ_holds_charter(&self) -> bool {
        self.held().is_some_and(charter)
    }

    pub(super) fn civ_has_charter(&self) -> bool {
        self.inventory.iter().flatten().any(charter) || self.offhand.as_ref().is_some_and(charter)
    }

    /// Prefer the acting hand; chat's founding command may use another slot.
    pub(super) fn civ_consume_charter(&mut self) -> Result<(), String> {
        self.civ_require_server()?;
        let slot = if self.civ_holds_charter() {
            self.held_mut()
        } else if let Some(i) = self
            .inventory
            .iter()
            .position(|stack| stack.as_ref().is_some_and(charter))
        {
            &mut self.inventory[i]
        } else {
            &mut self.offhand
        };
        let stack = slot
            .as_mut()
            .filter(|s| charter(s))
            .ok_or("Нужна Грамота общины")?;
        stack.count -= 1;
        if stack.count == 0 {
            *slot = None;
        }
        Ok(())
    }

    /// Observable village evidence, not a claim that a generated structure was
    /// found. Player-built villages meeting the same requirements are accepted.
    pub(super) fn civ_village_evidence(&self, bell: [i32; 3]) -> Result<(u32, u32), String> {
        self.civ_require_server()?;
        if self.dim != Dim::Overworld {
            return Err("Община основывается в обычном мире".into());
        }
        if bell.iter().any(|v| v.unsigned_abs() > 29_999_900)
            || !self.world.loaded(bell[0], bell[2])
            || self
                .res
                .facts()
                .get(self.world.block(bell[0], bell[1], bell[2]))
                .name
                .as_ref()
                != "minecraft:bell"
        {
            return Err("Нужен загруженный деревенский колокол".into());
        }
        let player = &self.player.as_ref().ok_or("No acting player")?.position;
        let distance = (player.x - f64::from(bell[0]) - 0.5).powi(2)
            + (player.y - f64::from(bell[1]) - 0.5).powi(2)
            + (player.z - f64::from(bell[2]) - 0.5).powi(2);
        if distance > 36.0 {
            return Err("Подойди к колоколу ближе (не дальше 6 блоков)".into());
        }
        let mut beds = 0;
        for dx in -32_i32..=32 {
            for dz in -32_i32..=32 {
                if dx * dx + dz * dz > 32 * 32 || !self.world.loaded(bell[0] + dx, bell[2] + dz) {
                    continue;
                }
                for dy in -12..=12 {
                    let block = self.world.block(bell[0] + dx, bell[1] + dy, bell[2] + dz);
                    if let Some(state) = self.pack.assets.registry.state(block)
                        && state.name.ends_with("_bed")
                        && state
                            .properties
                            .get("part")
                            .is_some_and(|part| part == "head")
                    {
                        beds += 1;
                    }
                }
            }
        }
        let residents = self
            .mobs
            .iter()
            .filter(|mob| {
                mob.alive()
                    && mob.species() == Species::Villager
                    && (mob.position.x - f64::from(bell[0])).powi(2)
                        + (mob.position.z - f64::from(bell[2])).powi(2)
                        <= 32.0 * 32.0
                    && (mob.position.y - f64::from(bell[1])).abs() <= 12.0
            })
            .count() as u32;
        Ok((beds, residents))
    }

    pub(super) fn civ_take_materials(
        &mut self,
        requirements: &[(String, u32)],
    ) -> Result<(), String> {
        self.civ_require_server()?;
        take_materials(&mut self.inventory, &mut self.offhand, requirements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_material_is_transactional_and_protects_components() {
        let mut named = ItemStack::new("minecraft:oak_log", 64);
        named.extra = Some(Box::new(StackData {
            custom_name: Some(Tag::String("Keep".into())),
            ..Default::default()
        }));
        let mut inventory = vec![Some(ItemStack::new("minecraft:oak_log", 10)), Some(named)];
        let before = inventory.clone();
        let mut offhand = Some(ItemStack::new("minecraft:cobblestone", 31));
        let hand_before = offhand.clone();
        assert!(
            take_materials(
                &mut inventory,
                &mut offhand,
                &[
                    ("minecraft:oak_log".into(), 10),
                    ("minecraft:cobblestone".into(), 32)
                ]
            )
            .is_err()
        );
        assert_eq!(inventory, before);
        assert_eq!(offhand, hand_before);
        assert!(
            take_materials(
                &mut inventory,
                &mut offhand,
                &[("minecraft:oak_log".into(), 11)]
            )
            .is_err()
        );
        assert_eq!(inventory, before);
    }

    #[test]
    fn aggregate_cost_spans_slots_and_offhand_without_underpayment() {
        let mut inventory = vec![
            Some(ItemStack::new("minecraft:wheat", 10)),
            Some(ItemStack::new("minecraft:wheat", 8)),
        ];
        let mut offhand = Some(ItemStack::new("minecraft:wheat", 8));
        take_materials(
            &mut inventory,
            &mut offhand,
            &[
                ("minecraft:wheat".into(), 12),
                ("minecraft:wheat".into(), 12),
            ],
        )
        .unwrap();
        assert_eq!(inventory, vec![None, None]);
        assert_eq!(offhand.unwrap().count, 2);
    }

    #[test]
    fn charter_is_marked_data_and_guide_has_readable_pages() {
        let [book, charter_item] = start_items();
        assert!(charter(&charter_item));
        assert!(!charter(&ItemStack::new("minecraft:paper", 1)));
        assert_eq!(
            book.data().unwrap().written.as_ref().unwrap().pages.len(),
            4
        );
        let encoded = horizon_client::item_data::encode_components(&charter_item);
        let (_, extra) = horizon_client::item_data::decode_components(encoded.as_ref());
        assert!(charter(&ItemStack {
            extra,
            ..charter_item
        }));
    }
}
