//! Race choices are player data, independent of appearance and civilization membership.
use super::Bell;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Race {
    Human,
    DesertCat,
}
impl Race {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "desert-cat" => Some(Self::DesertCat),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::DesertCat => "desert-cat",
        }
    }
    pub fn name_key(self) -> &'static str {
        match self {
            Self::Human => "horizon.civilization.race.human",
            Self::DesertCat => "horizon.civilization.race.desert_cat",
        }
    }
    pub fn structure(self) -> &'static str {
        match self {
            Self::Human => "minecraft:village_plains",
            Self::DesertCat => "minecraft:village_desert",
        }
    }
    pub fn habitat(self, biome: &str) -> bool {
        match self {
            Self::Human => matches!(
                biome,
                "minecraft:plains" | "minecraft:sunflower_plains" | "minecraft:meadow"
            ),
            Self::DesertCat => biome == "minecraft:desert",
        }
    }
    /// An owned attribute modifier, never a replacement for potion/equipment modifiers.
    pub fn movement_bonus(self, biome: &str, sand: bool) -> f64 {
        if self == Self::Human {
            return 0.0;
        }
        if biome.contains("snow")
            || biome.contains("frozen")
            || biome.contains("ice")
            || matches!(biome, "minecraft:grove" | "minecraft:jagged_peaks")
        {
            -0.15
        } else if sand {
            0.20
        } else {
            0.10
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub settlement: Bell,
    pub structure: String,
    pub position: [i32; 3],
    pub biome: String,
}
impl Start {
    pub(super) fn valid(&self, race: Race) -> bool {
        super::valid_bell(&self.settlement)
            && (self.structure == race.structure() || self.structure == "civilization:settlement")
            && (self.structure != "civilization:settlement"
                || (64..=287).contains(&self.settlement.y))
            && race.habitat(&self.biome)
            && (-29_999_900..=29_999_900).contains(&self.position[0])
            && (-29_999_900..=29_999_900).contains(&self.position[2])
            && (-63..=317).contains(&self.position[1])
            && (i64::from(self.position[0]) - i64::from(self.settlement.x)).abs() <= 64
            && (i64::from(self.position[2]) - i64::from(self.settlement.z)).abs() <= 64
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub race: Race,
    pub start: Start,
    /// The server loads and checks real blocks before moving the player; resumes on login.
    pub arriving: bool,
    #[serde(default)]
    pub settlement_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn climate_and_sand_have_opposing_effects_without_affecting_humans() {
        assert_eq!(
            Race::DesertCat.movement_bonus("minecraft:desert", false),
            0.10
        );
        assert_eq!(
            Race::DesertCat.movement_bonus("minecraft:desert", true),
            0.20
        );
        assert_eq!(
            Race::DesertCat.movement_bonus("minecraft:snowy_plains", true),
            -0.15
        );
        assert_eq!(
            Race::Human.movement_bonus("minecraft:snowy_plains", true),
            0.0
        );
        assert!(!Race::DesertCat.habitat("minecraft:plains"));
    }
}
