//! Fights resolved statistically, for the parts of the world no player is watching.
//! Integer maths only, so a seed gives the same year on every machine.

use crate::def::Tuning;
use crate::rng::Rng;
use crate::world::{ActorId, World};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BattleResult {
    pub won: bool,
    /// Fighters whose wounds were fatal; the caller kills them (titles pass on).
    pub deaths: Vec<ActorId>,
}

/// The chance, as `(numerator, denominator)`, that strength `ours` beats strength `theirs`:
/// `ours^k / (ours^k + theirs^k)`.
pub fn win_odds(ours: u64, theirs: u64, sharpness: u32) -> (u128, u128) {
    let k = sharpness.clamp(1, 4);
    let (s, e) = (u128::from(ours).pow(k), u128::from(theirs).pow(k));
    (s, s + e)
}

/// `fighters` fight an enemy of strength `enemy`. Everyone is wounded, the losers more; a
/// winner's survivors learn from it.
pub fn resolve(
    world: &mut World,
    fighters: &[ActorId],
    enemy: u64,
    rng: &mut Rng,
    tuning: &Tuning,
) -> BattleResult {
    let ours: u64 = fighters.iter().map(|&id| world.get(id).strength()).sum();
    if ours == 0 {
        return BattleResult {
            won: false,
            deaths: Vec::new(),
        };
    }
    let (num, den) = win_odds(ours, enemy, tuning.sharpness);
    let won = rng.chance(num, den);
    // Against an equal enemy a fighter loses `wound_percent` health, more against a stronger one.
    let base = u64::from(tuning.wound_percent) * 2 * enemy / (ours + enemy);
    let mut deaths = Vec::new();
    for &id in fighters {
        let mut wound = base * u64::from(rng.range(50, 150)) / 100;
        if won {
            wound /= 2;
        }
        let actor = world.get_mut(id);
        if wound >= u64::from(actor.health) {
            deaths.push(id);
        } else {
            actor.health -= wound as u32;
        }
    }
    if won {
        let survivors = fighters.len() - deaths.len();
        if survivors > 0 {
            let share = (enemy / u64::from(tuning.xp_divisor.max(1)) / survivors as u64).max(1);
            for &id in fighters.iter().filter(|id| !deaths.contains(id)) {
                let actor = world.get_mut(id);
                actor.power = actor.power.saturating_add(share as u32);
            }
        }
    }
    BattleResult { won, deaths }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stronger_sides_win_more_often() {
        let (n, d) = win_odds(200, 100, 3);
        assert_eq!((n, d), (8_000_000, 9_000_000), "8 in 9");
        let (n, d) = win_odds(100, 100, 3);
        assert_eq!(n * 2, d, "even fight, even odds");
        let (n, d) = win_odds(0, 100, 3);
        assert_eq!((n, d), (0, 1_000_000));
    }
}
