use serde::{Deserialize, Serialize};

use crate::bot::{BotDraft, BotPatch, BotSummary, present};
use crate::event::{ApprovalDecision, Event, TurnUsage};
use crate::harness::{BotSkillFigures, Consumption, HarnessSettings, SkillFigures};
use crate::memory::{MemoryEntry, MemoryOperation};
use crate::preferences::{Preferences, PreferencesPatch};
use crate::provider::{DaemonInfo, ProviderModels, ProviderStatus};
use crate::rules::{ApprovalRule, RuleDraft};
use crate::search::SearchResults;
use crate::skill::{SkillFinding, SkillStateChange, SkillSummary, SkillVersion};

pub const PROTOCOL_VERSION: u32 = 15;

pub const MESSAGE_LENGTH: usize = 100_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol_version: u32,
    },
    ListBots,
    CreateBot {
        draft: BotDraft,
    },
    UpdateBot {
        bot_id: i64,
        patch: BotPatch,
    },
    /// Creates a bot with the settings of another one and no conversation of it.
    DuplicateBot {
        bot_id: i64,
    },
    /// Stops the bot's running turns, then removes it and its conversations.
    DeleteBot {
        bot_id: i64,
    },
    /// Asks for the conversations of a bot, the most recently active first.
    ListConversations {
        bot_id: i64,
    },
    /// Starts a conversation and makes it the bot's current one.
    NewConversation {
        bot_id: i64,
    },
    OpenConversation {
        conversation_id: i64,
    },
    CloseConversation {
        conversation_id: i64,
    },
    SendMessage {
        conversation_id: i64,
        text: String,
    },
    LoadOlderEvents {
        conversation_id: i64,
        before: i64,
    },
    SetDraft {
        conversation_id: i64,
        text: String,
    },
    ResolveApproval {
        conversation_id: i64,
        approval_id: String,
        decision: ApprovalDecision,
    },
    StopTurn {
        conversation_id: i64,
    },
    EditQueuedMessage {
        conversation_id: i64,
        queue_id: i64,
        text: String,
    },
    DeleteQueuedMessage {
        conversation_id: i64,
        queue_id: i64,
    },
    SendQueuedMessageNow {
        conversation_id: i64,
        queue_id: i64,
    },
    ResumeQueue {
        conversation_id: i64,
    },
    /// The client shows the conversation up to its end: all of it is read.
    MarkRead {
        conversation_id: i64,
        /// The client keeps the end on screen: what arrives next is read
        /// too, until it sends a mark that says otherwise or closes the
        /// conversation.
        #[serde(default)]
        following: bool,
    },
    MarkBotRead {
        bot_id: i64,
    },
    /// Moves the read mark back, before the last turn that ended.
    MarkBotUnread {
        bot_id: i64,
    },
    GetPreferences,
    SetPreferences {
        patch: PreferencesPatch,
    },
    DeleteHistory {
        bot_id: i64,
    },
    RevealEvent {
        conversation_id: i64,
        before: i64,
        event_id: i64,
    },
    /// Asks for the events after `after`, up to the event `until`, which the
    /// client already holds.
    LoadNewerEvents {
        conversation_id: i64,
        after: i64,
        until: i64,
    },
    ListProviders,
    CheckProvider {
        provider: String,
    },
    /// Starts a sign-in of the provider. `method` names one of the methods
    /// of its status; with none, the daemon starts the first one.
    SignIn {
        provider: String,
        #[serde(default)]
        method: Option<String>,
    },
    CancelSignIn {
        provider: String,
    },
    /// Downloads the runtime of the provider now, instead of at the first turn.
    InstallRuntime {
        provider: String,
    },
    /// Sets the executable the daemon starts for the provider, in place of
    /// the runtime it installs. `None` goes back to that runtime.
    SetProviderExecutable {
        provider: String,
        path: Option<String>,
    },
    /// Asks for the models and the reasoning levels the provider offers.
    ListModels {
        provider: String,
    },
    GetDaemonInfo,
    /// Stops every new turn the daemon would start on its own; the turns
    /// that run go on. Every attached client receives the daemon's info.
    PauseDaemon,
    /// Lets the daemon start its own turns again.
    ResumeDaemon,
    Search {
        request: u64,
        query: String,
    },
    /// Asks for the approval rules of a bot.
    ListApprovalRules {
        bot_id: i64,
    },
    /// Gives a bot a rule. This message is the only way a rule is made: no
    /// daemon tool, prompt, or file makes one.
    AddApprovalRule {
        bot_id: i64,
        rule: RuleDraft,
    },
    DeleteApprovalRule {
        rule_id: i64,
    },
    /// Asks for the active memory of a bot: its own entries and the user's.
    ListMemory {
        bot_id: i64,
    },
    /// Adds, corrects, or retires memory entries as a person, all or none.
    /// The change is recorded in the bot's current conversation.
    ChangeMemory {
        bot_id: i64,
        operations: Vec<MemoryOperation>,
    },
    /// Asks for the skills of a bot, active and archived.
    ListSkills {
        bot_id: i64,
    },
    /// Asks for the versions a skill of a bot had, the oldest first.
    ListSkillHistory {
        bot_id: i64,
        name: String,
    },
    /// Makes an earlier version of a skill the active one, as a person. The
    /// change is recorded in the bot's current conversation.
    RollBackSkill {
        bot_id: i64,
        name: String,
        hash: String,
    },
    /// Gives a bot the skill a folder of this computer holds, as a person. A
    /// skill the scan finds dangerous is imported only when `confirm` is set.
    ImportSkill {
        bot_id: i64,
        folder: String,
        #[serde(default)]
        confirm: bool,
    },
    /// Pins, unpins, archives, or restores a skill, as a person.
    SetSkillState {
        bot_id: i64,
        name: String,
        change: SkillStateChange,
    },
    /// Asks for what the daemon may start on its own for a bot.
    GetHarnessSettings {
        bot_id: i64,
    },
    /// Sets what the daemon may start on its own for a bot, and how much of
    /// it a day. This message is the only way they change: no daemon tool,
    /// prompt, or file changes them. A switch other than `retry` is refused
    /// while no daily limit of turns is set.
    SetHarnessSettings {
        bot_id: i64,
        settings: HarnessSettings,
    },
    /// Asks what a bot's turns consumed during the day, per origin.
    GetConsumption {
        bot_id: i64,
    },
    /// Asks how the turns that loaded a skill went, per version.
    GetSkillFigures {
        bot_id: i64,
        name: String,
    },
    /// Asks how often a bot loads the skills its index lists.
    GetBotSkillFigures {
        bot_id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerMessage {
    Welcome {
        protocol_version: u32,
        daemon_version: String,
    },
    Bots(Vec<BotSummary>),
    /// A bot was created or changed. Every attached client receives it.
    BotChanged(BotSummary),
    /// Answers the client that created or duplicated a bot, after `BotChanged`.
    BotCreated {
        bot_id: i64,
    },
    /// A bot and its conversations are gone. Every attached client receives it.
    BotDeleted {
        bot_id: i64,
    },
    Conversations {
        bot_id: i64,
        conversations: Vec<ConversationSummary>,
    },
    /// Answers the client that started a conversation.
    ConversationCreated(ConversationSummary),
    /// A conversation was created or got its title. Every attached client receives it.
    ConversationChanged(ConversationSummary),
    ConversationOpened(ConversationSnapshot),
    ConversationEvent {
        conversation_id: i64,
        event: Event,
        #[serde(default)]
        at_ms: i64,
    },
    OlderEvents {
        conversation_id: i64,
        events: Vec<TimedEvent>,
        older: Option<i64>,
    },
    OlderEventsFailed {
        conversation_id: i64,
        message: String,
    },
    /// Answers `RevealEvent` when too many events lie between the event and
    /// the ones the client holds: a part of the history around the event.
    /// `newer` names its last event when later ones exist before the client's.
    EventsWindow {
        conversation_id: i64,
        events: Vec<TimedEvent>,
        older: Option<i64>,
        newer: Option<i64>,
    },
    NewerEvents {
        conversation_id: i64,
        events: Vec<TimedEvent>,
        newer: Option<i64>,
    },
    NewerEventsFailed {
        conversation_id: i64,
        message: String,
    },
    MessageAccepted {
        conversation_id: i64,
    },
    MessageRefused {
        conversation_id: i64,
        message: String,
    },
    ApprovalNotResolved {
        conversation_id: i64,
        approval_id: String,
        message: String,
    },
    QueueChanged {
        conversation_id: i64,
        queue: QueueState,
    },
    Error {
        message: String,
    },
    /// A request was refused because of one of its fields, and nothing was stored.
    InvalidField {
        field: String,
        message: String,
    },
    Preferences(Preferences),
    Incompatible {
        protocol_version: u32,
        daemon_version: String,
    },
    /// The daemon was told to stop and closes every connection next. A client
    /// shows it as stopped and does not start it again on its own.
    Stopping,
    TurnModel {
        conversation_id: i64,
        model: Option<String>,
    },
    BotUpdated {
        bot_id: i64,
    },
    HistoryDeleted {
        bot_id: i64,
        current_conversation_id: i64,
    },
    Providers(Vec<ProviderStatus>),
    ProviderChanged(ProviderStatus),
    /// Answers `ListModels`, and follows every change of what a provider offers.
    ProviderModels(ProviderModels),
    DaemonInfo(DaemonInfo),
    SearchResults(SearchResults),
    /// The approval rules of a bot, the oldest first. Answers
    /// `ListApprovalRules`, and every attached client receives it after a
    /// rule of the bot is added or deleted.
    ApprovalRules {
        bot_id: i64,
        rules: Vec<ApprovalRule>,
    },
    /// Answers the client that added a rule, after `ApprovalRules`.
    ApprovalRuleAdded {
        bot_id: i64,
        rule_id: i64,
    },
    /// The active memory of a bot, the oldest first. Answers `ListMemory`,
    /// and every attached client receives it after the memory changes.
    Memory {
        bot_id: i64,
        entries: Vec<MemoryEntry>,
    },
    /// The skills of a bot, by name. Answers `ListSkills`, and every
    /// attached client receives it after the skills change.
    Skills {
        bot_id: i64,
        skills: Vec<SkillSummary>,
    },
    /// Answers `ListSkillHistory`.
    SkillHistory {
        bot_id: i64,
        name: String,
        versions: Vec<SkillVersion>,
    },
    /// Answers an `ImportSkill` that was not applied: why, and what the scan
    /// found when that is the reason.
    SkillNotImported {
        bot_id: i64,
        folder: String,
        message: String,
        findings: Vec<SkillFinding>,
    },
    /// What the daemon may start on its own for a bot. Answers
    /// `GetHarnessSettings`, and every attached client receives it after
    /// the settings change.
    HarnessSettings {
        bot_id: i64,
        settings: HarnessSettings,
    },
    /// Answers `GetConsumption`.
    Consumption(Consumption),
    /// Answers `GetSkillFigures`.
    SkillFigures(SkillFigures),
    /// Answers `GetBotSkillFigures`.
    BotSkillFigures(BotSkillFigures),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: i64,
    pub bot_id: i64,
    pub title: Option<String>,
    pub created_at_ms: i64,
    pub last_activity_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedEvent {
    #[serde(default)]
    pub id: i64,
    pub at_ms: i64,
    pub event: Event,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationSnapshot {
    pub conversation: ConversationSummary,
    pub events: Vec<TimedEvent>,
    #[serde(default)]
    pub older: Option<i64>,
    #[serde(default)]
    pub draft: String,
    pub turn_active: bool,
    pub pending_approvals: Vec<String>,
    #[serde(default)]
    pub queue: QueueState,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub turn_model: Option<Option<String>>,
    /// What the agent reported the last turn that ended consumed. `None`
    /// when it reported nothing, or when no turn ended yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TurnUsage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueState {
    pub messages: Vec<QueuedMessage>,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub id: i64,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(turn_model: Option<Option<String>>) -> ConversationSnapshot {
        ConversationSnapshot {
            conversation: ConversationSummary {
                id: 1,
                bot_id: 1,
                title: None,
                created_at_ms: 0,
                last_activity_ms: 0,
            },
            events: Vec::new(),
            older: None,
            draft: String::new(),
            turn_active: turn_model.is_some(),
            pending_approvals: Vec::new(),
            queue: QueueState::default(),
            turn_model,
            usage: None,
        }
    }

    #[test]
    fn a_snapshot_tells_the_default_model_of_a_turn_from_no_turn() {
        for turn_model in [None, Some(None), Some(Some("opus".to_owned()))] {
            let sent = snapshot(turn_model);
            let wire = serde_json::to_string(&sent).unwrap();
            assert_eq!(
                serde_json::from_str::<ConversationSnapshot>(&wire).unwrap(),
                sent
            );
        }
    }

    #[test]
    fn a_snapshot_from_an_older_daemon_has_no_turn_model() {
        let wire = r#"{"conversation":{"id":1,"bot_id":1,"title":null,"created_at_ms":0,
            "last_activity_ms":0},"events":[],"turn_active":true,"pending_approvals":[]}"#;
        let read: ConversationSnapshot = serde_json::from_str(wire).unwrap();
        assert_eq!(read.turn_model, None);
        assert!(read.turn_active);
        assert_eq!(read.older, None);
        assert_eq!(read.draft, "");
    }

    #[test]
    fn a_sign_in_from_an_older_client_names_no_method() {
        let read: ClientMessage =
            serde_json::from_str(r#"{"type":"sign_in","data":{"provider":"claude"}}"#).unwrap();
        assert_eq!(
            read,
            ClientMessage::SignIn {
                provider: "claude".to_owned(),
                method: None,
            }
        );
    }

    #[test]
    fn a_page_of_events_carries_the_time_of_each_and_where_older_ones_start() {
        let sent = ServerMessage::OlderEvents {
            conversation_id: 3,
            events: vec![TimedEvent {
                id: 40,
                at_ms: 1_700_000_000_000,
                event: Event::user_text("hi".to_owned()),
            }],
            older: Some(41),
        };
        let wire = serde_json::to_string(&sent).unwrap();
        assert_eq!(
            wire,
            r#"{"type":"older_events","data":{"conversation_id":3,"events":[{"id":40,"at_ms":1700000000000,"event":{"type":"user_text","data":"hi"}}],"older":41}}"#
        );
        assert_eq!(serde_json::from_str::<ServerMessage>(&wire).unwrap(), sent);
    }
}
