//! The records of the bots' folders (EP-008): what a turn changed, a
//! restore a person asks for, and the sweep of what no record names.

use std::path::Path;
use std::sync::Arc;

use crowbot_protocol::ServerMessage;

use super::{Daemon, Refusal};
use crate::checkpoints::{Checkpoints, folder_key};
use crate::conversation::{Command, Sink};

impl Daemon {
    /// The records of the bots' folders.
    pub fn checkpoints(&self) -> &Arc<Checkpoints> {
        &self.checkpoints
    }

    /// Answers `GetTurnChanges`.
    pub fn turn_changes(
        &self,
        conversation_id: i64,
        event_id: i64,
    ) -> Result<ServerMessage, Refusal> {
        self.checkpoints
            .changes(&self.store, conversation_id, event_id)
            .map(ServerMessage::TurnChanges)
            .map_err(Refusal::Other)
    }

    /// Hands a restore to the conversation of the turn, which records it in
    /// its place among its events.
    pub fn restore_checkpoint(
        self: &Arc<Self>,
        conversation_id: i64,
        event_id: i64,
        paths: Option<Vec<String>>,
        confirm: bool,
        sink: Sink,
    ) -> Result<(), Refusal> {
        let handle = self.conversation(conversation_id).map_err(Refusal::Other)?;
        handle.send(Command::Restore {
            event_id,
            paths,
            confirm,
            sink,
        });
        Ok(())
    }

    /// Whether a turn runs, or waits on a person, in a conversation of a bot
    /// whose folder is `key`.
    pub fn folder_busy(&self, key: &Path) -> bool {
        let working: Vec<i64> = {
            let state = self.state();
            state
                .activity
                .values()
                .filter(|activity| activity.working)
                .map(|activity| activity.bot_id)
                .collect()
        };
        if working.is_empty() {
            return false;
        }
        match self.store.with(|store| store.bots()) {
            Ok(bots) => bots.iter().any(|row| {
                working.contains(&row.bot.id)
                    && folder_key(&row.bot.workspace).is_some_and(|folder| folder == key)
            }),
            // A store that cannot say is taken as busy: nothing is written.
            Err(_) => true,
        }
    }

    /// Removes, away from the runtime's threads, the trees and the contents
    /// that no record names after conversations were deleted.
    pub async fn sweep_checkpoints(&self) {
        let (store, checkpoints) = (self.store.clone(), self.checkpoints.clone());
        let swept = tokio::task::spawn_blocking(move || checkpoints.sweep(&store)).await;
        match swept {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => eprintln!("[crowbotd] cannot sweep the checkpoints: {error}"),
            Err(error) => eprintln!("[crowbotd] the sweep of the checkpoints failed: {error}"),
        }
    }
}
