//! The curator (US-028): a pass over the skills that bots and reviews
//! wrote, which marks stale the ones nobody used for long and archives the
//! ones nobody used for longer, so that each bot's index stays short. It
//! calls no model, deletes no version, and leaves alone a skill that is
//! pinned, that a person imported, or that a routine names; an archived
//! skill comes back as it was when a person restores it.

mod limits;

pub use limits::{CURATOR_ARCHIVE, CURATOR_IDLE, CURATOR_INTERVAL, CURATOR_STALE};

use std::collections::BTreeMap;
use std::time::Duration;

use crowbot_protocol::SkillState;

use crate::store::{Expected, SkillPlan, Store, StoreError};

/// The name of the curator's mark: the time of its last pass.
pub const MARK: &str = "curator";

fn millis(wait: Duration) -> i64 {
    i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)
}

/// When a pass is due at `now`: `CURATOR_INTERVAL` after the last one, and
/// no turn for `CURATOR_IDLE`. A daemon that never ran one counts from now.
pub fn due(store: &Store, now: i64, working: bool) -> Result<bool, StoreError> {
    let Some(last) = store.mark(MARK)? else {
        store.set_mark(MARK, now)?;
        return Ok(false);
    };
    if working || now - last < millis(CURATOR_INTERVAL) {
        return Ok(false);
    }
    Ok(store
        .last_turn_ms()?
        .is_none_or(|last_turn| now - last_turn >= millis(CURATOR_IDLE)))
}

/// What a pass changes, per bot: a skill unused for `CURATOR_ARCHIVE` is
/// archived, one unused for `CURATOR_STALE` is marked stale.
pub fn plans(store: &Store, now: i64) -> Result<BTreeMap<i64, Vec<SkillPlan>>, StoreError> {
    let mut plans: BTreeMap<i64, Vec<SkillPlan>> = BTreeMap::new();
    let mut spared: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for candidate in store.curator_candidates()? {
        let names = match spared.get(&candidate.bot_id) {
            Some(names) => names,
            None => spared
                .entry(candidate.bot_id)
                .or_insert(store.routine_skill_names(candidate.bot_id)?),
        };
        if names.contains(&candidate.name) {
            continue;
        }
        let unused = now - candidate.last_active_ms;
        let mut plan = SkillPlan::new(&candidate.name, Expected::Any);
        if unused >= millis(CURATOR_ARCHIVE) {
            plan.state = Some(SkillState::Archived);
        } else if unused >= millis(CURATOR_STALE) && !candidate.stale {
            plan.stale = true;
        } else {
            continue;
        }
        plans.entry(candidate.bot_id).or_default().push(plan);
    }
    Ok(plans)
}
