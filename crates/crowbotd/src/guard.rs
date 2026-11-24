//! What the daemon watches in a turn, from the events it records, to end a
//! turn that went wrong: tool calls that repeat with the same results, a turn
//! of its own that went silent, a turn that asks for more tool calls than its
//! bot allows, and the retries of a turn that failed before it said anything.
//! The agent owns its loop; this watches it from outside (`AGENTS.md`, "The
//! harness").

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::time::Duration;

use crowbot_protocol::{Event, ToolStatus};

mod limits;

pub use limits::{INACTIVITY_LIMIT, NO_PROGRESS_CYCLE, NO_PROGRESS_REPEATS, RETRY_DELAYS};

/// The message of origin `Budget` the daemon sends after a turn that used
/// the bot's tool calls, when the person allowed it: a summary rather than
/// more work (`hermes-agent/agent/turn_finalizer.py:138-164`).
pub const BUDGET_SUMMARY: &str = "You reached the limit of tool calls for one turn. Call no tool now. Summarize the work done so far, what is left to do, and what you would do next.";

/// Divides the waits above in the daemon's own tests, in debug builds only,
/// so that a test reads the same constants and does not wait ten minutes.
#[cfg(debug_assertions)]
const TIME_SCALE: &str = "CROWBOT_TEST_TIME_SCALE";

fn time_scale() -> u32 {
    static SCALE: OnceLock<u32> = OnceLock::new();
    *SCALE.get_or_init(|| {
        #[cfg(debug_assertions)]
        if let Some(scale) = std::env::var(TIME_SCALE)
            .ok()
            .and_then(|scale| scale.parse::<u32>().ok())
            .filter(|scale| *scale > 0)
        {
            return scale;
        }
        1
    })
}

/// A wait of this module, as the daemon applies it.
pub fn scaled(wait: Duration) -> Duration {
    wait / time_scale()
}

/// A tool call of the running turn, as its events describe it so far.
#[derive(Default)]
struct Call {
    title: String,
    input: Option<String>,
    output: Option<String>,
    finished: bool,
}

impl Call {
    /// What makes two calls the same: their title, their input, and their
    /// output. An agent that sends no output compares on the first two
    /// (`docs/harness-providers.md`, A6).
    fn signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        (&self.title, &self.input, &self.output).hash(&mut hasher);
        hasher.finish()
    }
}

/// What one event tells about the turn's tool calls.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// The event started a tool call the turn had not made yet.
    pub new_call: bool,
    /// The turn repeats the same calls with the same results.
    pub no_progress: bool,
}

/// The tool calls of one turn.
#[derive(Default)]
pub struct Progress {
    calls: HashMap<String, Call>,
    /// The signatures of the calls that ended, in the order they ended: just
    /// enough of them to hold the laps of the longest cycle.
    ended: Vec<u64>,
}

impl Progress {
    /// The tool calls the turn started.
    pub fn calls(&self) -> usize {
        self.calls.len()
    }

    pub fn observe(&mut self, event: &Event) -> Observation {
        let mut seen = Observation::default();
        match event {
            Event::ToolCallStarted {
                id,
                title,
                status,
                input,
                ..
            } => {
                seen.new_call = !self.calls.contains_key(id);
                let call = self.calls.entry(id.clone()).or_default();
                call.title.clone_from(title);
                if input.is_some() {
                    call.input.clone_from(input);
                }
                seen.no_progress = self.end_when_final(id, *status);
            }
            Event::ToolCallUpdated {
                id,
                title,
                status,
                input,
                output,
                ..
            } => {
                let call = self.calls.entry(id.clone()).or_default();
                if let Some(title) = title {
                    call.title.clone_from(title);
                }
                if input.is_some() {
                    call.input.clone_from(input);
                }
                if output.is_some() {
                    call.output.clone_from(output);
                }
                seen.no_progress = self.end_when_final(id, *status);
            }
            _ => {}
        }
        seen
    }

    /// A call that completed or failed is compared with the ones that ended
    /// before it, once.
    fn end_when_final(&mut self, id: &str, status: Option<ToolStatus>) -> bool {
        if !matches!(status, Some(ToolStatus::Completed | ToolStatus::Failed)) {
            return false;
        }
        let Some(call) = self.calls.get_mut(id).filter(|call| !call.finished) else {
            return false;
        };
        call.finished = true;
        self.ended.push(call.signature());
        let kept = NO_PROGRESS_REPEATS * NO_PROGRESS_CYCLE;
        if self.ended.len() > kept {
            self.ended.drain(..self.ended.len() - kept);
        }
        self.repeats()
    }

    /// The last calls that ended repeat one call, or one cycle of up to
    /// `NO_PROGRESS_CYCLE` calls, `NO_PROGRESS_REPEATS` times.
    fn repeats(&self) -> bool {
        (1..=NO_PROGRESS_CYCLE).any(|period| {
            let span = period * NO_PROGRESS_REPEATS;
            let Some(start) = self.ended.len().checked_sub(span) else {
                return false;
            };
            let laps = &self.ended[start..];
            laps.iter()
                .skip(period)
                .zip(laps)
                .all(|(later, earlier)| later == earlier)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: &str, title: &str, input: &str) -> Event {
        Event::ToolCallStarted {
            id: id.to_owned(),
            title: title.to_owned(),
            status: Some(ToolStatus::InProgress),
            kind: None,
            input: Some(input.to_owned()),
            effect: None,
        }
    }

    fn ended(id: &str, output: Option<&str>) -> Event {
        Event::ToolCallUpdated {
            id: id.to_owned(),
            title: None,
            status: Some(ToolStatus::Completed),
            kind: None,
            input: None,
            output: output.map(str::to_owned),
        }
    }

    /// Runs the calls, each `(title, input, output)`, and returns the number
    /// of the call after which the guard saw no progress.
    fn stopped_after(calls: &[(&str, &str, Option<&str>)]) -> Option<usize> {
        let mut progress = Progress::default();
        for (index, (title, input, output)) in calls.iter().enumerate() {
            let id = format!("c{index}");
            assert!(progress.observe(&started(&id, title, input)).new_call);
            if progress.observe(&ended(&id, *output)).no_progress {
                return Some(index + 1);
            }
        }
        None
    }

    #[test]
    fn the_same_call_with_the_same_output_stops_at_the_fifth() {
        let call = ("Read", "notes.txt", Some("one"));
        assert_eq!(stopped_after(&[call; 8]), Some(NO_PROGRESS_REPEATS));
        assert_eq!(stopped_after(&[call; 4]), None);
    }

    #[test]
    fn the_same_call_with_other_outputs_is_progress() {
        let calls: Vec<_> = ["1", "2", "3", "4", "5", "6"]
            .iter()
            .map(|output| ("Run tests", "cargo test", Some(*output)))
            .collect();
        assert_eq!(stopped_after(&calls), None);
    }

    #[test]
    fn calls_with_no_output_compare_on_title_and_input() {
        let call = ("Run", "ls", None);
        assert_eq!(stopped_after(&[call; 5]), Some(5));
    }

    #[test]
    fn a_cycle_of_two_to_four_calls_stops_at_its_fifth_lap() {
        for period in 2..=NO_PROGRESS_CYCLE {
            let lap: Vec<_> = (0..period)
                .map(|step| ("Read", ["a", "b", "c", "d"][step], Some("same")))
                .collect();
            let calls: Vec<_> = lap.iter().copied().cycle().take(period * 7).collect();
            assert_eq!(
                stopped_after(&calls),
                Some(period * NO_PROGRESS_REPEATS),
                "period {period}"
            );
        }
    }

    #[test]
    fn a_cycle_whose_outputs_change_is_progress() {
        let calls: Vec<_> = (0..20)
            .map(|step| {
                let input = if step % 2 == 0 { "a" } else { "b" };
                let output = ["x", "y", "z"][step % 3];
                ("Read", input, Some(output))
            })
            .collect();
        assert_eq!(stopped_after(&calls), None);
    }

    #[test]
    fn a_call_counts_once_however_many_updates_it_gets() {
        let mut progress = Progress::default();
        assert!(progress.observe(&started("c1", "Read", "a")).new_call);
        assert!(!progress.observe(&started("c1", "Read", "a")).new_call);
        for _ in 0..6 {
            assert!(!progress.observe(&ended("c1", Some("x"))).no_progress);
        }
        assert_eq!(progress.calls(), 1);
    }

    #[test]
    fn a_wait_is_divided_by_no_scale_unless_a_test_sets_one() {
        if std::env::var_os(TIME_SCALE).is_none() {
            assert_eq!(scaled(INACTIVITY_LIMIT), INACTIVITY_LIMIT);
        }
    }
}
