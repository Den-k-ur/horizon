//! Physical warehouse transfers through the authenticated acting player's inventory.
use super::Game;
use horizon_client::{items::ItemStack, scoreboard::json_to_tag, text::Text};
use horizon_mods::civilization::{
    Stage,
    economy::{self, Operation},
};
use serde_json::{Value, json};

fn tr(key: &str) -> Value {
    json!({"translate":format!("horizon.civilization.stock.{key}")})
}
fn line(text: Text) -> Value {
    json!({"type":"minecraft:plain_message","width":360,"contents":text.component()})
}
fn action(key: &str, command: &str) -> Value {
    json!({"label":tr(key),"width":170,"action":{"type":"minecraft:run_command","command":command}})
}
impl Game<'_> {
    pub(super) fn civ_stock_panel(&mut self, page: &str) -> Result<String, String> {
        let player = self.civ_player_id();
        let c = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?;
        let s = c.economy.as_ref().unwrap();
        let admin = c.leader == player && c.stage == Stage::Elder;
        let mut body = vec![line(Text::tr(
            "horizon.civilization.stock.description",
            &[],
        ))];
        let mut inputs = Vec::new();
        let mut actions = Vec::new();
        match page {
            "stock" | "stock_history" => {
                if page == "stock_history" {
                    body.push(line(Text::tr(
                        "horizon.civilization.stock.imported",
                        &[&s.imported_food],
                    )));
                }
                for category in ["basic", "industrial"] {
                    body.push(line(Text::tr(
                        &format!("horizon.civilization.stock.{category}"),
                        &[],
                    )));
                    let mut entries = 0;
                    for (id, _) in economy::ITEMS.iter().filter(|(_, g)| *g == category) {
                        let number = |map: &std::collections::BTreeMap<String, u64>| {
                            map.get(*id).copied().unwrap_or(0)
                        };
                        if page == "stock" && s.count(id) == 0
                            || page == "stock_history"
                                && number(&s.deposited) == 0
                                && number(&s.withdrawn) == 0
                                && number(&s.consumed) == 0
                                && number(&s.produced) == 0
                                && s.count(id) == 0
                        {
                            continue;
                        }
                        entries += 1;
                        let details = if page == "stock_history" {
                            Text::tr(
                                "horizon.civilization.stock.audit",
                                &[
                                    &number(&s.deposited),
                                    &number(&s.withdrawn),
                                    &number(&s.consumed),
                                    &number(&s.produced),
                                ],
                            )
                        } else {
                            Text::literal(s.count(id).to_string())
                        };
                        body.push(line(
                            Text::tr(&item_key(id), &[])
                                .append(Text::literal(": "))
                                .append(details),
                        ));
                    }
                    if entries == 0 {
                        body.push(line(Text::tr("horizon.civilization.stock.empty", &[])));
                    }
                }
                if page == "stock_history" {
                    body.push(line(Text::tr("horizon.civilization.stock.future", &[])));
                }
                actions.push(action("deposit", "civ panel stock_deposit"));
                if admin {
                    actions.push(action("withdraw", "civ panel stock_withdraw"));
                    actions.push(action(
                        "planks",
                        &format!(
                            "civ stock planks {} {} minecraft:oak_log 1",
                            c.id, s.revision
                        ),
                    ));
                }
                actions.push(action("history", "civ panel stock_history"));
                actions.push(action("refresh", "civ panel stock"));
            }
            "stock_deposit" | "stock_withdraw" => {
                if page == "stock_withdraw" && !admin {
                    return Err("horizon.civilization.stock.leader_only".into());
                }
                body.push(line(Text::tr("horizon.civilization.stock.near_bell", &[])));
                let options: Vec<_> = economy::ITEMS
                    .iter()
                    .map(|(id, _)| json!({"id":id,"display":{"translate":item_key(id)}}))
                    .collect();
                inputs.push(json!({"type":"minecraft:single_option","key":"item","label":tr("item"),"width":300,"options":options}));
                inputs.push(json!({"type":"minecraft:text","key":"count","label":tr("count"),"initial":"1","width":300,"max_length":2}));
                let op = if page == "stock_deposit" {
                    "deposit"
                } else {
                    "withdraw"
                };
                actions.push(json!({"label":tr(op),"width":170,"action":{"type":"minecraft:dynamic/run_command","template":format!("civ stock {op} {} {} $(item) $(count)",c.id,s.revision)}}));
            }
            _ => return Err("horizon.civilization.stock.invalid".into()),
        }
        // A single text body avoids per-widget gaps; the native dialog still scrolls long accounts.
        let mut contents = Vec::new();
        for row in &body {
            if !contents.is_empty() {
                contents.push(json!({"text":"\n"}));
            }
            contents.push(row["contents"].clone());
        }
        body = vec![
            json!({"type":"minecraft:plain_message","width":360,"contents":{"text":"","extra":contents}}),
        ];
        actions.push(action("overview", "civ panel overview"));
        let dialog = json!({"type":"minecraft:multi_action","title":tr("title"),"pause":false,"can_close_with_escape":true,"after_action":"close","body":body,"inputs":inputs,"actions":actions,"columns":2,"exit_action":{"label":tr("close"),"width":170}});
        self.show_click_dialog(&json_to_tag(&dialog));
        Ok(String::new())
    }

    pub(super) fn civ_stock_command(&mut self, args: &[&str]) -> Result<String, String> {
        let invalid = "horizon.civilization.stock.invalid";
        let [op, city_id, revision, item, count] = args else {
            return Err(invalid.into());
        };
        let operation = match *op {
            "deposit" => Operation::Deposit,
            "withdraw" => Operation::Withdraw,
            "planks" => Operation::Planks,
            _ => return Err(invalid.into()),
        };
        let revision: u64 = revision.parse().map_err(|_| invalid)?;
        let count: u32 = count.parse().map_err(|_| invalid)?;
        if !(1..=64).contains(&count) || !economy::supported(item) {
            return Err(invalid.into());
        }
        let player = self.civ_player_id();
        let c = self
            .civilization
            .registry
            .civilization_for(&player)
            .ok_or("horizon.civilization.quest.no_city")?
            .clone();
        if self.spectator()
            || self
                .player
                .as_ref()
                .is_none_or(|p| p.health.health() <= 0.0)
        {
            return Err("horizon.civilization.quest.unavailable".into());
        }
        // The same real bell, distance, dimension and line of sight as foundation.
        self.civ_village_evidence([c.bell.x, c.bell.y, c.bell.z])?;
        let old = self.civilization.registry.clone();
        let inventory = self.inventory.clone();
        let offhand = self.offhand.clone();
        let result = (|| {
            self.civilization
                .registry
                .stock_operation(&player, city_id, revision, operation, item, count)?;
            match operation {
                Operation::Deposit => self.civ_take_materials(&[(item.to_string(), count)])?,
                Operation::Withdraw => {
                    let mut stack = ItemStack::new(*item, count);
                    if self.inventory.len() < 36
                        || super::inventory_add(
                            &mut self.inventory,
                            &mut self.offhand,
                            self.selected,
                            64,
                            &mut stack,
                        )
                    {
                        return Err("horizon.civilization.book.inventory_full".into());
                    }
                }
                Operation::Planks => {}
            }
            self.save_civilization()
        })();
        if let Err(e) = result {
            self.civilization.registry = old;
            self.inventory = inventory;
            self.offhand = offhand;
            return Err(e);
        }
        self.system_message(Text::tr("horizon.civilization.stock.updated", &[]));
        self.civ_stock_panel("stock")
    }
}
fn item_key(id: &str) -> String {
    let name = id.trim_start_matches("minecraft:");
    let prefix = if matches!(
        name,
        "bread" | "wheat" | "iron_ingot" | "copper_ingot" | "coal" | "charcoal"
    ) {
        "item"
    } else {
        "block"
    };
    format!("{prefix}.minecraft.{name}")
}
