//! What every connection and every conversation share: the bots, the attached
//! clients, the running conversations, and what each bot is doing.

mod providers;

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crowbot_protocol::{
    AVATAR_COLORS, Avatar, BOT_INSTRUCTIONS_LENGTH as INSTRUCTIONS_LENGTH,
    BOT_MODEL_LENGTH as MODEL_LENGTH, BOT_NAME_LENGTH, BOT_TITLE_LENGTH as TITLE_LENGTH, BotDraft,
    BotPatch, BotState, BotSummary, ConversationSummary, DaemonInfo, PROTOCOL_VERSION,
    PermissionLevel, Preferences, PreferencesPatch, SIDEBAR_WIDTH_LIMIT, ServerMessage, mascot_at,
};
use tokio::sync::oneshot;

use crate::conversation::{self, Command, ConversationHandle, Sink};
use crate::prompt::Sections;
use crate::runtime::Runtimes;
use crate::store::{BotRow, NewBot, SharedStore, StoreError};
use crate::tools::Registry;

/// The provider of a bot that runs the agent command given to the daemon.
pub const CUSTOM_PROVIDER: &str = "custom";

const SLUG_LENGTH: usize = 48;
/// How long a deleted bot's conversation gets to stop its turn. A turn ends
/// within the stop timeout of the conversation, which is shorter.
const SHUTDOWN_PATIENCE: Duration = Duration::from_secs(15);

/// Why the daemon did not do what a client asked.
#[derive(Debug)]
pub enum Refusal {
    /// One field of the request is not valid. Nothing was stored.
    Field {
        field: &'static str,
        message: String,
    },
    Other(String),
}

impl Refusal {
    pub fn into_message(self) -> ServerMessage {
        match self {
            Self::Field { field, message } => ServerMessage::InvalidField {
                field: field.to_owned(),
                message,
            },
            Self::Other(message) => ServerMessage::Error { message },
        }
    }
}

impl From<StoreError> for Refusal {
    fn from(error: StoreError) -> Self {
        Self::Other(error.to_string())
    }
}

fn invalid(field: &'static str, message: impl Into<String>) -> Refusal {
    Refusal::Field {
        field,
        message: message.into(),
    }
}

fn missing_bot(bot_id: i64) -> String {
    format!("bot {bot_id} does not exist")
}

/// Where a new bot works.
enum Folder {
    /// A folder the user chose.
    Chosen(String),
    /// A new folder of its own, under the daemon's folder for bots.
    Own,
}

/// What one conversation is doing right now. The stored events do not tell.
#[derive(Clone, Copy)]
struct Activity {
    bot_id: i64,
    working: bool,
    waiting: bool,
}

#[derive(Default)]
struct State {
    conversations: HashMap<i64, ConversationHandle>,
    clients: HashMap<u64, Sink>,
    activity: HashMap<i64, Activity>,
    /// The last summary sent for each bot: a summary that did not change is not sent again.
    published: HashMap<i64, BotSummary>,
    /// The bots whose conversations are being removed: none of them starts a task.
    deleting: HashSet<i64>,
    /// The daemon was told to stop: no conversation starts a task.
    stopping: bool,
    providers: HashMap<&'static str, providers::ProviderEntry>,
}

impl State {
    fn broadcast(&mut self, message: &ServerMessage) {
        self.clients
            .retain(|_, sink| sink.send(message.clone()).is_ok());
    }
}

pub struct Daemon {
    store: SharedStore,
    database: PathBuf,
    bots_dir: PathBuf,
    /// Where the daemon keeps what it installs and what providers store.
    data_dir: PathBuf,
    /// The agent command given to the daemon, which every bot then runs.
    command: Option<String>,
    runtimes: Runtimes,
    state: Mutex<State>,
    next_connection: AtomicU64,
    tools: Arc<Registry>,
    sections: Sections,
}

impl Daemon {
    pub fn new(
        store: SharedStore,
        database: PathBuf,
        bots_dir: PathBuf,
        data_dir: PathBuf,
        command: Option<String>,
        runtimes: Runtimes,
    ) -> Self {
        Self {
            store,
            database,
            bots_dir,
            data_dir,
            command,
            runtimes,
            state: Mutex::new(State::default()),
            next_connection: AtomicU64::new(1),
            tools: Arc::new(Registry::new()),
            sections: Sections::new(),
        }
    }

    /// The daemon's own tools, which every session's bridge serves.
    pub fn tools(&self) -> &Arc<Registry> {
        &self.tools
    }

    /// The sections the daemon adds to a session's system prompt.
    pub fn sections(&self) -> &Sections {
        &self.sections
    }

    pub fn store(&self) -> &SharedStore {
        &self.store
    }

    pub fn info(&self) -> ServerMessage {
        ServerMessage::DaemonInfo(DaemonInfo {
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol_version: PROTOCOL_VERSION,
            database: self.database.to_string_lossy().into_owned(),
            linger: crate::systemd::linger(),
        })
    }

    pub fn search(&self, request: u64, query: &str) -> Result<ServerMessage, String> {
        self.store
            .with(|store| store.search(request, query))
            .map(ServerMessage::SearchResults)
            .map_err(|error| error.to_string())
    }

    // The state is locked before the store, never the other way around.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn next_connection(&self) -> u64 {
        self.next_connection.fetch_add(1, Ordering::Relaxed)
    }

    /// From now on the client receives what changes for every bot.
    pub fn attach(&self, connection: u64, sink: Sink) {
        self.state().clients.insert(connection, sink);
    }

    pub fn detach(&self, connection: u64) {
        self.state().clients.remove(&connection);
    }

    pub fn conversation(self: &Arc<Self>, id: i64) -> Result<ConversationHandle, String> {
        let mut state = self.state();
        if state.stopping {
            return Err("The daemon is stopping.".to_owned());
        }
        if let Some(handle) = state.conversations.get(&id) {
            return Ok(handle.clone());
        }
        let conversation = self
            .store
            .with(|store| store.conversation(id))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("conversation {id} does not exist"))?;
        if state.deleting.contains(&conversation.bot_id) {
            return Err("This bot's conversations are being deleted.".to_owned());
        }
        let handle = conversation::spawn(self.clone(), &conversation);
        state.conversations.insert(id, handle.clone());
        Ok(handle)
    }

    fn checked_provider(&self, provider: Option<String>) -> Result<String, Refusal> {
        let offered = self.provider_ids();
        let Some(provider) = provider else {
            return Ok(offered[0].to_owned());
        };
        match self.known_provider(&provider) {
            Ok(id) => Ok(id.to_owned()),
            Err(message) => Err(invalid("provider", message)),
        }
    }

    fn summarize(&self, state: &State, row: BotRow) -> Result<BotSummary, StoreError> {
        let standing = self.store.with(|store| store.standing(row.bot.id))?;
        let (working, waiting) = state
            .activity
            .values()
            .filter(|activity| activity.bot_id == row.bot.id)
            .fold((false, false), |(working, waiting), activity| {
                (working || activity.working, waiting || activity.waiting)
            });
        Ok(BotSummary {
            state: BotState::of(waiting, standing.failed, working, standing.unread),
            unread: standing.unread,
            last_activity_ms: standing.last_activity_ms,
            preview: standing.preview,
            current_conversation_id: row.current_conversation_id,
            bot: row.bot,
        })
    }

    pub fn list_bots(&self) -> Result<ServerMessage, String> {
        let state = self.state();
        let bots = self
            .store
            .with(|store| store.bots())
            .and_then(|rows| {
                rows.into_iter()
                    .map(|row| self.summarize(&state, row))
                    .collect::<Result<Vec<_>, _>>()
            })
            .map_err(|error| error.to_string())?;
        Ok(ServerMessage::Bots(bots))
    }

    /// Sends the summary of a bot to every attached client, when it changed.
    pub fn publish_bot(&self, bot_id: i64) {
        let mut state = self.state();
        let summary = self
            .store
            .with(|store| store.bot(bot_id))
            .and_then(|row| row.map(|row| self.summarize(&state, row)).transpose());
        match summary {
            Ok(Some(summary)) => {
                if state.published.get(&bot_id) != Some(&summary) {
                    state.published.insert(bot_id, summary.clone());
                    state.broadcast(&ServerMessage::BotChanged(summary));
                }
            }
            Ok(None) => state
                .activity
                .retain(|_, activity| activity.bot_id != bot_id),
            Err(error) => eprintln!("[crowbotd] cannot summarize bot {bot_id}: {error}"),
        }
    }

    /// A conversation tells whether a turn runs and whether an approval waits.
    pub fn report_activity(&self, conversation_id: i64, bot_id: i64, working: bool, waiting: bool) {
        {
            let mut state = self.state();
            if working || waiting {
                state.activity.insert(
                    conversation_id,
                    Activity {
                        bot_id,
                        working,
                        waiting,
                    },
                );
            } else {
                state.activity.remove(&conversation_id);
            }
        }
        self.publish_bot(bot_id);
    }

    /// Sends the summary of a conversation to every attached client.
    pub fn publish_conversation(&self, conversation_id: i64) {
        match self.store.with(|store| store.summary(conversation_id)) {
            Ok(Some(summary)) => self
                .state()
                .broadcast(&ServerMessage::ConversationChanged(summary)),
            Ok(None) => {}
            Err(error) => {
                eprintln!("[crowbotd] cannot summarize conversation {conversation_id}: {error}");
            }
        }
    }

    pub fn create_bot(&self, draft: BotDraft) -> Result<i64, Refusal> {
        let name = checked_name(&draft.name)?;
        let avatar = match draft.avatar {
            Some(avatar) => checked_avatar(avatar)?,
            None => self.default_avatar()?,
        };
        let folder = match draft.workspace {
            Some(workspace) => Folder::Chosen(checked_workspace(&workspace)?),
            None => Folder::Own,
        };
        let new = NewBot {
            title: checked_text("title", draft.title.trim(), TITLE_LENGTH)?,
            instructions: checked_text("instructions", &draft.instructions, INSTRUCTIONS_LENGTH)?,
            avatar,
            provider: self.checked_provider(draft.provider)?,
            model: checked_choice("model", draft.model)?,
            effort: checked_choice("effort", draft.effort)?,
            permission_level: draft.permission_level.unwrap_or_default(),
            workspace: String::new(),
            name,
        };
        self.insert_bot(new, folder)
    }

    /// Creates a bot with the settings of another one. It gets a name and a
    /// current conversation of its own, and a folder of its own unless the
    /// source works in a folder the user chose.
    pub fn duplicate_bot(&self, bot_id: i64) -> Result<i64, Refusal> {
        let (source, bots) = self
            .store
            .with(|store| Ok::<_, StoreError>((store.bot(bot_id)?, store.bots()?)))?;
        let source = source
            .ok_or_else(|| Refusal::Other(missing_bot(bot_id)))?
            .bot;
        let names: Vec<String> = bots.into_iter().map(|row| row.bot.name).collect();
        let folder = if Path::new(&source.workspace).parent() == Some(self.bots_dir.as_path()) {
            Folder::Own
        } else {
            Folder::Chosen(source.workspace)
        };
        let new = NewBot {
            name: copy_name(&source.name, &names),
            title: source.title,
            instructions: source.instructions,
            avatar: source.avatar,
            provider: source.provider,
            model: source.model,
            effort: source.effort,
            permission_level: source.permission_level,
            workspace: String::new(),
        };
        self.insert_bot(new, folder)
    }

    fn insert_bot(&self, mut new: NewBot, folder: Folder) -> Result<i64, Refusal> {
        let own = matches!(folder, Folder::Own);
        new.workspace = match folder {
            Folder::Chosen(workspace) => workspace,
            Folder::Own => self.create_folder(&new.name)?,
        };
        match self.store.with(|store| store.create_bot(&new)) {
            Ok(row) => {
                let id = row.bot.id;
                self.publish_bot(id);
                Ok(id)
            }
            Err(error) => {
                if own {
                    // Only the empty folder made a moment ago can be removed this way.
                    std::fs::remove_dir(&new.workspace).ok();
                }
                Err(error.into())
            }
        }
    }

    /// Makes the folder of a new bot: the first free name that starts with
    /// the slug of the bot's name.
    fn create_folder(&self, name: &str) -> Result<String, Refusal> {
        let failed = |folder: &Path, error: std::io::Error| {
            invalid(
                "workspace",
                format!("Cannot create the folder {}: {error}", folder.display()),
            )
        };
        std::fs::create_dir_all(&self.bots_dir).map_err(|error| failed(&self.bots_dir, error))?;
        let slug = slug(name);
        for attempt in 1..=1000 {
            let folder = self.bots_dir.join(if attempt == 1 {
                slug.clone()
            } else {
                format!("{slug}-{attempt}")
            });
            match std::fs::create_dir(&folder) {
                Ok(()) => return Ok(folder.to_string_lossy().into_owned()),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(failed(&folder, error)),
            }
        }
        Err(invalid(
            "workspace",
            format!("No free folder name is left for {slug}."),
        ))
    }

    /// The avatar of a bot created with none: the next color and the next head
    /// after those of the bots that exist.
    fn default_avatar(&self) -> Result<Avatar, Refusal> {
        let bots = self.store.with(|store| store.bots())?.len();
        Ok(Avatar {
            color: AVATAR_COLORS[bots % AVATAR_COLORS.len()].to_owned(),
            shape: mascot_at(bots as u64),
        })
    }

    pub fn update_bot(&self, bot_id: i64, patch: BotPatch) -> Result<(), Refusal> {
        let mut patch = checked_patch(patch)?;
        patch.provider = patch
            .provider
            .map(|provider| self.checked_provider(Some(provider)))
            .transpose()?;
        let (before, after) = self.store.with(|store| {
            let before = store.bot(bot_id)?;
            Ok::<_, StoreError>((before, store.update_bot(bot_id, &patch)?))
        })?;
        let (Some(before), Some(after)) = (before, after) else {
            return Err(Refusal::Other(missing_bot(bot_id)));
        };
        let trusted = |row: &BotRow| row.bot.permission_level == PermissionLevel::FullAccess;
        if trusted(&after) && !trusted(&before) {
            // The requests that already wait are allowed, as later ones will be.
            for handle in self.running_conversations(bot_id)? {
                handle.send(Command::PermissionLevelChanged);
            }
        }
        self.publish_bot(bot_id);
        Ok(())
    }

    /// The conversations of a bot that have a task in this daemon.
    fn running_conversations(&self, bot_id: i64) -> Result<Vec<ConversationHandle>, StoreError> {
        let ids = self.store.with(|store| store.conversation_ids(bot_id))?;
        let state = self.state();
        Ok(ids
            .iter()
            .filter_map(|id| state.conversations.get(id).cloned())
            .collect())
    }

    /// Stops the running turns of a bot and ends the tasks of its
    /// conversations. Until `deleting` lets go of the bot, none starts again.
    async fn shut_down_conversations(&self, bot_id: i64) -> Result<(), String> {
        let running = {
            let mut state = self.state();
            if state.deleting.contains(&bot_id) {
                return Err("This bot's conversations are already being deleted.".to_owned());
            }
            let ids = self
                .store
                .with(|store| {
                    let bot = store.bot(bot_id)?;
                    Ok::<_, StoreError>((bot.is_some(), store.conversation_ids(bot_id)?))
                })
                .map_err(|error| error.to_string())?;
            let (true, ids) = ids else {
                return Err(missing_bot(bot_id));
            };
            state.deleting.insert(bot_id);
            ids.iter()
                .filter_map(|id| state.conversations.remove(id))
                .collect::<Vec<_>>()
        };
        for handle in running {
            let (done, stopped) = oneshot::channel();
            handle.send(Command::ShutDown {
                done,
                patience: conversation::STOP_TIMEOUT,
            });
            tokio::time::timeout(SHUTDOWN_PATIENCE, stopped).await.ok();
        }
        Ok(())
    }

    /// Stops every running turn, for a daemon that was told to stop: each
    /// waiting approval is answered as cancelled, and an agent that has not
    /// ended its turn after `patience` is closed. Returns once every
    /// conversation ended, or at `deadline`. From now on no conversation
    /// starts.
    pub async fn stop(&self, patience: Duration, deadline: tokio::time::Instant) {
        let running: Vec<ConversationHandle> = {
            let mut state = self.state();
            state.stopping = true;
            state
                .conversations
                .drain()
                .map(|(_, handle)| handle)
                .collect()
        };
        let mut ended = Vec::with_capacity(running.len());
        for handle in running {
            let (done, stopped) = oneshot::channel();
            handle.send(Command::ShutDown { done, patience });
            ended.push(stopped);
        }
        for stopped in ended {
            tokio::time::timeout_at(deadline, stopped).await.ok();
        }
    }

    /// Stops the running turns of a bot, then removes it and its conversations.
    /// The folder of the bot stays: it holds the user's files.
    pub async fn delete_bot(&self, bot_id: i64) -> Result<(), String> {
        self.shut_down_conversations(bot_id).await?;
        let deleted = self.store.with(|store| store.delete_bot(bot_id));
        let mut state = self.state();
        state.deleting.remove(&bot_id);
        state
            .activity
            .retain(|_, activity| activity.bot_id != bot_id);
        state.published.remove(&bot_id);
        deleted.map_err(|error| error.to_string())?;
        state.broadcast(&ServerMessage::BotDeleted { bot_id });
        Ok(())
    }

    /// Stops the running turns of a bot, then removes every conversation it
    /// has and gives it a new, empty one. The bot and its folder stay.
    pub async fn delete_history(&self, bot_id: i64) -> Result<(), String> {
        self.shut_down_conversations(bot_id).await?;
        let current = self.store.with(|store| store.delete_history(bot_id));
        {
            let mut state = self.state();
            state.deleting.remove(&bot_id);
            state
                .activity
                .retain(|_, activity| activity.bot_id != bot_id);
            let current = current
                .map_err(|error| error.to_string())?
                .ok_or_else(|| missing_bot(bot_id))?;
            state.broadcast(&ServerMessage::HistoryDeleted {
                bot_id,
                current_conversation_id: current,
            });
        }
        self.publish_bot(bot_id);
        Ok(())
    }

    pub fn list_conversations(&self, bot_id: i64) -> Result<ServerMessage, String> {
        let conversations = self
            .store
            .with(|store| {
                Ok::<_, StoreError>(match store.bot(bot_id)? {
                    Some(_) => Some(store.summaries(bot_id)?),
                    None => None,
                })
            })
            .map_err(|error| error.to_string())?
            .ok_or_else(|| missing_bot(bot_id))?;
        Ok(ServerMessage::Conversations {
            bot_id,
            conversations,
        })
    }

    /// Starts a conversation and makes it the bot's current one. The other
    /// conversations of the bot go on as they were.
    pub fn new_conversation(&self, bot_id: i64) -> Result<ConversationSummary, String> {
        if self.state().deleting.contains(&bot_id) {
            return Err("This bot's conversations are being deleted.".to_owned());
        }
        let summary = self
            .store
            .with(|store| store.new_conversation(bot_id))
            .map_err(|error| error.to_string())?
            .ok_or_else(|| missing_bot(bot_id))?;
        self.state()
            .broadcast(&ServerMessage::ConversationChanged(summary.clone()));
        self.publish_bot(bot_id);
        Ok(summary)
    }

    pub fn preferences(&self) -> ServerMessage {
        let preferences = self
            .store
            .with(|store| store.preferences())
            .unwrap_or_else(|error| {
                eprintln!("[crowbotd] cannot read the preferences: {error}");
                Preferences::default()
            });
        ServerMessage::Preferences(preferences)
    }

    pub fn set_preferences(&self, patch: &PreferencesPatch) -> Result<(), Refusal> {
        if patch
            .sidebar_width
            .is_some_and(|width| width > SIDEBAR_WIDTH_LIMIT)
        {
            return Err(invalid(
                "sidebar_width",
                format!("The sidebar can be {SIDEBAR_WIDTH_LIMIT} pixels wide at most."),
            ));
        }
        let mut state = self.state();
        let preferences = self.store.with(|store| store.update_preferences(patch))?;
        state.broadcast(&ServerMessage::Preferences(preferences));
        Ok(())
    }

    pub fn mark_bot_read(&self, bot_id: i64) -> Result<(), String> {
        self.move_bot_mark(bot_id, |store| store.mark_bot_read(bot_id).map(|()| true))
            .map(|_| ())
    }

    pub fn mark_bot_unread(&self, bot_id: i64) -> Result<(), String> {
        match self.move_bot_mark(bot_id, |store| store.mark_bot_unread(bot_id))? {
            true => Ok(()),
            false => Err("This bot has no finished turn to mark as unread.".to_owned()),
        }
    }

    fn move_bot_mark(
        &self,
        bot_id: i64,
        mark: impl FnOnce(&crate::store::Store) -> Result<bool, StoreError>,
    ) -> Result<bool, String> {
        let moved = self
            .store
            .with(|store| {
                Ok::<_, StoreError>(match store.bot(bot_id)? {
                    Some(_) => Some(mark(store)?),
                    None => None,
                })
            })
            .map_err(|error| error.to_string())?
            .ok_or_else(|| missing_bot(bot_id))?;
        self.publish_bot(bot_id);
        Ok(moved)
    }
}

fn checked_name(name: &str) -> Result<String, Refusal> {
    let name = name.trim();
    match name.chars().count() {
        0 => Err(invalid("name", "A bot needs a name.")),
        length if length > BOT_NAME_LENGTH => Err(invalid(
            "name",
            format!("A bot's name can hold {BOT_NAME_LENGTH} characters at most."),
        )),
        _ => Ok(name.to_owned()),
    }
}

fn checked_text(field: &'static str, text: &str, limit: usize) -> Result<String, Refusal> {
    if text.chars().count() > limit {
        return Err(invalid(
            field,
            format!("This field can hold {limit} characters at most."),
        ));
    }
    Ok(text.to_owned())
}

fn checked_avatar(avatar: Avatar) -> Result<Avatar, Refusal> {
    let color = avatar.color.trim();
    let hex = color.strip_prefix('#').unwrap_or_default();
    if hex.len() != 6 || !hex.chars().all(|digit| digit.is_ascii_hexdigit()) {
        return Err(invalid(
            "avatar",
            "An avatar color is written #rrggbb, for example #3b82f6.",
        ));
    }
    Ok(Avatar {
        color: color.to_ascii_lowercase(),
        ..avatar
    })
}

/// A model or a reasoning level with no text is the provider's default.
fn checked_choice(field: &'static str, choice: Option<String>) -> Result<Option<String>, Refusal> {
    let Some(choice) = choice.map(|choice| choice.trim().to_owned()) else {
        return Ok(None);
    };
    if choice.chars().count() > MODEL_LENGTH {
        return Err(invalid(
            field,
            format!("This field can hold {MODEL_LENGTH} characters at most."),
        ));
    }
    Ok((!choice.is_empty()).then_some(choice))
}

fn checked_workspace(workspace: &str) -> Result<String, Refusal> {
    let folder = Path::new(workspace);
    if !folder.is_absolute() {
        return Err(invalid(
            "workspace",
            format!("The folder {workspace} is not a full path."),
        ));
    }
    if !folder.is_dir() {
        return Err(invalid(
            "workspace",
            format!("The folder {workspace} does not exist."),
        ));
    }
    Ok(workspace.to_owned())
}

fn checked_patch(patch: BotPatch) -> Result<BotPatch, Refusal> {
    Ok(BotPatch {
        name: patch.name.as_deref().map(checked_name).transpose()?,
        title: patch
            .title
            .map(|title| checked_text("title", title.trim(), TITLE_LENGTH))
            .transpose()?,
        instructions: patch
            .instructions
            .map(|instructions| checked_text("instructions", &instructions, INSTRUCTIONS_LENGTH))
            .transpose()?,
        avatar: patch.avatar.map(checked_avatar).transpose()?,
        model: patch
            .model
            .map(|model| checked_choice("model", model))
            .transpose()?,
        effort: patch
            .effort
            .map(|effort| checked_choice("effort", effort))
            .transpose()?,
        provider: patch.provider,
        workspace: patch
            .workspace
            .as_deref()
            .map(checked_workspace)
            .transpose()?,
        permission_level: patch.permission_level,
        pinned: patch.pinned,
        hidden: patch.hidden,
    })
}

/// The folder name of a bot: its name in lower case, with a dash for each run
/// of characters that are neither letters nor digits.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            slug.push(character);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.chars().count() >= SLUG_LENGTH {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    // Windows keeps these names for devices, whatever the folder.
    let device = matches!(slug, "con" | "prn" | "aux" | "nul")
        || (slug.len() == 4
            && (slug.starts_with("com") || slug.starts_with("lpt"))
            && slug.ends_with(|last: char| last.is_ascii_digit()));
    match slug {
        "" => "bot".to_owned(),
        slug if device => format!("{slug}-bot"),
        slug => slug.to_owned(),
    }
}

/// The name of a duplicate: "Atlas copy", then "Atlas copy 2", and so on, the
/// first that no bot has, whatever the letter case.
fn copy_name(source: &str, taken: &[String]) -> String {
    let base = copy_base(source);
    let taken: Vec<String> = taken.iter().map(|name| name.to_lowercase()).collect();
    (1_u32..)
        .map(|number| {
            let suffix = match number {
                1 => " copy".to_owned(),
                number => format!(" copy {number}"),
            };
            let room = BOT_NAME_LENGTH.saturating_sub(suffix.chars().count());
            let kept: String = base.chars().take(room).collect();
            format!("{}{suffix}", kept.trim_end())
        })
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .unwrap_or_else(|| source.to_owned())
}

/// The name without the " copy" or " copy 3" that an earlier duplicate added.
fn copy_base(name: &str) -> &str {
    let numbered = name
        .trim_end_matches(|last: char| last.is_ascii_digit())
        .trim_end();
    [name, numbered]
        .into_iter()
        .filter_map(|candidate| candidate.strip_suffix(" copy"))
        .map(str::trim_end)
        .find(|base| !base.is_empty())
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(refusal: Result<impl std::fmt::Debug, Refusal>) -> &'static str {
        match refusal {
            Err(Refusal::Field { field, .. }) => field,
            other => panic!("expected a refused field, got {other:?}"),
        }
    }

    #[test]
    fn a_name_is_trimmed_and_holds_one_to_eighty_characters() {
        assert_eq!(checked_name("  Atlas ").unwrap(), "Atlas");
        assert_eq!(field(checked_name("")), "name");
        assert_eq!(field(checked_name("   ")), "name");
        assert_eq!(checked_name(&"é".repeat(80)).unwrap().chars().count(), 80);
        assert_eq!(field(checked_name(&"é".repeat(81))), "name");
    }

    #[test]
    fn a_patch_is_checked_field_by_field() {
        let patch = |patch: BotPatch| checked_patch(patch);
        assert_eq!(
            field(patch(BotPatch {
                name: Some(" ".to_owned()),
                ..BotPatch::default()
            })),
            "name"
        );
        assert_eq!(
            field(patch(BotPatch {
                instructions: Some("x".repeat(INSTRUCTIONS_LENGTH + 1)),
                ..BotPatch::default()
            })),
            "instructions"
        );
        assert_eq!(
            field(patch(BotPatch {
                workspace: Some("relative/folder".to_owned()),
                ..BotPatch::default()
            })),
            "workspace"
        );
        assert_eq!(
            field(patch(BotPatch {
                avatar: Some(Avatar {
                    color: "blue".to_owned(),
                    shape: Default::default(),
                }),
                ..BotPatch::default()
            })),
            "avatar"
        );
        let kept = patch(BotPatch {
            name: Some(" Orion ".to_owned()),
            model: Some(Some("  ".to_owned())),
            ..BotPatch::default()
        })
        .unwrap();
        assert_eq!(kept.name.as_deref(), Some("Orion"));
        assert_eq!(kept.model, Some(None));
        assert_eq!(kept.title, None);
    }

    #[test]
    fn a_folder_name_comes_from_the_bot_name() {
        assert_eq!(slug("Atlas"), "atlas");
        assert_eq!(slug("  Research & Notes!  "), "research-notes");
        assert_eq!(slug("Éloïse 2"), "éloïse-2");
        assert_eq!(slug("???"), "bot");
        assert_eq!(slug("CON"), "con-bot");
        assert_eq!(slug("Com1"), "com1-bot");
        assert_eq!(slug(&"a".repeat(200)).chars().count(), SLUG_LENGTH);
    }

    #[test]
    fn a_duplicate_takes_the_first_free_copy_name() {
        let taken = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(copy_name("Atlas", &taken(&["Atlas"])), "Atlas copy");
        assert_eq!(
            copy_name("Atlas", &taken(&["Atlas", "atlas COPY"])),
            "Atlas copy 2"
        );
        assert_eq!(
            copy_name(
                "Atlas copy 2",
                &taken(&["Atlas", "Atlas copy", "Atlas copy 2"])
            ),
            "Atlas copy 3"
        );
        assert_eq!(copy_name("Bot 2", &taken(&["Bot 2"])), "Bot 2 copy");
        let long = copy_name(&"n".repeat(80), &[]);
        assert_eq!(long.chars().count(), BOT_NAME_LENGTH);
        assert!(long.ends_with(" copy"));
    }
}
