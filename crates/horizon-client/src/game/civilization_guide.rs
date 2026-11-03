//! Ordinary written-book components keep the menu available to both local and LAN clients.
use horizon_client::{
    item_data::{StackData, WrittenBook},
    items::ItemStack,
    scoreboard::json_to_tag,
};
use horizon_mods::civilization::races::Choice;
use horizon_world::nbt::Tag;
use serde_json::{Value, json};

pub(super) fn owned(stack: &ItemStack, key: &str) -> bool {
    stack
        .data()
        .and_then(|d| d.other("minecraft:custom_data"))
        .and_then(|t| t.get(key))
        == Some(&Tag::Byte(1))
}
pub(super) fn is_guide(stack: &ItemStack) -> bool {
    stack.item.as_ref() == "minecraft:written_book"
        && (owned(stack, "horizon_civ_guide")
            || stack
                .data()
                .and_then(|d| d.written.as_ref())
                .is_some_and(|w| w.title == "Путь общины" && w.author == "Совет жителей"))
}
fn line(key: &str) -> Value {
    json!({"translate":key})
}
fn link(key: &str, action: &str, value: Value) -> Value {
    let mut node = line(key);
    let field = if action == "change_page" {
        "page"
    } else {
        "command"
    };
    node["color"] = "dark_blue".into();
    node["underlined"] = true.into();
    node["click_event"] = json!({"action":action,field:value});
    node
}
pub(super) fn guide_book(choice: Option<&Choice>) -> ItemStack {
    let select = |key, id| {
        if choice.is_some() {
            line("horizon.civilization.book.fixed")
        } else {
            link(key, "run_command", format!("/civ race choose {id}").into())
        }
    };
    let menu = match choice {
        Some(c) => vec![
            line("horizon.civilization.book.selected"),
            line(c.race.name_key()),
            link(
                "horizon.civilization.book.compass",
                "run_command",
                "/civ compass".into(),
            ),
            link("horizon.civilization.book.begin", "change_page", 4.into()),
        ],
        None => vec![
            line("horizon.civilization.book.menu"),
            link(
                "horizon.civilization.book.human_link",
                "change_page",
                2.into(),
            ),
            link(
                "horizon.civilization.book.cat_link",
                "change_page",
                3.into(),
            ),
            line("horizon.civilization.book.choice_note"),
        ],
    };
    let pages = [
        menu,
        vec![
            line("horizon.civilization.book.human_body"),
            select("horizon.civilization.book.choose", "human"),
            link("horizon.civilization.book.back", "change_page", 1.into()),
        ],
        vec![
            line("horizon.civilization.book.cat_body"),
            select("horizon.civilization.book.choose", "desert-cat"),
            link("horizon.civilization.book.back", "change_page", 1.into()),
        ],
        vec![
            line("horizon.civilization.book.instructions"),
            link(
                "horizon.civilization.council.open",
                "run_command",
                "/civ panel".into(),
            ),
            link("horizon.civilization.book.back", "change_page", 1.into()),
        ],
    ];
    let tags: Vec<_> = pages
        .into_iter()
        .map(|nodes| json_to_tag(&json!({"text":"","extra":nodes})))
        .collect();
    let mut book = ItemStack::new("minecraft:written_book", 1);
    book.extra = Some(Box::new(StackData {
        custom_name: Some(json_to_tag(&line("horizon.civilization.book.title"))),
        written: Some(WrittenBook {
            title: horizon_client::text::Text::tr("horizon.civilization.book.title", &[])
                .plain()
                .into(),
            author: horizon_client::text::Text::tr("horizon.civilization.book.author", &[])
                .plain()
                .into(),
            pages: tags
                .iter()
                .map(|t| horizon_client::entity_nbt::name_text(t).plain().to_owned())
                .collect(),
            page_tags: tags,
            ..Default::default()
        }),
        other: vec![(
            "minecraft:custom_data".into(),
            Tag::compound([("horizon_civ_guide", Tag::Byte(1))]),
        )],
        ..Default::default()
    }));
    book
}
pub(super) fn compass(target: Option<&horizon_mods::civilization::Bell>) -> ItemStack {
    let mut stack = ItemStack::new("minecraft:compass", 1);
    stack.data_mut().custom_name = Some(json_to_tag(&line("horizon.civilization.compass.name")));
    stack.data_mut().set_other(
        "minecraft:custom_data",
        Some(Tag::compound([("horizon_civ_compass", Tag::Byte(1))])),
    );
    if let Some(b) = target {
        stack.data_mut().set_other(
            "minecraft:lodestone_tracker",
            Some(Tag::compound([
                ("tracked", Tag::Byte(0)),
                (
                    "target",
                    Tag::compound([
                        ("dimension", Tag::String(b.dimension.as_str().into())),
                        ("pos", Tag::IntArray(vec![b.x, b.y, b.z])),
                    ]),
                ),
            ])),
        );
    }
    stack
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_client::item_data::{decode_components, encode_components};
    #[test]
    fn book_buttons_survive_the_inventory_wire_and_keep_server_commands() {
        let book = guide_book(None);
        let (_, extra) = decode_components(encode_components(&book).as_ref());
        let written = extra.as_ref().unwrap().written.as_ref().unwrap();
        let page = horizon_client::scoreboard::tag_to_json(&written.page_tags[2]);
        assert_eq!(page["extra"][1]["click_event"]["action"], "run_command");
        assert_eq!(
            page["extra"][1]["click_event"]["command"],
            "/civ race choose desert-cat"
        );
        assert!(owned(
            &horizon_client::items::ItemStack { extra, ..book },
            "horizon_civ_guide"
        ));
    }
}
