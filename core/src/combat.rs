//! Pure, deterministic combat simulation — the feasibility brain for the plan
//! pass. No I/O, no clock, no RNG: given two stat blocks it returns whether the
//! player wins, in how many turns, and with how much HP to spare.
//!
//! We commit to a single approach: **deterministic, critical strikes OFF**.
//! Crits only ever help the player (1.5× damage), so a crit-off win is a true
//! lower bound — if the simulator says "win", the real fight is guaranteed
//! winnable. A probabilistic Monte Carlo win-% is intentionally NOT modelled.
//!
//! The damage formula is quoted from the live docs
//! (<https://docs.artifactsmmo.com/concepts/stats_and_fights>):
//!
//! ```text
//! Elemental attack = Round(Base elemental attack × (1 + Total damage bonus / 100))
//! Final damage     = Round(Elemental attack × (1 - Resistance / 100))
//! ```
//!
//! where `Total damage bonus = global damage% + that element's dmg%`, each point
//! of resistance reduces by 1%, the four elements are computed independently and
//! summed, and `.5` always rounds up. Turn order is decided by `initiative`
//! (highest acts first; tie → higher HP); a fight that is not over within 100
//! turns is a loss.

use crate::step::{CharacterView, FightOutcome};

/// Element index order shared by every `[i32; 4]` stat array below.
/// fire, earth, water, air.
pub const FIRE: usize = 0;
pub const EARTH: usize = 1;
pub const WATER: usize = 2;
pub const AIR: usize = 3;

/// Hard cap on fight length; reaching it is a loss (live rule).
pub const MAX_TURNS: u32 = 100;

/// One combatant's stat block — identical shape for player and monster, so the
/// simulator is symmetric. `attack`/`res`/`dmg` are indexed by the element
/// constants above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombatStats {
    pub hp: i32,
    pub initiative: i32,
    pub attack: [i32; 4],
    pub res: [i32; 4],
    /// Per-element damage bonus % (from gear). Monsters have all zero.
    pub dmg: [i32; 4],
    /// Global damage bonus %, added to every element's `dmg`.
    pub global_dmg: i32,
    /// Crit chance %. Recorded for completeness; the deterministic sim ignores it.
    pub critical_strike: i32,
    /// Cooldown reduction % (1 = 1% off the fight cooldown). Not a combat-
    /// resolution stat — `simulate` ignores it — but it rides along here because
    /// the fight `:cost` needs it and it's a per-combatant character stat.
    pub haste: i32,
}

impl From<&CharacterView> for CombatStats {
    fn from(v: &CharacterView) -> Self {
        Self {
            hp: v.hp as i32,
            initiative: v.initiative,
            attack: [v.attack_fire, v.attack_earth, v.attack_water, v.attack_air],
            res: [v.res_fire, v.res_earth, v.res_water, v.res_air],
            dmg: [v.dmg_fire, v.dmg_earth, v.dmg_water, v.dmg_air],
            global_dmg: v.dmg,
            critical_strike: v.critical_strike,
            haste: v.haste,
        }
    }
}

/// Round half up, matching the live engine ("`.5` always rounds up"). Inputs are
/// non-negative (attack/damage are clamped at 0), so this is exact.
fn round_half_up(x: f64) -> i32 {
    (x + 0.5).floor() as i32
}

/// Damage one combatant deals to another in a single hit, crits off: sum the
/// four elements independently per the quoted formula. Never negative (a
/// resistance above 100% can't heal the target).
fn hit_damage(attacker: &CombatStats, defender: &CombatStats) -> i32 {
    let mut total = 0;
    for e in 0..4 {
        let bonus = attacker.global_dmg + attacker.dmg[e];
        let atk = round_half_up(attacker.attack[e] as f64 * (1.0 + bonus as f64 / 100.0));
        let dealt = round_half_up(atk as f64 * (1.0 - defender.res[e] as f64 / 100.0));
        total += dealt.max(0);
    }
    total
}

/// The prediction returned to the plan pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FightPrediction {
    pub result: FightOutcome,
    /// Total turns (each combatant's action is one turn, matching the live
    /// `fight.turns` field), so cooldown prediction lines up with the server.
    pub turns: u32,
    /// Player HP left on a win (0 on a loss).
    pub player_hp_remaining: i32,
}

/// True if the player acts first: higher initiative wins, ties break on HP.
fn player_acts_first(player: &CombatStats, monster: &CombatStats) -> bool {
    use std::cmp::Ordering::*;
    match player.initiative.cmp(&monster.initiative) {
        Greater => true,
        Less => false,
        Equal => player.hp >= monster.hp,
    }
}

/// Resolve a fight deterministically with crits off. Both combatants deal a
/// fixed per-hit amount, alternating in initiative order, until one reaches 0 HP
/// or the turn cap (a loss) is hit.
pub fn simulate(player: &CombatStats, monster: &CombatStats) -> FightPrediction {
    let mut player_hp = player.hp;
    let mut monster_hp = monster.hp;
    let player_dmg = hit_damage(player, monster);
    let monster_dmg = hit_damage(monster, player);

    let mut player_turn = player_acts_first(player, monster);
    let mut turns = 0;
    while turns < MAX_TURNS {
        turns += 1;
        if player_turn {
            monster_hp -= player_dmg;
            if monster_hp <= 0 {
                return FightPrediction {
                    result: FightOutcome::Win,
                    turns,
                    player_hp_remaining: player_hp.max(0),
                };
            }
        } else {
            player_hp -= monster_dmg;
            if player_hp <= 0 {
                return FightPrediction {
                    result: FightOutcome::Lose,
                    turns,
                    player_hp_remaining: 0,
                };
            }
        }
        player_turn = !player_turn;
    }

    // Not over within the cap → a loss, however much HP remains.
    FightPrediction {
        result: FightOutcome::Lose,
        turns: MAX_TURNS,
        player_hp_remaining: player_hp.max(0),
    }
}

// ─── Monster reference data (GET /monsters/{code}) ───────────────────────────

/// A single drop entry: a `1/rate` chance per win of `min_quantity..=max_quantity`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MonsterDrop {
    pub code: String,
    pub rate: u32,
    pub min_quantity: u32,
    pub max_quantity: u32,
}

/// Monster stat block as returned by the live `/monsters` endpoint. This is the
/// static reference data the simulator consumes; it is fetched and cached, never
/// hardcoded.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct MonsterView {
    pub code: String,
    pub name: String,
    pub level: u32,
    pub hp: i32,
    pub attack_fire: i32,
    pub attack_earth: i32,
    pub attack_water: i32,
    pub attack_air: i32,
    pub res_fire: i32,
    pub res_earth: i32,
    pub res_water: i32,
    pub res_air: i32,
    #[serde(default)]
    pub critical_strike: i32,
    #[serde(default)]
    pub initiative: i32,
    #[serde(default)]
    pub drops: Vec<MonsterDrop>,
}

impl MonsterView {
    pub fn combat_stats(&self) -> CombatStats {
        CombatStats {
            hp: self.hp,
            initiative: self.initiative,
            attack: [
                self.attack_fire,
                self.attack_earth,
                self.attack_water,
                self.attack_air,
            ],
            res: [self.res_fire, self.res_earth, self.res_water, self.res_air],
            dmg: [0; 4],
            global_dmg: 0,
            critical_strike: self.critical_strike,
            haste: 0,
        }
    }
}

/// Paginated `/monsters` response.
#[derive(Debug, serde::Deserialize)]
pub struct MonstersPage {
    pub data: Vec<MonsterView>,
    pub total: u32,
    pub page: u32,
    pub size: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ground-truth fixture captured live: character `nillinbot`
    /// (init 100, hp 120, earth attack 4) vs `chicken` (init 50, hp 60, water
    /// attack 4). The real `action/fight` returned turns=29, win, final_hp 64.
    fn nillinbot() -> CombatStats {
        CombatStats {
            hp: 120,
            initiative: 100,
            attack: [0, 4, 0, 0],
            res: [0; 4],
            dmg: [0; 4],
            global_dmg: 0,
            critical_strike: 5,
            haste: 0,
        }
    }

    fn chicken() -> CombatStats {
        CombatStats {
            hp: 60,
            initiative: 50,
            attack: [0, 0, 4, 0],
            res: [0; 4],
            dmg: [0; 4],
            global_dmg: 0,
            critical_strike: 0,
            haste: 0,
        }
    }

    #[test]
    fn matches_live_chicken_fight() {
        let p = simulate(&nillinbot(), &chicken());
        assert_eq!(p.result, FightOutcome::Win);
        assert_eq!(p.turns, 29, "live fight took 29 turns");
        assert_eq!(p.player_hp_remaining, 64, "live fight ended at 64/120");
    }

    #[test]
    fn damage_bonus_and_resistance_round_half_up() {
        // 4 earth attack, +10% global dmg → 4 × 1.1 = 4.4 → rounds to 4.
        let mut p = nillinbot();
        p.global_dmg = 10;
        assert_eq!(hit_damage(&p, &chicken()), 4);
        // +13% → 4 × 1.13 = 4.52 → rounds up to 5.
        p.global_dmg = 13;
        assert_eq!(hit_damage(&p, &chicken()), 5);
        // 50% earth resistance on the defender → 4 × 0.5 = 2.
        let mut def = chicken();
        def.res[EARTH] = 50;
        assert_eq!(hit_damage(&nillinbot(), &def), 2);
    }

    #[test]
    fn a_player_who_cannot_win_loses_when_killed() {
        // 0 attack → can't scratch the chicken, but the chicken still kills the
        // player (4/turn vs 120 hp): loss well before the turn cap.
        let mut weak = nillinbot();
        weak.attack = [0; 4];
        let p = simulate(&weak, &chicken());
        assert_eq!(p.result, FightOutcome::Lose);
        assert!(p.turns < MAX_TURNS, "player dies before the cap");
    }

    #[test]
    fn a_stalemate_hits_the_turn_cap_as_a_loss() {
        // Neither side can damage the other → 100-turn cap → loss (not a hang).
        let mut a = nillinbot();
        a.attack = [0; 4];
        let mut b = chicken();
        b.attack = [0; 4];
        let p = simulate(&a, &b);
        assert_eq!(p.result, FightOutcome::Lose);
        assert_eq!(p.turns, MAX_TURNS);
    }

    #[test]
    fn initiative_decides_who_strikes_first() {
        // A glass-cannon duel where striking first is the whole game: both deal
        // lethal damage in one hit, so whoever acts first wins.
        let a = CombatStats {
            hp: 10,
            initiative: 20,
            attack: [0, 100, 0, 0],
            res: [0; 4],
            dmg: [0; 4],
            global_dmg: 0,
            critical_strike: 0,
            haste: 0,
        };
        let mut slow = a.clone();
        slow.initiative = 5;
        assert_eq!(simulate(&a, &slow).result, FightOutcome::Win);
        assert_eq!(simulate(&slow, &a).result, FightOutcome::Lose);
    }
}
