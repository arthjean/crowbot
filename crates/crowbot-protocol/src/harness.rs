//! What a person allows the daemon to start on its own for a bot, and what
//! the turns of each origin consumed (decision D4 of the harness PRD).

use serde::{Deserialize, Serialize};

use crate::event::{Cost, TurnOrigin};

/// The highest daily limit of daemon-started turns a bot can have.
pub const DAILY_TURN_LIMIT_MAX: u32 = 10_000;
/// The highest daily limit of tokens of daemon-started turns a bot can have.
pub const DAILY_TOKEN_LIMIT_MAX: u64 = 1_000_000_000_000;

/// One switch per origin of a turn the daemon starts. A turn whose switch is
/// off does not start. Every switch but `retry` starts off, and only a
/// person turns one on, through a client message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnSwitches {
    #[serde(default)]
    pub routine: bool,
    #[serde(default)]
    pub review: bool,
    #[serde(default)]
    pub goal: bool,
    #[serde(default)]
    pub delegation: bool,
    #[serde(default = "on")]
    pub retry: bool,
    #[serde(default)]
    pub budget: bool,
}

fn on() -> bool {
    true
}

impl Default for TurnSwitches {
    fn default() -> Self {
        Self {
            routine: false,
            review: false,
            goal: false,
            delegation: false,
            retry: true,
            budget: false,
        }
    }
}

impl TurnSwitches {
    /// Whether a turn of `origin` may start. A person's turn always may.
    pub fn allows(&self, origin: TurnOrigin) -> bool {
        match origin {
            TurnOrigin::User => true,
            TurnOrigin::Routine => self.routine,
            TurnOrigin::Review => self.review,
            TurnOrigin::Goal => self.goal,
            TurnOrigin::Delegation => self.delegation,
            TurnOrigin::Retry => self.retry,
            TurnOrigin::Budget => self.budget,
        }
    }

    /// The origins whose switch is on, other than `Retry`: each needs a
    /// daily limit of turns.
    pub fn need_a_limit(&self) -> bool {
        self.routine || self.review || self.goal || self.delegation || self.budget
    }
}

/// What the daemon may start on its own for one bot, and how much of it a
/// day. Read and written by a client only: no daemon tool takes any of it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessSettings {
    #[serde(default)]
    pub switches: TurnSwitches,
    /// The turns the daemon may start for the bot in one local day. A switch
    /// other than `retry` turns on only once it is set.
    #[serde(default)]
    pub daily_turn_limit: Option<u32>,
    /// The tokens the daemon-started turns of one local day may consume, as
    /// their agents report them. A turn whose agent reports no tokens counts
    /// toward the limit of turns only.
    #[serde(default)]
    pub daily_token_limit: Option<u64>,
    /// A turn that edited files and ran nothing after is reminded once to
    /// verify its work.
    #[serde(default)]
    pub verification_reminder: bool,
}

/// Why a turn the daemon would have started did not start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnRefusal {
    /// The daemon is paused.
    Paused,
    /// The switch of the turn's origin is off.
    SwitchOff,
    /// The daemon-started turns of the day reached the daily limit.
    TurnLimit,
    /// The tokens of the day's daemon-started turns reached the daily limit.
    TokenLimit,
    #[serde(other)]
    Other,
}

/// What the turns of one origin consumed during the day, as their agents
/// reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginConsumption {
    pub origin: TurnOrigin,
    /// The turns of this origin that started.
    pub turns: u32,
    /// The turns of this origin that the daemon did not start.
    #[serde(default)]
    pub refused: u32,
    /// The turns whose agent reported tokens, and what they add up to.
    #[serde(default)]
    pub turns_with_tokens: u32,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    /// What the turns cost, one amount per currency.
    #[serde(default)]
    pub costs: Vec<Cost>,
}

/// What a bot's turns consumed during the daemon's local day, per origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Consumption {
    pub bot_id: i64,
    /// The local day, written `YYYY-MM-DD`.
    pub day: String,
    /// One entry per origin, `User` first.
    pub origins: Vec<OriginConsumption>,
}

/// A figure that may hold a fraction, compared by its bits: the daemon
/// shows it and decides nothing from it.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Figure(pub f64);

impl PartialEq for Figure {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}

impl Eq for Figure {}

/// The turns that loaded one version of a skill, and how they went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillVersionFigures {
    pub hash: String,
    pub turns: u32,
    /// The share of those turns that completed, from 0 to 1. `None` when no
    /// turn loaded the version.
    #[serde(default)]
    pub completed: Option<Figure>,
    /// The mean of the tokens their agents reported, over the turns that
    /// reported some.
    #[serde(default)]
    pub mean_tokens: Option<Figure>,
    /// The mean cost of a turn, one per currency, over the turns that
    /// reported a cost in it.
    #[serde(default)]
    pub mean_costs: Vec<Cost>,
}

/// The figures of one skill, per version, the oldest version first. A
/// version no turn loaded holds zero turns and no share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillFigures {
    pub bot_id: i64,
    pub name: String,
    pub versions: Vec<SkillVersionFigures>,
}

/// How often a bot loads the skills its index lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BotSkillFigures {
    pub bot_id: i64,
    /// The turns that ended while the index listed at least one skill.
    pub turns: u32,
    /// Those of them that loaded a skill the index listed.
    pub loaded: u32,
    /// `loaded` out of `turns`, from 0 to 1. `None` with no turn.
    #[serde(default)]
    pub share: Option<Figure>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_bot_retries_and_starts_nothing_else() {
        let settings: HarnessSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, HarnessSettings::default());
        let switches = settings.switches;
        assert!(switches.retry);
        for origin in [
            TurnOrigin::Routine,
            TurnOrigin::Review,
            TurnOrigin::Goal,
            TurnOrigin::Delegation,
            TurnOrigin::Budget,
        ] {
            assert!(!switches.allows(origin), "{origin:?}");
        }
        assert!(switches.allows(TurnOrigin::User));
        assert!(!switches.need_a_limit());
        assert!(!settings.verification_reminder);
        assert_eq!(settings.daily_turn_limit, None);
    }

    #[test]
    fn a_refusal_of_a_later_daemon_reads_as_other() {
        let read: TurnRefusal = serde_json::from_str(r#""quota""#).unwrap();
        assert_eq!(read, TurnRefusal::Other);
        assert_eq!(
            serde_json::to_string(&TurnRefusal::SwitchOff).unwrap(),
            r#""switch_off""#
        );
    }
}
