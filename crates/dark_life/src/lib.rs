//! Life (docs/PLAN.md §4.6, §14): bodies with real needs, in a world with drink and weather.
//!
//! Low fantasy is a pillar: bodies get hungry, thirsty, tired, need the toilet and get dirty;
//! drink makes them merry, then clumsy, then unconscious; cold kills without warm clothes, a
//! fire, shelter or heating magic. Everything here is plain integer state stepped a game minute
//! at a time ([`Body::minute`]), so the host runs it for every character and a night's sleep is
//! just many minutes. What it means for a character (slower, staggering, out cold) is its
//! [`Condition`]; the character controller applies that.

mod body;
mod def;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use body::{
    Body, Condition, Consumable, Drunk, FULL, Level, Minute, Need, SCALE, Shelter, Status,
    Surroundings, Temperature, WARM_CORE,
};
pub use def::{
    Climate, CoreLevels, DrunkLevels, Harm, IconDef, ItemDef, ItemUse, LifeDef, LifeError,
    NeedRates, Rates, Structure, StructureArt,
};

/// Things carried, in the order they were first picked up (the hotbar's order), and what is worn.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inventory {
    items: Vec<(String, u16)>,
    pub worn: Option<String>,
}

impl Inventory {
    pub fn with(start: &[(String, u16)]) -> Self {
        let mut inventory = Self::default();
        for (id, count) in start {
            inventory.add(id, *count);
        }
        inventory
    }

    pub fn add(&mut self, id: &str, count: u16) {
        match self.items.iter_mut().find(|(i, _)| i == id) {
            Some((_, n)) => *n = n.saturating_add(count),
            None => self.items.push((id.to_owned(), count)),
        }
    }

    pub fn count(&self, id: &str) -> u16 {
        self.items
            .iter()
            .find(|(i, _)| i == id)
            .map_or(0, |(_, n)| *n)
    }

    /// Removes one; false if there was none. An empty stack keeps its place on the hotbar.
    pub fn take(&mut self, id: &str) -> bool {
        match self.items.iter_mut().find(|(i, n)| i == id && *n > 0) {
            Some((_, n)) => {
                *n -= 1;
                true
            }
            None => false,
        }
    }

    pub fn slots(&self) -> &[(String, u16)] {
        &self.items
    }

    /// Warmth of what is worn.
    pub fn insulation(&self, items: &BTreeMap<String, ItemDef>) -> i32 {
        match self
            .worn
            .as_ref()
            .and_then(|id| items.get(id))
            .map(|d| &d.use_)
        {
            Some(ItemUse::Wear { insulation }) => *insulation,
            _ => 0,
        }
    }
}

/// What using an item did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Used {
    /// Nothing there, or nothing left.
    Nothing,
    Consumed(String),
    PutOn(String),
    TookOff(String),
    Warmed(String),
    Washed(String),
    /// Taken out to be set down: the caller places it in the world.
    Place(String, Structure),
}

/// Uses what is in hotbar `slot`.
pub fn use_item(
    body: &mut Body,
    inventory: &mut Inventory,
    items: &BTreeMap<String, ItemDef>,
    slot: usize,
) -> Used {
    let Some((id, count)) = inventory.slots().get(slot).cloned() else {
        return Used::Nothing;
    };
    let Some(def) = items.get(&id) else {
        return Used::Nothing;
    };
    // Clothes are put on and taken off, not used up, so they need not be in hand twice.
    if let ItemUse::Wear { .. } = def.use_ {
        if inventory.worn.as_deref() == Some(id.as_str()) {
            inventory.worn = None;
            return Used::TookOff(id);
        }
        if count == 0 {
            return Used::Nothing;
        }
        inventory.worn = Some(id.clone());
        return Used::PutOn(id);
    }
    if !inventory.take(&id) {
        return Used::Nothing;
    }
    match &def.use_ {
        ItemUse::Consume(what) => {
            body.eat_or_drink(what);
            Used::Consumed(id)
        }
        ItemUse::Warm { bonus, minutes } => {
            body.warm(*bonus, *minutes);
            Used::Warmed(id)
        }
        ItemUse::Place(structure) => Used::Place(id, *structure),
        ItemUse::Wash => {
            body.wash();
            Used::Washed(id)
        }
        // Money and the like: carried, not used, and not spent by pressing its key.
        ItemUse::Keep => Used::Nothing,
        ItemUse::Wear { .. } => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests;
