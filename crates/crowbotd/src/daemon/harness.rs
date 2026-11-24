//! What the daemon starts on its own for a bot: the settings a person gives
//! it, the check every daemon-started turn passes before it starts, what the
//! turns of each origin consumed, and the reviews of the bot's work.

use std::sync::{Arc, MutexGuard, PoisonError};

use crowbot_protocol::{
    DAILY_TOKEN_LIMIT_MAX, DAILY_TURN_LIMIT_MAX, HarnessSettings, MemoryChange, ServerMessage,
    SkillChange, TurnOrigin, TurnRefusal,
};

use super::{Daemon, Refusal, ReviewSlot, invalid, missing_bot};
use crate::conversation::Command;
use crate::review::{self, Focus};
use crate::store::StoreError;

impl Daemon {
    pub fn harness_settings(&self, bot_id: i64) -> Result<ServerMessage, Refusal> {
        let settings = self.store.with(|store| {
            store
                .bot(bot_id)?
                .map(|_| store.harness_settings(bot_id))
                .transpose()
        })?;
        let settings = settings.ok_or_else(|| Refusal::Other(missing_bot(bot_id)))?;
        Ok(ServerMessage::HarnessSettings { bot_id, settings })
    }

    /// Stores what a person allows the daemon to start for a bot, and tells
    /// every attached client. A switch other than `retry` needs a daily
    /// limit of turns: no default limit is invented.
    pub fn set_harness_settings(
        &self,
        bot_id: i64,
        settings: HarnessSettings,
    ) -> Result<(), Refusal> {
        if settings
            .daily_turn_limit
            .is_some_and(|limit| limit == 0 || limit > DAILY_TURN_LIMIT_MAX)
        {
            return Err(invalid(
                "daily_turn_limit",
                format!("A daily limit of turns is between 1 and {DAILY_TURN_LIMIT_MAX}."),
            ));
        }
        if settings
            .daily_token_limit
            .is_some_and(|limit| limit == 0 || limit > DAILY_TOKEN_LIMIT_MAX)
        {
            return Err(invalid(
                "daily_token_limit",
                format!("A daily limit of tokens is between 1 and {DAILY_TOKEN_LIMIT_MAX}."),
            ));
        }
        if settings.switches.need_a_limit() && settings.daily_turn_limit.is_none() {
            return Err(invalid(
                "daily_turn_limit",
                "Set how many turns Crow Bot may start on its own each day before you turn this on.",
            ));
        }
        if !self
            .store
            .with(|store| store.set_harness_settings(bot_id, &settings))?
        {
            return Err(Refusal::Other(missing_bot(bot_id)));
        }
        self.state()
            .broadcast(&ServerMessage::HarnessSettings { bot_id, settings });
        Ok(())
    }

    pub fn consumption(&self, bot_id: i64) -> Result<ServerMessage, Refusal> {
        let consumption = self.store.with(|store| {
            store
                .bot(bot_id)?
                .map(|_| store.consumption(bot_id))
                .transpose()
        })?;
        consumption
            .map(ServerMessage::Consumption)
            .ok_or_else(|| Refusal::Other(missing_bot(bot_id)))
    }

    pub fn skill_figures(&self, bot_id: i64, name: &str) -> Result<ServerMessage, Refusal> {
        self.store
            .with(|store| store.skill_figures(bot_id, name))?
            .map(ServerMessage::SkillFigures)
            .ok_or_else(|| Refusal::Other(format!("bot {bot_id} has no skill named {name}")))
    }

    pub fn bot_skill_figures(&self, bot_id: i64) -> Result<ServerMessage, Refusal> {
        let figures = self.store.with(|store| {
            store
                .bot(bot_id)?
                .map(|_| store.bot_skill_figures(bot_id))
                .transpose()
        })?;
        figures
            .map(ServerMessage::BotSkillFigures)
            .ok_or_else(|| Refusal::Other(missing_bot(bot_id)))
    }

    /// Whether a turn of `origin` may start for a bot now: a person's always
    /// may; the daemon's needs the daemon not paused, the switch of its
    /// origin, and room under the bot's daily limits. Settings that cannot be
    /// read start nothing.
    pub fn admit(&self, bot_id: i64, origin: TurnOrigin) -> Result<(), TurnRefusal> {
        if origin == TurnOrigin::User {
            return Ok(());
        }
        if self.paused() {
            return Err(TurnRefusal::Paused);
        }
        let (settings, day) = self
            .store
            .with(|store| {
                Ok::<_, StoreError>((store.harness_settings(bot_id)?, store.daemon_day(bot_id)?))
            })
            .map_err(|error| {
                eprintln!("[crowbotd] bot {bot_id}: cannot read what it may start: {error}");
                TurnRefusal::Other
            })?;
        if !settings.switches.allows(origin) {
            return Err(TurnRefusal::SwitchOff);
        }
        if settings
            .daily_turn_limit
            .is_some_and(|limit| day.turns >= limit)
        {
            return Err(TurnRefusal::TurnLimit);
        }
        if settings
            .daily_token_limit
            .is_some_and(|limit| day.tokens >= limit)
        {
            return Err(TurnRefusal::TokenLimit);
        }
        Ok(())
    }

    /// Records a turn the daemon did not start, with its origin and cause.
    pub fn record_refusal(
        &self,
        bot_id: i64,
        conversation_id: Option<i64>,
        origin: TurnOrigin,
        cause: TurnRefusal,
        once_a_day: bool,
    ) {
        let recorded = self
            .store
            .with(|store| store.record_refusal(bot_id, conversation_id, origin, cause, once_a_day));
        match recorded {
            Ok(true) => eprintln!(
                "[crowbotd] bot {bot_id}: a turn of origin {origin:?} did not start ({cause:?})"
            ),
            Ok(false) => {}
            Err(error) => eprintln!("[crowbotd] bot {bot_id}: cannot record a refusal: {error}"),
        }
    }

    /// Held while a daemon-started turn is checked (`admit`) and its start
    /// recorded: two such turns cannot both take the last place of a day.
    pub fn admission(&self) -> MutexGuard<'_, ()> {
        self.admission
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// A turn of `conversation_id` ended: when no turn of the bot runs or
    /// waits in a queue and enough work ran since the last review, a review
    /// of that conversation starts, if the bot's switch and limits allow it.
    pub fn consider_review(self: &Arc<Self>, bot_id: i64, conversation_id: i64) {
        {
            let mut state = self.state();
            if state.stopping
                || state.deleting.contains(&bot_id)
                || state.reviews.contains_key(&bot_id)
                || state
                    .activity
                    .values()
                    .any(|activity| activity.bot_id == bot_id && activity.working)
            {
                return;
            }
            // Reserved in the section that found no turn of the bot working:
            // another turn that ends sees it, and so does a person's turn
            // that starts (`person_wrote`).
            state
                .reviews
                .insert(bot_id, ReviewSlot::Starting { stop: false });
        }
        if !self.start_review(bot_id, conversation_id) {
            let mut state = self.state();
            if matches!(
                state.reviews.get(&bot_id),
                Some(ReviewSlot::Starting { .. })
            ) {
                state.reviews.remove(&bot_id);
            }
        }
    }

    /// Starts the review reserved for a bot, when one is due. `false` when
    /// none started, and the reservation is then still the caller's.
    fn start_review(self: &Arc<Self>, bot_id: i64, conversation_id: i64) -> bool {
        let read = self.store.with(|store| {
            Ok::<_, StoreError>((
                store.review_due(bot_id)?,
                store.bot_has_queued(bot_id)?,
                store.summary(conversation_id)?,
            ))
        });
        let (due, queued, summary) = match read {
            Ok(read) => read,
            Err(error) => {
                eprintln!("[crowbotd] bot {bot_id}: cannot tell whether a review is due: {error}");
                return false;
            }
        };
        let Some(focus) = Focus::due(due.tool_calls, due.user_turns) else {
            return false;
        };
        if queued || self.review_stopped(bot_id) {
            return false;
        }
        if let Err(cause) = self.admit(bot_id, TurnOrigin::Review) {
            self.record_refusal(
                bot_id,
                Some(conversation_id),
                TurnOrigin::Review,
                cause,
                true,
            );
            return false;
        }
        let reviewed = summary
            .and_then(|summary| summary.title)
            .unwrap_or_else(|| "a conversation".to_owned());
        let title = format!("Review of {reviewed}");
        let created = self
            .store
            .with(|store| store.create_review(conversation_id, focus.skills, focus.memory, &title));
        let review_id = match created {
            Ok(Some(id)) => id,
            Ok(None) => return false,
            Err(error) => {
                eprintln!("[crowbotd] bot {bot_id}: cannot start a review: {error}");
                return false;
            }
        };
        self.publish_conversation(review_id);
        let text = match self.review_prompt(review_id, &reviewed, focus) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("[crowbotd] bot {bot_id}: cannot read the work to review: {error}");
                return false;
            }
        };
        let handle = match self.conversation(review_id) {
            Ok(handle) => handle,
            Err(message) => {
                eprintln!("[crowbotd] bot {bot_id}: cannot start a review: {message}");
                return false;
            }
        };
        let mut state = self.state();
        if state.reviews.get(&bot_id) != Some(&ReviewSlot::Starting { stop: false }) {
            // A person's turn started meanwhile: the review gives way.
            return false;
        }
        state.reviews.insert(bot_id, ReviewSlot::Running(review_id));
        // Sent in the section that marks the review running, so that a stop
        // a person's turn sends after it reaches the review after its start.
        handle.send(Command::Review { text });
        true
    }

    /// Whether a person's turn started since the review of a bot was
    /// reserved.
    fn review_stopped(&self, bot_id: i64) -> bool {
        self.state().reviews.get(&bot_id) == Some(&ReviewSlot::Starting { stop: true })
    }

    fn review_prompt(
        &self,
        review_id: i64,
        title: &str,
        focus: Focus,
    ) -> Result<String, StoreError> {
        self.store.with(|store| {
            let row = store
                .review(review_id)?
                .ok_or(StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))?;
            let reviewed = row.reviewed_conversation_id;
            let after = store.reviewed_until(reviewed, review_id)?;
            let events = store.work_events(
                reviewed,
                after,
                row.after_event_id,
                review::REVIEW_CHARACTERS,
            )?;
            Ok(review::prompt(focus, title, &events))
        })
    }

    /// A person's turn of the bot started: a review that runs stops, and
    /// one being created does not start. Called once the turn shows as
    /// working, so that a review reserved after it sees the turn instead.
    pub fn person_wrote(&self, bot_id: i64) {
        let mut guard = self.state();
        let state = &mut *guard;
        match state.reviews.get_mut(&bot_id) {
            Some(ReviewSlot::Starting { stop }) => *stop = true,
            Some(ReviewSlot::Running(review)) => {
                if let Some(handle) = state.conversations.get(review) {
                    handle.send(Command::Stop);
                }
            }
            None => {}
        }
    }

    /// The turn of a review ended. When it changed memory or a skill, the
    /// conversation it reviewed records what changed, with a link to it.
    pub fn review_ended(
        self: &Arc<Self>,
        bot_id: i64,
        review_id: i64,
        reviewed: i64,
        memory: Vec<MemoryChange>,
        skills: Vec<SkillChange>,
    ) {
        {
            let mut state = self.state();
            if state.reviews.get(&bot_id) == Some(&ReviewSlot::Running(review_id)) {
                state.reviews.remove(&bot_id);
            }
        }
        if memory.is_empty() && skills.is_empty() {
            return;
        }
        match self.conversation(reviewed) {
            Ok(handle) => handle.send(Command::RecordReview {
                review_conversation_id: review_id,
                memory,
                skills,
            }),
            Err(message) => {
                eprintln!("[crowbotd] review {review_id}: cannot tell its conversation: {message}");
            }
        }
    }
}
