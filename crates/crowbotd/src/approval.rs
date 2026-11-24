//! What answers an approval request without a person (`AGENTS.md`, "What
//! answers an approval without a person"): the built-in floors, then the
//! bot's rules (refuse before allow), then the bot's level. A daemon tool,
//! the third source, is answered where the agent's request is read
//! (`provider/acp.rs`).
//!
//! A request whose target was cut is never allowed, by any source and by
//! no person: nobody could read it in full.

pub mod command;
pub mod floors;

use std::path::{Path, PathBuf};

use crowbot_protocol::{ApprovalReason, ApprovalRule, PermissionLevel, RuleKind};

use self::command::{Parsed, Simple};
use self::floors::{Floors, Layer, Verdict, holds, mentions, program_name};

/// What an agent asked to be allowed, as the daemon reads it.
#[derive(Debug, Clone, Default)]
pub struct Request {
    /// The command, or the path, the request names.
    pub target: Option<String>,
    /// The target was longer than the daemon keeps, so it was cut.
    pub cut: bool,
    /// The target is a command a shell runs.
    pub command: bool,
    /// The folder the command runs in, when the agent names it in a field
    /// the model does not write (`AgentProfile::command_folder`).
    pub folder: Option<PathBuf>,
    /// Only a person answers it: a change the scan found dangerous in, which
    /// no level or rule allows.
    pub person: bool,
}

/// Where the bot stands when a request is answered: read from its record
/// at that moment, and from nowhere else.
#[derive(Debug, Clone)]
pub struct Standing {
    pub level: PermissionLevel,
    /// The bot's folder.
    pub folder: PathBuf,
    pub rules: Vec<ApprovalRule>,
}

/// What the daemon does with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Allow(ApprovalReason),
    Decline(ApprovalReason),
    /// Only a person answers it.
    Wait,
}

/// The answer to a request, from the four sources in their order. A
/// `standing` the daemon could not read is `None`: the request then waits,
/// unless a floor refuses it.
pub fn answer(request: &Request, standing: Option<&Standing>, floors: &Floors) -> Answer {
    let mut reading = Reading::of(request, standing);
    if let Some(refusal) = reading.refusal(floors, standing) {
        return Answer::Decline(refusal);
    }
    let Some(standing) = standing else {
        return Answer::Wait;
    };
    if request.cut || reading.held || request.person {
        return Answer::Wait;
    }
    if let Some(rule) = reading.allow_rule(request, standing) {
        return Answer::Allow(ApprovalReason::Rule { id: rule });
    }
    match standing.level {
        PermissionLevel::FullAccess => Answer::Allow(ApprovalReason::PermissionLevel),
        PermissionLevel::AskFirst => Answer::Wait,
    }
}

/// What refuses a request a person allows: a floor or a refuse rule, read
/// at the moment of the answer.
pub fn refusal(
    request: &Request,
    standing: Option<&Standing>,
    floors: &Floors,
) -> Option<ApprovalReason> {
    Reading::of(request, standing).refusal(floors, standing)
}

/// A request's command, read once for every source.
struct Reading {
    parsed: Parsed,
    layers: Vec<Layer>,
    /// The folders relative paths are read from: the request's and the
    /// bot's.
    folders: Vec<PathBuf>,
    /// A source could not tell what the command does: no source but a
    /// person may allow it.
    held: bool,
}

impl Reading {
    fn of(request: &Request, standing: Option<&Standing>) -> Self {
        let mut folders: Vec<PathBuf> = request
            .folder
            .iter()
            .chain(standing.map(|standing| &standing.folder))
            .cloned()
            .collect();
        if folders.is_empty() {
            folders.push(PathBuf::new());
        }
        let (parsed, layers) = match (&request.target, request.command) {
            (Some(target), true) => {
                let parsed = command::parse(target, request.cut);
                let layers = floors::layers(&parsed, target);
                (parsed, layers)
            }
            _ => (Parsed::Opaque, Vec::new()),
        };
        Self {
            parsed,
            layers,
            folders,
            held: false,
        }
    }

    fn refusal(&mut self, floors: &Floors, standing: Option<&Standing>) -> Option<ApprovalReason> {
        match floors.check(&self.layers, &self.folders) {
            Verdict::Refuse => return Some(ApprovalReason::Floor),
            Verdict::Unsure => self.held = true,
            Verdict::Clear => {}
        }
        let refuse_rules = standing
            .into_iter()
            .flat_map(|standing| &standing.rules)
            .filter(|rule| rule.kind == RuleKind::Refuse);
        for rule in refuse_rules {
            for layer in &self.layers {
                match layer {
                    Layer::Simple(simple) if refuses(rule, simple) => {
                        return Some(ApprovalReason::Rule { id: rule.id });
                    }
                    Layer::Opaque(text)
                        if mentions(text, |name| name == program_name(&rule.program)) =>
                    {
                        self.held = true;
                    }
                    _ => {}
                }
            }
        }
        None
    }

    fn allow_rule(&self, request: &Request, standing: &Standing) -> Option<i64> {
        let Parsed::Simple(simple) = &self.parsed else {
            return None;
        };
        // The folder the command runs in must be named by the agent, and
        // be inside the bot's folder.
        let folder = request.folder.as_deref()?;
        if !inside(folder, &standing.folder) {
            return None;
        }
        standing
            .rules
            .iter()
            .filter(|rule| rule.kind == RuleKind::Allow)
            .find(|rule| allows(rule, simple))
            .map(|rule| rule.id)
    }
}

/// An allow rule matches the program as written, and arguments that start
/// with the rule's: `./cargo` is not `cargo`, and `git push` is not
/// `git status`.
fn allows(rule: &ApprovalRule, simple: &Simple) -> bool {
    simple.program == rule.program && simple.args.starts_with(&rule.args)
}

/// A refuse rule matches the program by its name, from any folder, and the
/// rule's arguments in their order among the command's: `git -C x push`
/// is refused by `git push`.
fn refuses(rule: &ApprovalRule, simple: &Simple) -> bool {
    if program_name(&simple.program) != program_name(&rule.program) {
        return false;
    }
    let mut wanted = rule.args.iter().peekable();
    for arg in &simple.args {
        if wanted.peek().is_some_and(|next| *next == arg) {
            wanted.next();
        }
    }
    wanted.peek().is_none()
}

/// Whether `folder` is the bot's folder or inside it, read on the disk
/// when both exist, so that a link does not lead out of it.
fn inside(folder: &Path, bot_folder: &Path) -> bool {
    match (folder.canonicalize(), bot_folder.canonicalize()) {
        (Ok(folder), Ok(bot_folder)) => holds(&bot_folder, &folder),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crowbot_protocol::scratch::Scratch;

    fn rule(id: i64, kind: RuleKind, program: &str, args: &[&str]) -> ApprovalRule {
        ApprovalRule {
            id,
            bot_id: 1,
            kind,
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            created_at_ms: 0,
        }
    }

    fn floors() -> Floors {
        Floors::new(None, Vec::new())
    }

    fn command(text: &str, folder: Option<&Path>) -> Request {
        Request {
            target: Some(text.to_owned()),
            cut: false,
            command: true,
            folder: folder.map(Path::to_path_buf),
            person: false,
        }
    }

    #[test]
    fn rules_match_parsed_commands() {
        let scratch = Scratch::new("approval-rules");
        let bot = scratch.path().join("bot");
        std::fs::create_dir_all(bot.join("sub")).unwrap();
        let outside = scratch.path().join("other");
        std::fs::create_dir_all(&outside).unwrap();
        let standing = |level, rules| Standing {
            level,
            folder: bot.clone(),
            rules,
        };
        let ask = standing(
            PermissionLevel::AskFirst,
            vec![
                rule(1, RuleKind::Allow, "git", &["status"]),
                rule(2, RuleKind::Allow, "cargo", &["test"]),
                rule(3, RuleKind::Refuse, "cargo", &["test", "--release"]),
            ],
        );
        let answer =
            |request: &Request, standing: &Standing| answer(request, Some(standing), &floors());
        let in_bot = Some(bot.as_path());
        assert_eq!(
            answer(&command("git status --short", in_bot), &ask),
            Answer::Allow(ApprovalReason::Rule { id: 1 })
        );
        assert_eq!(
            answer(&command("cargo test", Some(&bot.join("sub"))), &ask),
            Answer::Allow(ApprovalReason::Rule { id: 2 })
        );
        assert_eq!(answer(&command("git push", in_bot), &ask), Answer::Wait);
        assert_eq!(answer(&command("./git status", in_bot), &ask), Answer::Wait);
        assert_eq!(
            answer(&command("git status | cat", in_bot), &ask),
            Answer::Wait
        );
        assert_eq!(answer(&command("git status", None), &ask), Answer::Wait);
        assert_eq!(
            answer(&command("git status", Some(&outside)), &ask),
            Answer::Wait
        );
        assert_eq!(
            answer(&command("git status", Some(&bot.join("sub/../.."))), &ask),
            Answer::Wait
        );
        let mut cut = command("git status", in_bot);
        cut.cut = true;
        assert_eq!(answer(&cut, &ask), Answer::Wait);
        // Refuse wins over allow, and needs no folder.
        assert_eq!(
            answer(&command("cargo test --release", in_bot), &ask),
            Answer::Decline(ApprovalReason::Rule { id: 3 })
        );
        assert_eq!(
            answer(&command("/usr/bin/cargo test -q --release", None), &ask),
            Answer::Decline(ApprovalReason::Rule { id: 3 })
        );
        assert_eq!(
            answer(&command("sudo cargo test --release", None), &ask),
            Answer::Decline(ApprovalReason::Rule { id: 3 })
        );
        let full = standing(
            PermissionLevel::FullAccess,
            vec![rule(4, RuleKind::Refuse, "git", &["push"])],
        );
        assert_eq!(
            answer(&command("git -C x push origin", None), &full),
            Answer::Decline(ApprovalReason::Rule { id: 4 })
        );
        // An opaque command that names the program of a refuse rule waits.
        assert_eq!(
            answer(&command("cd x && git push", None), &full),
            Answer::Wait
        );
        assert_eq!(
            answer(&command("cargo build && ls", None), &full),
            Answer::Allow(ApprovalReason::PermissionLevel)
        );
    }

    #[test]
    fn a_cut_request_is_never_allowed_and_a_floor_wins_over_every_rule() {
        let standing = Standing {
            level: PermissionLevel::FullAccess,
            folder: PathBuf::from("/nowhere"),
            rules: vec![rule(1, RuleKind::Allow, "rm", &["-rf"])],
        };
        let mut cut = command("ls", None);
        cut.cut = true;
        assert_eq!(answer(&cut, Some(&standing), &floors()), Answer::Wait);
        let file = Request {
            target: Some("notes.txt".to_owned()),
            cut: true,
            command: false,
            folder: None,
            person: false,
        };
        assert_eq!(answer(&file, Some(&standing), &floors()), Answer::Wait);
        let file = Request { cut: false, ..file };
        assert_eq!(
            answer(&file, Some(&standing), &floors()),
            Answer::Allow(ApprovalReason::PermissionLevel)
        );
        if cfg!(unix) {
            assert_eq!(
                answer(
                    &command("rm -rf /", Some(Path::new("/nowhere"))),
                    Some(&standing),
                    &floors()
                ),
                Answer::Decline(ApprovalReason::Floor)
            );
        }
        // A relative path is read from the folder the request names and
        // from the bot's: a folder the agent names does not hide where the
        // command reaches when it runs in the bot's.
        if cfg!(unix) {
            let standing = Standing {
                folder: PathBuf::from("/home/ada/CrowBot/Bots/ada"),
                ..standing.clone()
            };
            let floors = Floors::new(Some(PathBuf::from("/home/ada")), Vec::new());
            for named in ["/elsewhere/a/b/c/d", "/home/ada/CrowBot/Bots/ada"] {
                assert_eq!(
                    answer(
                        &command("rm -rf ../../..", Some(Path::new(named))),
                        Some(&standing),
                        &floors
                    ),
                    Answer::Decline(ApprovalReason::Floor),
                    "{named}"
                );
            }
            assert_eq!(
                answer(
                    &command(
                        "rm -rf ../../..",
                        Some(Path::new("/home/ada/CrowBot/Bots/ada/x/y/z"))
                    ),
                    Some(&standing),
                    &floors
                ),
                Answer::Decline(ApprovalReason::Floor)
            );
        }
        // A bot whose record cannot be read is not allowed anything.
        assert_eq!(answer(&command("ls", None), None, &floors()), Answer::Wait);
    }
}
