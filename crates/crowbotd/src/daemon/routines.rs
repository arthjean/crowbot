//! The routines of the bots, as the daemon runs them: the scheduler's look
//! at the clock, the start of each run, what clients ask of routines, and
//! the curator's pass, which the same look starts.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crowbot_protocol::{
    Event, RoutineDraft, RoutinePause, RoutineState, RoutineStateChange, RunOutcome, ServerMessage,
    SkillAuthor, TurnOrigin, TurnRefusal,
};
use tokio::sync::Notify;

use super::{Daemon, Refusal, invalid, missing_bot};
use crate::clock;
use crate::conversation::Command;
use crate::curator;
use crate::routines::script::{self, ScriptEnd};
use crate::routines::{self, Draft};
use crate::store::{RoutineRow, RunEnd, StoreError};

impl Daemon {
    /// What wakes the scheduler before its next look at the clock.
    pub fn routines_wake(&self) -> &Notify {
        &self.routines_wake
    }

    /// A routine changed: the scheduler looks at it now.
    pub fn wake_routines(&self) {
        self.routines_wake.notify_one();
    }

    /// One look of the scheduler at the clock: each routine that came due
    /// fires, and the curator runs when its pass is due. Gives how long to
    /// sleep before the next look, or `None` once the daemon stops. A paused
    /// daemon fires nothing: when it resumes, the rule for missed
    /// occurrences decides what runs.
    pub fn routine_tick(self: &Arc<Self>) -> Option<Duration> {
        let (stopping, paused, working) = {
            let state = self.state();
            (
                state.stopping,
                state.paused,
                state.activity.values().any(|activity| activity.working),
            )
        };
        if stopping {
            return None;
        }
        let tick = clock::tick();
        let now = clock::now_ms();
        if !paused {
            match self.store.with(|store| store.due_routines(now)) {
                Ok(due) => {
                    for row in due {
                        self.fire_routine(&row, now);
                    }
                }
                Err(error) => {
                    eprintln!("[crowbotd] cannot read the routines that are due: {error}")
                }
            }
        }
        self.curate(now, working);
        let next = if paused {
            None
        } else {
            self.store.with(|store| store.next_due()).unwrap_or(None)
        };
        Some(next.map_or(tick, |next| {
            let wait = u64::try_from((next - now).max(10)).unwrap_or(10);
            Duration::from_millis(wait).min(tick)
        }))
    }

    fn fire_routine(self: &Arc<Self>, row: &RoutineRow, now: i64) {
        let Some(decision) = routines::decide(row, now) else {
            return;
        };
        let routine = &row.routine;
        let fired = match self.store.with(|store| store.fire(&decision.fire)) {
            Ok(Some(fired)) => fired,
            Ok(None) => return,
            Err(error) => {
                eprintln!(
                    "[crowbotd] routine {}: cannot record its run: {error}",
                    routine.id
                );
                return;
            }
        };
        if let Some((missed, caught_up)) = decision.missed {
            self.tell_routine(
                routine.conversation_id,
                Event::RoutineMissed {
                    routine_id: routine.id,
                    missed,
                    caught_up,
                },
            );
        }
        match fired {
            (run_id, RunOutcome::Queued) => self.start_run(run_id),
            (_, outcome) => eprintln!(
                "[crowbotd] routine {}: an occurrence did not start ({outcome:?})",
                routine.id
            ),
        }
        self.publish_routine(routine.id);
    }

    /// Records an event in the conversation of a routine, in its place.
    fn tell_routine(self: &Arc<Self>, conversation_id: i64, event: Event) {
        match self.conversation(conversation_id) {
            Ok(handle) => handle.send(Command::Notice { event }),
            Err(message) => {
                eprintln!(
                    "[crowbotd] conversation {conversation_id}: cannot record {event:?}: {message}"
                );
            }
        }
    }

    /// Starts a queued run: its guard script first, when it has one, then
    /// its turn in the routine's conversation.
    pub fn start_run(self: &Arc<Self>, run_id: i64) {
        let read = self.store.with(|store| {
            let Some(run) = store.run(run_id)? else {
                return Ok(None);
            };
            let routine = store.routine(run.routine_id)?;
            let bot = store.bot(run.bot_id)?;
            Ok::<_, StoreError>(routine.zip(bot).map(|(routine, bot)| (run, routine, bot)))
        });
        let (run, routine, bot) = match read {
            Ok(Some(read)) => read,
            Ok(None) => return,
            Err(error) => {
                eprintln!("[crowbotd] run {run_id}: cannot read it: {error}");
                return;
            }
        };
        if run.outcome != RunOutcome::Queued {
            return;
        }
        let routine = routine.routine;
        let Some(script) = routine.script.clone() else {
            let text = routines::run_text(&routine.name, &run.prompt, &routine.skills, None);
            return self.send_run(run_id, routine.conversation_id, text);
        };
        // A guard script is the daemon's own start of a program: it needs
        // what the run's turn needs, the bot's switch and room under its
        // limits. The turn is admitted again when it starts.
        if let Err(cause) = self.admit(run.bot_id, TurnOrigin::Routine) {
            self.record_refusal(
                run.bot_id,
                Some(routine.conversation_id),
                TurnOrigin::Routine,
                cause,
                false,
            );
            if cause != TurnRefusal::SwitchOff {
                self.tell_routine(
                    routine.conversation_id,
                    Event::TurnNotStarted {
                        origin: TurnOrigin::Routine,
                        cause,
                    },
                );
            }
            return self.end_run(
                run_id,
                routine.id,
                RunOutcome::NotStarted,
                &RunEnd {
                    refusal: Some(cause),
                    detail: None,
                },
            );
        }
        let daemon = self.clone();
        let folder = bot.bot.workspace;
        tokio::spawn(async move {
            let ended = script::run(&script, Path::new(&folder)).await;
            let end = |outcome: RunOutcome, detail: String| {
                daemon.end_run(
                    run_id,
                    routine.id,
                    outcome,
                    &RunEnd {
                        refusal: None,
                        detail: Some(detail),
                    },
                );
            };
            match ended {
                ScriptEnd::Changed => {
                    end(
                        RunOutcome::NotStarted,
                        "The guard script changed since a person approved it, so it did not run."
                            .to_owned(),
                    );
                    let paused = daemon.store.with(|store| {
                        store.set_routine_state(routine.id, RoutineState::Paused, routine.next_ms)
                    });
                    if let Err(error) = paused {
                        eprintln!(
                            "[crowbotd] routine {}: cannot pause it: {error}",
                            routine.id
                        );
                    }
                    daemon.tell_routine(
                        routine.conversation_id,
                        Event::RoutinePaused {
                            routine_id: routine.id,
                            cause: RoutinePause::ScriptChanged,
                        },
                    );
                    daemon.publish_routine(routine.id);
                }
                ScriptEnd::Silent => end(
                    RunOutcome::Silent,
                    "The guard script found nothing new.".to_owned(),
                ),
                ScriptEnd::Failed(message) => end(RunOutcome::ScriptFailed, message),
                ScriptEnd::Wake(output) => {
                    let findings = crate::scan::scan(&output);
                    if findings.is_empty() {
                        let text = routines::run_text(
                            &routine.name,
                            &run.prompt,
                            &routine.skills,
                            Some(&output),
                        );
                        daemon.send_run(run_id, routine.conversation_id, text);
                    } else {
                        end(RunOutcome::Blocked, crate::scan::describe(&findings));
                    }
                }
            }
        });
    }

    fn send_run(self: &Arc<Self>, run_id: i64, conversation_id: i64, text: String) {
        match self.conversation(conversation_id) {
            Ok(handle) => handle.send(Command::StartRun { run_id, text }),
            Err(message) => {
                let routine = self
                    .store
                    .with(|store| store.run(run_id))
                    .ok()
                    .flatten()
                    .map_or(0, |run| run.routine_id);
                self.end_run(
                    run_id,
                    routine,
                    RunOutcome::NotStarted,
                    &RunEnd {
                        refusal: None,
                        detail: Some(message),
                    },
                );
            }
        }
    }

    /// Records how a run ended and tells every attached client.
    pub fn end_run(&self, run_id: i64, routine_id: i64, outcome: RunOutcome, end: &RunEnd) {
        let ended = self
            .store
            .with(|store| store.end_run(run_id, outcome, end, clock::now_ms()));
        if let Err(error) = ended {
            eprintln!("[crowbotd] run {run_id}: cannot record its end: {error}");
        }
        self.publish_routine(routine_id);
    }

    pub fn list_routines(&self, bot_id: i64) -> Result<ServerMessage, Refusal> {
        let routines = self.store.with(|store| {
            store
                .bot(bot_id)?
                .map(|_| store.routines(bot_id))
                .transpose()
        })?;
        let routines = routines.ok_or_else(|| Refusal::Other(missing_bot(bot_id)))?;
        Ok(ServerMessage::Routines { bot_id, routines })
    }

    /// Tells every attached client where a routine stands.
    pub fn publish_routine(&self, routine_id: i64) {
        match self.store.with(|store| store.routine(routine_id)) {
            Ok(Some(row)) => self
                .state()
                .broadcast(&ServerMessage::RoutineChanged(row.routine)),
            Ok(None) => {}
            Err(error) => eprintln!("[crowbotd] routine {routine_id}: cannot read it: {error}"),
        }
    }

    /// After a routine was stored or changed through the `schedule` tool:
    /// every attached client receives the bot's routines, its settings, and
    /// the routine's conversation, and the scheduler looks at it.
    pub fn routines_changed(&self, bot_id: i64) {
        let read = self.store.with(|store| {
            Ok::<_, StoreError>((store.routines(bot_id)?, store.harness_settings(bot_id)?))
        });
        match read {
            Ok((routines, settings)) => {
                for routine in &routines {
                    self.publish_conversation(routine.conversation_id);
                }
                let mut state = self.state();
                state.broadcast(&ServerMessage::HarnessSettings { bot_id, settings });
                for routine in routines {
                    state.broadcast(&ServerMessage::RoutineChanged(routine));
                }
            }
            Err(error) => eprintln!("[crowbotd] bot {bot_id}: cannot read its routines: {error}"),
        }
        self.wake_routines();
    }

    /// Creates a routine as a person, in a conversation of its own, and turns
    /// the bot's `routine` switch on. Refused while the bot has no daily
    /// limit of turns: no default limit is invented.
    pub fn create_routine(&self, bot_id: i64, draft: RoutineDraft) -> Result<i64, Refusal> {
        let now = clock::now_ms();
        let created = self.store.with(|store| {
            let Some(bot) = store.bot(bot_id)? else {
                return Ok(Err(Refusal::Other(missing_bot(bot_id))));
            };
            let settings = store.harness_settings(bot_id)?;
            if settings.daily_turn_limit.is_none() {
                return Ok(Err(invalid(
                    "daily_turn_limit",
                    "Set how many turns Crow Bot may start on its own each day before you create a routine.",
                )));
            }
            let planned = match routines::plan(
                store,
                bot_id,
                Path::new(&bot.bot.workspace),
                Draft {
                    name: &draft.name,
                    schedule: draft.schedule.clone(),
                    prompt: &draft.prompt,
                    skills: &draft.skills,
                    skip_missed: draft.skip_missed,
                    script: draft.script.as_deref(),
                },
                now,
            ) {
                Ok(planned) => planned,
                Err((field, message)) => return Ok(Err(invalid(field, message))),
            };
            let id = store.create_routine(&planned.new, true)?;
            Ok::<_, StoreError>(id.ok_or_else(|| Refusal::Other(missing_bot(bot_id))))
        })?;
        let routine_id = created?;
        self.routines_changed(bot_id);
        Ok(routine_id)
    }

    /// Pauses or resumes a routine, as a person. A routine resumed is next
    /// due at its first occurrence after now, and turns the bot's `routine`
    /// switch on: it needs the daily limit of turns.
    pub fn set_routine_state(
        &self,
        routine_id: i64,
        change: RoutineStateChange,
    ) -> Result<(), Refusal> {
        let now = clock::now_ms();
        let changed = self.store.with(|store| {
            let Some(row) = store.routine(routine_id)? else {
                return Ok(Err(Refusal::Other(format!(
                    "routine {routine_id} does not exist"
                ))));
            };
            let routine = row.routine;
            match change {
                RoutineStateChange::Pause => {
                    if routine.state == RoutineState::Active {
                        store.set_routine_state(routine_id, RoutineState::Paused, routine.next_ms)?;
                    }
                }
                RoutineStateChange::Resume => {
                    if matches!(routine.state, RoutineState::Finished | RoutineState::Missed) {
                        return Ok(Err(Refusal::Other(
                            "This routine ran once, or missed its only occurrence: create another one."
                                .to_owned(),
                        )));
                    }
                    let settings = store.harness_settings(routine.bot_id)?;
                    if settings.daily_turn_limit.is_none() {
                        return Ok(Err(invalid(
                            "daily_turn_limit",
                            "Set how many turns Crow Bot may start on its own each day before you resume a routine.",
                        )));
                    }
                    if routine.state == RoutineState::Paused {
                        match routines::resumed_next(&routine.schedule, routine.next_ms, now) {
                            Some(next) => {
                                store.set_routine_state(routine_id, RoutineState::Active, Some(next))?;
                            }
                            None => {
                                store.set_routine_state(routine_id, RoutineState::Missed, None)?;
                            }
                        }
                    }
                    store.turn_routine_switch_on(routine.bot_id)?;
                }
            }
            Ok::<_, StoreError>(Ok(routine.bot_id))
        })?;
        let bot_id = changed?;
        self.routines_changed(bot_id);
        Ok(())
    }

    /// Removes a routine, as a person. Its conversation stays.
    pub fn delete_routine(self: &Arc<Self>, routine_id: i64) -> Result<(), Refusal> {
        let conversation = self
            .store
            .with(|store| store.routine(routine_id))?
            .map(|row| row.routine.conversation_id);
        let bot_id = self
            .store
            .with(|store| store.delete_routine(routine_id))?
            .ok_or_else(|| Refusal::Other(format!("routine {routine_id} does not exist")))?;
        if let Some(conversation) = conversation
            && let Ok(handle) = self.conversation(conversation)
        {
            handle.send(Command::RoutineDeleted);
        }
        self.state()
            .broadcast(&ServerMessage::RoutineDeleted { bot_id, routine_id });
        Ok(())
    }

    /// Runs a routine once now, as a person. Its next occurrence does not
    /// move.
    pub fn run_routine_now(self: &Arc<Self>, routine_id: i64) -> Result<(), Refusal> {
        let (run_id, outcome) = self
            .store
            .with(|store| store.run_now(routine_id, clock::now_ms()))?
            .ok_or_else(|| Refusal::Other(format!("routine {routine_id} does not exist")))?;
        if outcome == RunOutcome::Queued {
            self.start_run(run_id);
        }
        self.publish_routine(routine_id);
        if outcome == RunOutcome::Queued {
            Ok(())
        } else {
            Err(Refusal::Other(
                "The routine's last run has not ended, so this one did not start.".to_owned(),
            ))
        }
    }

    /// The curator's pass, when one is due: it changes skills through each
    /// bot's current conversation, which records what it did.
    fn curate(self: &Arc<Self>, now: i64, working: bool) {
        let planned = self.store.with(|store| {
            if !curator::due(store, now, working)? {
                return Ok(None);
            }
            let plans = curator::plans(store, now)?;
            store.set_mark(curator::MARK, now)?;
            Ok::<_, StoreError>(Some(plans))
        });
        let plans = match planned {
            Ok(Some(plans)) => plans,
            Ok(None) => return,
            Err(error) => {
                eprintln!("[crowbotd] the curator cannot run: {error}");
                return;
            }
        };
        eprintln!("[crowbotd] the curator ran over {} bot(s)", plans.len());
        for (bot_id, plans) in plans {
            let (sink, _) = tokio::sync::mpsc::unbounded_channel();
            if let Err(refusal) = self.change_skills_as(bot_id, plans, SkillAuthor::Curator, sink) {
                eprintln!(
                    "[crowbotd] bot {bot_id}: the curator cannot change its skills: {refusal:?}"
                );
            }
        }
    }
}
