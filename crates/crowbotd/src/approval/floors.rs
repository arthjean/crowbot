//! The built-in refusals: commands that destroy the computer, the user's
//! home folder, or the daemon itself. They apply to every bot, on every
//! level, before any rule of the bot (US-014 of
//! `tasks/prd-crow-bot-harness.md`).
//!
//! A floor reads parsed commands, never text, so quoting does not hide a
//! program from it. Through the commands that run another one (`sudo`,
//! `env`, `sh -c`, and the like) it reads the inner command too. A command
//! it cannot read, because it is opaque, waits for a person when it names
//! a program of a floor: no level allows it.

use std::path::{Component, Path, PathBuf};

use super::command::{self, Parsed, Simple};

/// How deep a floor follows commands that run a command.
const UNWRAP_DEPTH: usize = 8;

/// The programs whose name makes an opaque command wait for a person.
const FLOOR_PROGRAMS: [&str; 11] = [
    "rm", "dd", "mkfs", "mke2fs", "mkswap", "wipefs", "shutdown", "reboot", "poweroff", "halt",
    "kexec",
];

/// The same on Windows, where PowerShell and `cmd.exe` name them otherwise.
const WINDOWS_FLOOR_PROGRAMS: [&str; 13] = [
    "remove-item",
    "ri",
    "del",
    "erase",
    "rd",
    "rmdir",
    "format",
    "format-volume",
    "clear-disk",
    "initialize-disk",
    "diskpart",
    "stop-computer",
    "restart-computer",
];

/// What a floor says of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Declined at once.
    Refuse,
    /// The daemon cannot tell: only a person may allow it.
    Unsure,
    Clear,
}

/// The folders and files the floors protect on this computer.
#[derive(Debug, Clone)]
pub struct Floors {
    /// A recursive delete of one of these, or of a folder that holds one, is refused.
    protected: Vec<PathBuf>,
    /// Any delete of one of these, or of a folder that holds one, is refused.
    daemon: Vec<PathBuf>,
}

/// The command a floor reads, one layer of wrapping at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    Simple(Simple),
    /// A text the daemon could not reduce to one simple command.
    Opaque(String),
}

impl Floors {
    /// `home` is the user's home folder; `daemon` holds the daemon's data
    /// folder, its database, and its program.
    pub fn new(home: Option<PathBuf>, daemon: Vec<PathBuf>) -> Self {
        Self {
            protected: with_targets(protected_folders(home)),
            daemon: with_targets(daemon.iter().map(|path| normalize(path)).collect()),
        }
    }

    /// What the floors say of a command request. A relative path is read
    /// from each of `folders`, the folder the request names and the bot's:
    /// an agent may name one and run the command in the other, so a floor
    /// that either reading reaches refuses.
    pub fn check(&self, layers: &[Layer], folders: &[PathBuf]) -> Verdict {
        let mut verdict = Verdict::Clear;
        for layer in layers {
            let found = match layer {
                Layer::Simple(simple) => self.check_simple(simple, folders),
                Layer::Opaque(text) => {
                    if mentions(text, is_floor_program) || names_block_device(text) {
                        Verdict::Unsure
                    } else {
                        Verdict::Clear
                    }
                }
            };
            match found {
                Verdict::Refuse => return Verdict::Refuse,
                Verdict::Unsure => verdict = Verdict::Unsure,
                Verdict::Clear => {}
            }
        }
        verdict
    }

    fn check_simple(&self, simple: &Simple, folders: &[PathBuf]) -> Verdict {
        let name = program_name(&simple.program);
        let args = &simple.args;
        match name.as_str() {
            "rm" | "remove-item" | "ri" | "del" | "erase" | "rd" | "rmdir" => {
                self.check_delete(&name, args, folders)
            }
            "find" if args.iter().any(|arg| arg == "-delete") => {
                // The starting points follow the options that come first
                // (`-L`, `-H`, `-P`, `-D debug`, `-O3`).
                let mut rest = args.iter().peekable();
                while let Some(option) = rest.next_if(|arg| {
                    matches!(arg.as_str(), "-H" | "-L" | "-P" | "-D") || arg.starts_with("-O")
                }) {
                    if option == "-D" {
                        rest.next();
                    }
                }
                let starts =
                    rest.take_while(|arg| !arg.starts_with('-') && *arg != "(" && *arg != "!");
                self.check_targets(starts, folders, true)
            }
            "dd" => {
                let raw = args.iter().any(|arg| {
                    arg.strip_prefix("of=")
                        .is_some_and(|output| is_block_device(output, folders))
                });
                if raw { Verdict::Refuse } else { Verdict::Clear }
            }
            _ if is_format(&name) => Verdict::Refuse,
            "shutdown" | "reboot" | "poweroff" | "halt" | "kexec" | "stop-computer"
            | "restart-computer" => Verdict::Refuse,
            "init" | "telinit" if args.iter().any(|arg| arg == "0" || arg == "6") => {
                Verdict::Refuse
            }
            "systemctl" | "loginctl"
                if args.iter().any(|arg| {
                    matches!(arg.as_str(), "poweroff" | "reboot" | "halt" | "kexec")
                }) =>
            {
                Verdict::Refuse
            }
            _ => Verdict::Clear,
        }
    }

    fn check_delete(&self, name: &str, args: &[String], folders: &[PathBuf]) -> Verdict {
        let mut recursive = false;
        let mut targets = Vec::new();
        let mut operands = false;
        for arg in args {
            if operands {
                targets.push(arg);
                continue;
            }
            let lower = arg.to_lowercase();
            if arg == "--" {
                operands = true;
            } else if is_recursive_long(&lower) || lower.starts_with("-rec") || lower == "/s" {
                recursive = true;
            } else if let Some(flags) = arg
                .strip_prefix('-')
                .filter(|flags| !flags.starts_with('-'))
            {
                // `-rf`, `-Rf`, and PowerShell's `-r`. A POSIX `rm` takes a
                // cluster of any length (`-rfvvv`); PowerShell's parameters
                // are words, so there only a short one reads as a cluster
                // (`-Force` is not recursive).
                let cluster = (cfg!(unix) && name == "rm") || flags.len() <= 4;
                if cluster && flags.contains(['r', 'R']) {
                    recursive = true;
                }
            } else if !arg.starts_with('-')
                && !(arg.starts_with('/') && arg.len() == 2 && name != "rm")
            {
                targets.push(arg);
            }
        }
        if name == "rd" || name == "rmdir" {
            // These remove what a folder holds only with `/s`.
            recursive = recursive && cfg!(windows);
        }
        self.check_targets(targets.into_iter(), folders, recursive)
    }

    fn check_targets<'a>(
        &self,
        targets: impl Iterator<Item = &'a String>,
        folders: &[PathBuf],
        recursive: bool,
    ) -> Verdict {
        for target in targets {
            // A link in the path leads where the delete goes: both the path
            // as written and the one on the disk are read.
            let paths = folders.iter().flat_map(|folder| {
                let joined = folder.join(target);
                [Some(normalize(&joined)), joined.canonicalize().ok()]
            });
            for path in paths.flatten() {
                if self.daemon.iter().any(|kept| holds(&path, kept)) {
                    return Verdict::Refuse;
                }
                if recursive && self.protected.iter().any(|kept| holds(&path, kept)) {
                    return Verdict::Refuse;
                }
            }
        }
        Verdict::Clear
    }
}

/// The folders a recursive delete may never reach, on this system.
pub fn protected_folders(home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = if cfg!(windows) {
        let variable = |name: &str, default: &str| {
            std::env::var_os(name).map_or_else(|| PathBuf::from(default), PathBuf::from)
        };
        let mut folders: Vec<PathBuf> = ('A'..='Z')
            .map(|drive| PathBuf::from(format!("{drive}:\\")))
            .collect();
        folders.push(variable("SystemRoot", r"C:\Windows"));
        folders.push(variable("ProgramFiles", r"C:\Program Files"));
        folders.push(variable("ProgramFiles(x86)", r"C:\Program Files (x86)"));
        folders.push(variable("ProgramData", r"C:\ProgramData"));
        folders
    } else {
        [
            "/", "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/var", "/sys",
            "/proc", "/dev", "/home", "/root",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    };
    folders.extend(home);
    folders.iter().map(|folder| normalize(folder)).collect()
}

/// The paths as written, then each one as the disk resolves it, when a
/// link leads elsewhere (a home folder under `/var/home`).
fn with_targets(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let resolved: Vec<PathBuf> = paths
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .filter(|resolved| !paths.contains(resolved))
        .collect();
    let mut all = paths;
    for path in resolved {
        if !all.contains(&path) {
            all.push(path);
        }
    }
    all
}

/// Each command a floor or a rule reads in a request: the command itself,
/// then the commands it runs, as far as they can be read.
pub fn layers(parsed: &Parsed, text: &str) -> Vec<Layer> {
    let mut layers = Vec::new();
    match parsed {
        Parsed::Simple(simple) => unwrap(simple, &mut layers, 0),
        Parsed::Opaque => layers.push(Layer::Opaque(text.to_owned())),
    }
    layers
}

fn unwrap(simple: &Simple, layers: &mut Vec<Layer>, depth: usize) {
    layers.push(Layer::Simple(simple.clone()));
    if depth >= UNWRAP_DEPTH {
        layers.push(Layer::Opaque(simple.args.join(" ")));
        return;
    }
    let name = program_name(&simple.program);
    let args = &simple.args;
    let inner = match name.as_str() {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash" | "busybox-sh" | "fish" => {
            if args.iter().any(|arg| arg.starts_with("--command")) {
                // fish's long form, which may hold the script after `=`.
                layers.push(Layer::Opaque(args.join(" ")));
                return;
            }
            match args.iter().position(|arg| is_dash_c(arg)) {
                Some(index) => {
                    script_after(args, index + 1, layers, depth);
                    return;
                }
                None => None,
            }
        }
        "su" | "runuser" => {
            // They run the script of `-c`, `--command x`, or `--command=x`.
            if let Some(script) = args.iter().find_map(|arg| arg.strip_prefix("--command=")) {
                script_layers(script, layers, depth);
                return;
            }
            match args
                .iter()
                .position(|arg| is_dash_c(arg) || arg == "--command")
            {
                Some(index) => script_after(args, index + 1, layers, depth),
                // `runuser -u x cmd` runs a command, and `su x args` hands
                // its arguments to a shell: neither is read here.
                None => layers.push(Layer::Opaque(args.join(" "))),
            }
            return;
        }
        "flock" => {
            let rest = skip_options(args, &FLOCK, 1, layers);
            match rest.and_then(|rest| rest.first()) {
                Some(first) if first == "-c" || first == "--command" => {
                    let start = args.len() - rest.map_or(0, <[String]>::len) + 1;
                    script_after(args, start, layers, depth);
                    return;
                }
                _ => rest,
            }
        }
        "cmd" | "powershell" | "pwsh" => {
            // These read the rest of the line as their own command.
            layers.push(Layer::Opaque(args.join(" ")));
            return;
        }
        "sudo" | "doas" | "pkexec" | "sudo.exe" | "doas.exe" => {
            let rest = skip_options(args, &SUDO, 0, layers);
            // `-D` and `--chdir` move the command, `-R` and `--chroot` its root:
            // its paths are no longer read from the request's folder.
            let moved = |arg: &String| {
                arg.starts_with("--chdir")
                    || arg.starts_with("--chroot")
                    || (!arg.starts_with("--") && (arg.contains('D') || arg.contains('R')))
            };
            moves(args, rest, moved, layers);
            rest
        }
        "env" => {
            if args
                .iter()
                .any(|arg| arg == "-S" || arg.starts_with("--split-string"))
            {
                layers.push(Layer::Opaque(args.join(" ")));
                return;
            }
            let rest = skip_options(args, &ENV, 0, layers);
            let moved = |arg: &String| {
                arg.starts_with("--chdir") || (!arg.starts_with("--") && arg.contains('C'))
            };
            moves(args, rest, moved, layers);
            rest.map(|rest| {
                let assignments = rest.iter().take_while(|arg| arg.contains('=')).count();
                &rest[assignments..]
            })
        }
        "nohup" | "builtin" | "busybox" => skip_options(args, &NONE, 0, layers),
        "time" => skip_options(args, &TIME, 0, layers),
        "command" => skip_options(args, &COMMAND, 0, layers),
        "exec" => skip_options(args, &EXEC, 0, layers),
        "setsid" => skip_options(args, &SETSID, 0, layers),
        "nice" => skip_options(args, &NICE, 0, layers),
        "ionice" => skip_options(args, &IONICE, 0, layers),
        "stdbuf" => skip_options(args, &STDBUF, 0, layers),
        "timeout" => skip_options(args, &TIMEOUT, 1, layers),
        "chrt" => skip_options(args, &CHRT, 1, layers),
        "taskset" => skip_options(args, &TASKSET, 1, layers),
        "xargs" => {
            // The inner command gets arguments nobody has read.
            layers.push(Layer::Opaque(args.join(" ")));
            skip_options(args, &XARGS, 0, layers)
        }
        _ => None,
    };
    if let Some(rest) = inner
        && let Some((program, args)) = rest.split_first()
    {
        let inner = Simple {
            program: program.clone(),
            args: args.to_vec(),
        };
        unwrap(&inner, layers, depth + 1);
    }
}

/// Whether an option of a shell is `-c`, alone or in a cluster of letters
/// of any length (`-lc`, `-euxc`).
fn is_dash_c(arg: &str) -> bool {
    arg.strip_prefix('-').is_some_and(|flags| {
        flags.contains('c') && flags.chars().all(|flag| flag.is_ascii_alphabetic())
    })
}

/// The layers of the script that follows the options after `-c`: `--`
/// ends them. A script read past other options may not be the one the
/// shell runs, so the whole line is opaque too.
fn script_after(args: &[String], start: usize, layers: &mut Vec<Layer>, depth: usize) {
    let mut skipped = false;
    let mut rest = args.iter().skip(start);
    let mut script = None;
    while let Some(arg) = rest.next() {
        if arg == "--" {
            script = rest.next();
            break;
        }
        if arg.starts_with(['-', '+']) {
            skipped = true;
            continue;
        }
        script = Some(arg);
        break;
    }
    if skipped {
        layers.push(Layer::Opaque(args.join(" ")));
    }
    script_layers(script.map_or("", String::as_str), layers, depth);
}

fn script_layers(script: &str, layers: &mut Vec<Layer>, depth: usize) {
    match command::parse_native(script) {
        Parsed::Simple(inner) => unwrap(&inner, layers, depth + 1),
        Parsed::Opaque => layers.push(Layer::Opaque(script.to_owned())),
    }
}

/// A wrapper whose options move the command to another folder or root
/// (`env -C`, `sudo -D`): a relative path of the inner command is not read
/// from the request's folder, so the inner command is also opaque, and a
/// floor program in it waits for a person.
fn moves(
    args: &[String],
    rest: Option<&[String]>,
    moved: impl Fn(&String) -> bool,
    layers: &mut Vec<Layer>,
) {
    let options = &args[..args.len() - rest.map_or(0, <[String]>::len)];
    if options.iter().any(|arg| arg.starts_with('-') && moved(arg)) {
        layers.push(Layer::Opaque(args.join(" ")));
    }
}

/// The options of a command that runs a command: those that take a value,
/// and those that take none. Any other option may take a value nobody
/// knows, so the word read as the inner program may not be it.
struct Options {
    value: &'static [&'static str],
    flags: &'static [&'static str],
    /// An option made of digits (`nice -5`).
    digits: bool,
}

const NONE: Options = Options {
    value: &[],
    flags: &[],
    digits: false,
};
const SUDO: Options = Options {
    value: &[
        "-u",
        "-g",
        "-C",
        "-D",
        "-h",
        "-p",
        "-r",
        "-t",
        "-U",
        "-R",
        "-T",
        "--user",
        "--group",
        "--close-from",
        "--chdir",
        "--chroot",
        "--host",
        "--prompt",
        "--role",
        "--type",
        "--other-user",
        "--command-timeout",
    ],
    flags: &[
        "-A",
        "-b",
        "-E",
        "-e",
        "-H",
        "-i",
        "-K",
        "-k",
        "-l",
        "-L",
        "-n",
        "-P",
        "-S",
        "-s",
        "-V",
        "-v",
        "-B",
        "-N",
        "--askpass",
        "--background",
        "--preserve-env",
        "--edit",
        "--set-home",
        "--login",
        "--remove-timestamp",
        "--reset-timestamp",
        "--list",
        "--non-interactive",
        "--preserve-groups",
        "--stdin",
        "--shell",
        "--validate",
        "--bell",
        "--keep-cwd",
        "--disable-internal-agent",
    ],
    digits: false,
};
const ENV: Options = Options {
    value: &["-u", "--unset", "-C", "--chdir"],
    flags: &[
        "-",
        "-i",
        "--ignore-environment",
        "-0",
        "--null",
        "-v",
        "--debug",
        "--default-signal",
        "--ignore-signal",
        "--block-signal",
        "--list-signal-handling",
    ],
    digits: false,
};
const TIME: Options = Options {
    value: &["-f", "--format", "-o", "--output"],
    flags: &[
        "-a",
        "--append",
        "-p",
        "--portability",
        "-q",
        "--quiet",
        "-v",
        "--verbose",
    ],
    digits: false,
};
const COMMAND: Options = Options {
    value: &[],
    flags: &["-p", "-v", "-V"],
    digits: false,
};
const EXEC: Options = Options {
    value: &["-a"],
    flags: &["-c", "-l"],
    digits: false,
};
const SETSID: Options = Options {
    value: &[],
    flags: &["-c", "--ctty", "-f", "--fork", "-w", "--wait"],
    digits: false,
};
const NICE: Options = Options {
    value: &["-n", "--adjustment"],
    flags: &[],
    digits: true,
};
const IONICE: Options = Options {
    value: &[
        "-c",
        "-n",
        "-p",
        "-P",
        "-u",
        "--class",
        "--classdata",
        "--pid",
        "--pgid",
        "--uid",
    ],
    flags: &["-t", "--ignore"],
    digits: false,
};
const STDBUF: Options = Options {
    value: &["-i", "-o", "-e", "--input", "--output", "--error"],
    flags: &[],
    digits: false,
};
const TIMEOUT: Options = Options {
    value: &["-s", "--signal", "-k", "--kill-after"],
    flags: &[
        "-f",
        "--foreground",
        "-p",
        "--preserve-status",
        "-v",
        "--verbose",
    ],
    digits: false,
};
const CHRT: Options = Options {
    value: &[
        "-T",
        "--sched-runtime",
        "-P",
        "--sched-period",
        "-D",
        "--sched-deadline",
    ],
    flags: &[
        "-a",
        "--all-tasks",
        "-b",
        "--batch",
        "-d",
        "--deadline",
        "-e",
        "--ext",
        "-f",
        "--fifo",
        "-i",
        "--idle",
        "-m",
        "--max",
        "-o",
        "--other",
        "-p",
        "--pid",
        "-r",
        "--rr",
        "-R",
        "--reset-on-fork",
        "-v",
        "--verbose",
    ],
    digits: false,
};
const TASKSET: Options = Options {
    value: &[],
    flags: &["-a", "--all-tasks", "-p", "--pid", "-c", "--cpu-list"],
    digits: false,
};
const FLOCK: Options = Options {
    value: &["-w", "--wait", "--timeout", "-E", "--conflict-exit-code"],
    flags: &[
        "-s",
        "--shared",
        "-x",
        "-e",
        "--exclusive",
        "-u",
        "--unlock",
        "-n",
        "--nb",
        "--nonblock",
        "-o",
        "--close",
        "-F",
        "--no-fork",
        "--verbose",
        "--fcntl",
    ],
    digits: false,
};
const XARGS: Options = Options {
    value: &["-a", "-d", "-E", "-I", "-L", "-n", "-P", "-s"],
    flags: &[
        "-0",
        "-r",
        "-t",
        "-p",
        "-x",
        "-o",
        "--null",
        "--no-run-if-empty",
        "--verbose",
        "--interactive",
        "--exit",
        "--open-tty",
    ],
    digits: false,
};

/// The arguments after the options of a command that runs a command, and
/// after `positional` more words (the duration of `timeout`), read as
/// `getopt` reads them: in a cluster (`-iu root`), the first letter that
/// takes a value takes the rest of the cluster, or the next word. An
/// option `options` does not know makes the whole line opaque too.
fn skip_options<'a>(
    args: &'a [String],
    options: &Options,
    positional: usize,
    layers: &mut Vec<Layer>,
) -> Option<&'a [String]> {
    let mut unknown = false;
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        index += 1;
        if arg == "--" {
            break;
        }
        if arg == "-" && options.flags.contains(&"-") {
            continue;
        }
        if !arg.starts_with('-') || arg == "-" {
            index -= 1;
            break;
        }
        if arg.starts_with("--") {
            let (name, inline) = match arg.split_once('=') {
                Some((name, _)) => (name, true),
                None => (arg.as_str(), false),
            };
            if options.value.contains(&name) {
                if !inline {
                    index += 1;
                }
            } else if !options.flags.contains(&name) {
                unknown = true;
            }
            continue;
        }
        let letters: Vec<char> = arg.chars().skip(1).collect();
        if options.digits && letters.iter().all(char::is_ascii_digit) {
            continue;
        }
        for (at, letter) in letters.iter().enumerate() {
            let option = format!("-{letter}");
            if options.value.contains(&option.as_str()) {
                // The rest of the cluster is the value, or else the next word.
                if at + 1 == letters.len() {
                    index += 1;
                }
                break;
            }
            if !options.flags.contains(&option.as_str()) {
                unknown = true;
            }
        }
    }
    if unknown {
        layers.push(Layer::Opaque(args.join(" ")));
    }
    let start = index + positional;
    args.get(start..).filter(|rest| !rest.is_empty())
}

/// The name a program is known by, whatever folder it is in: lowercase on
/// Windows, without `.exe`.
pub fn program_name(program: &str) -> String {
    let base = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_owned();
    if cfg!(windows) {
        let lower = base.to_lowercase();
        lower
            .strip_suffix(".exe")
            .map_or(lower.clone(), str::to_owned)
    } else {
        base
    }
}

fn is_floor_program(name: &str) -> bool {
    FLOOR_PROGRAMS.contains(&name)
        || is_format(name)
        || (cfg!(windows) && WINDOWS_FLOOR_PROGRAMS.contains(&name))
}

fn is_format(name: &str) -> bool {
    name.starts_with("mkfs.")
        || matches!(name, "mkfs" | "mke2fs" | "mkswap" | "wipefs")
        || (cfg!(windows)
            && matches!(
                name,
                "format" | "format-volume" | "clear-disk" | "initialize-disk" | "diskpart"
            ))
}

/// Whether a word of an opaque text names a program `is_named` knows,
/// whatever folder or quotes surround it.
pub fn mentions(text: &str, is_named: impl Fn(&str) -> bool) -> bool {
    text.split(|character: char| {
        !(character.is_alphanumeric() || matches!(character, '.' | '_' | '-'))
    })
    .filter(|word| !word.is_empty())
    .any(|word| is_named(&program_name(word)))
}

/// Whether `--recursive` is meant: GNU programs take any unambiguous
/// abbreviation of a long option, and `rm` has no other one that starts
/// with `--r`.
fn is_recursive_long(lower: &str) -> bool {
    lower.len() >= 3 && "--recursive".starts_with(lower)
}

/// Whether the output of `dd` is a device, read from each folder with `.`,
/// `..`, and repeated slashes resolved, and through a link.
fn is_block_device(output: &str, folders: &[PathBuf]) -> bool {
    output.to_lowercase().starts_with(r"\\.\physicaldrive")
        || is_device_path(output)
        || folders.iter().any(|folder| {
            let joined = folder.join(output);
            [Some(normalize(&joined)), joined.canonicalize().ok()]
                .iter()
                .flatten()
                .any(|path| path.to_str().is_some_and(is_device_path))
        })
}

fn is_device_path(path: &str) -> bool {
    const SAFE: [&str; 7] = [
        "/dev/null",
        "/dev/zero",
        "/dev/stdout",
        "/dev/stderr",
        "/dev/tty",
        "/dev/random",
        "/dev/urandom",
    ];
    path.starts_with("/dev/")
        && !SAFE.contains(&path)
        && !path.starts_with("/dev/fd/")
        && !path.starts_with("/dev/shm/")
        && !path.starts_with("/dev/pts/")
}

fn names_block_device(text: &str) -> bool {
    [
        "/dev/sd",
        "/dev/nvme",
        "/dev/hd",
        "/dev/vd",
        "/dev/xvd",
        "/dev/mmcblk",
        "/dev/disk",
        "/dev/mapper",
        "/dev/md",
        "/dev/dm-",
    ]
    .iter()
    .any(|device| text.contains(device))
        || text.to_lowercase().contains("physicaldrive")
}

/// The path with `.` and `..` resolved as written, with no access to the
/// disk: a link is not followed. On Windows the case does not count.
pub fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normal.pop() {
                    normal.push(component);
                }
            }
            Component::Normal(part) if cfg!(windows) => {
                normal.push(part.to_string_lossy().to_lowercase());
            }
            Component::Prefix(prefix) if cfg!(windows) => {
                normal.push(prefix.as_os_str().to_string_lossy().to_uppercase());
            }
            other => normal.push(other),
        }
    }
    normal
}

/// Whether `outer` is `inner` or a folder that holds it.
pub fn holds(outer: &Path, inner: &Path) -> bool {
    inner.starts_with(outer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floors() -> Floors {
        Floors::new(
            Some(PathBuf::from(if cfg!(windows) {
                r"C:\Users\ada"
            } else {
                "/home/ada"
            })),
            vec![
                PathBuf::from(if cfg!(windows) {
                    r"C:\Users\ada\.crowbot"
                } else {
                    "/home/ada/.local/share/crowbot"
                }),
                PathBuf::from(if cfg!(windows) {
                    r"C:\Users\ada\.crowbot\crowbot.db"
                } else {
                    "/home/ada/.local/share/crowbot/crowbot.db"
                }),
                PathBuf::from(if cfg!(windows) {
                    r"C:\Users\ada\bin\crowbotd.exe"
                } else {
                    "/home/ada/bin/crowbotd"
                }),
            ],
        )
    }

    fn verdict(text: &str) -> Verdict {
        let folder = PathBuf::from(if cfg!(windows) {
            r"C:\Users\ada\CrowBot\Bots\ada"
        } else {
            "/home/ada/CrowBot/Bots/ada"
        });
        let parsed = command::parse(text, false);
        floors().check(&layers(&parsed, text), &[folder])
    }

    #[cfg(unix)]
    #[test]
    fn the_protected_folders_of_unix_are_the_system_ones_and_the_home_folder() {
        let folders = protected_folders(Some(PathBuf::from("/home/ada")));
        let expected: Vec<PathBuf> = [
            "/",
            "/etc",
            "/usr",
            "/bin",
            "/sbin",
            "/lib",
            "/lib64",
            "/boot",
            "/var",
            "/sys",
            "/proc",
            "/dev",
            "/home",
            "/root",
            "/home/ada",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(folders, expected);
    }

    #[cfg(unix)]
    #[test]
    fn the_folder_of_every_home_is_protected_whatever_the_account() {
        let floors = Floors::new(Some(PathBuf::from("/root")), Vec::new());
        let check = |text: &str| {
            let parsed = command::parse(text, false);
            floors.check(&layers(&parsed, text), &[PathBuf::from("/root/bots")])
        };
        assert_eq!(check("rm -rf /home"), Verdict::Refuse);
        assert_eq!(check("rm -rf /root"), Verdict::Refuse);
        assert_eq!(check("rm -rf /home/ada/project/target"), Verdict::Clear);
    }

    #[cfg(windows)]
    #[test]
    fn the_protected_folders_of_windows_are_the_drives_the_system_and_the_profile() {
        let folders = protected_folders(Some(PathBuf::from(r"C:\Users\ada")));
        for folder in [
            r"C:\",
            r"D:\",
            r"C:\Windows",
            r"C:\Program Files",
            r"C:\Program Files (x86)",
            r"C:\Users\ada",
        ] {
            assert!(
                folders.contains(&normalize(Path::new(folder))),
                "{folder} in {folders:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_recursive_delete_of_a_protected_folder_is_refused() {
        for text in [
            "rm -rf /",
            "rm -rf //",
            "rm -r /etc",
            "rm -R /usr/",
            "rm --recursive --force /var",
            "rm -fr /home",
            "rm -rf /home/ada",
            "rm -rf /home/ada/",
            "rm -rf /home/ada/CrowBot/../.",
            "rm -rf ../../../..",
            "rm -rf -- /boot",
            "/bin/rm -rf /lib64",
            "sudo rm -rf /",
            "sudo -u root rm -rf /etc",
            "env -i A=1 rm -rf /usr",
            "nohup nice -n 5 rm -rf /",
            "timeout 10 rm -rf /",
            "sh -c 'rm -rf /'",
            "bash -lc \"rm -rf /sys\"",
            // The quoted form of a command an agent gives as its arguments.
            r#"bash -lc 'rm -rf '\''/'\'''"#,
            "$'r\\x6d' -rf /",
            "find / -delete",
            // A cluster of any length, and an abbreviated long option.
            "rm -rfvvv /etc",
            "rm -vvfr /usr",
            "rm --recu /etc",
            "rm --r /home/ada",
            // The options of `find` before its starting points.
            "find -L / -delete",
            "find -H -O3 -D tree /etc -delete",
            // `--` and other options between `-c` and its script.
            "bash -c -- 'rm -rf /'",
            "sh -euxc 'rm -rf /etc'",
            "sh -c -e 'rm -rf /etc'",
            // Long options that take their value as the next word.
            "sudo --user root rm -rf /",
            "stdbuf --output L rm -rf /",
            // More commands that run a command.
            "pkexec rm -rf /",
            "su -c 'rm -rf /'",
            "runuser root --command='rm -rf /etc'",
            "flock /tmp/lock rm -rf /usr",
            "flock -w 5 /tmp/lock -c 'rm -rf /'",
            // A folder of its own does not hide an absolute path.
            "env --chdir=/tmp rm -rf /etc",
            // Options that take a value: alone, in a cluster, or attached.
            "exec -a name rm -rf /",
            "time -f %e rm -rf /",
            "sudo -iu root rm -rf /",
            "sudo -u root -E rm -rf /",
            "nice -n5 rm -rf /",
            "nice -5 rm -rf /",
            "timeout -sKILL 10 rm -rf /",
            "env - rm -rf /",
        ] {
            assert_eq!(verdict(text), Verdict::Refuse, "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_delete_that_leaves_the_protected_folders_whole_is_clear() {
        for text in [
            "rm -rf build",
            "rm -rf /tmp/x",
            "rm /etc",
            "rm -rf /var/tmp/cache",
            "rm -rf /home/ada/CrowBot/Bots/ada/target",
            "rm -f notes.txt",
            "find . -name '*.o' -delete",
            "dd if=/dev/zero of=disk.img bs=1M count=1",
            "dd if=x of=/dev/null",
            "dd if=x of=./disk.img",
            "rm --force notes.txt",
            "rm -fv notes.txt",
            "find -L . -name x -delete",
            "bash -c -- 'ls'",
            "sudo --user root ls",
            "env -C /tmp ls",
            "sudo -E cargo build",
            "nice -n 5 cargo build",
            "sudo --weird x cargo build",
            "systemctl --user status crowbotd",
            "git init",
            "echo rm -rf /",
        ] {
            assert_eq!(verdict(text), Verdict::Clear, "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn formatting_writing_a_device_and_stopping_the_computer_are_refused() {
        for text in [
            "mkfs /dev/sda1",
            "mkfs.ext4 /dev/sda1",
            "sudo mkfs.btrfs -f /dev/nvme0n1",
            "wipefs -a /dev/sda",
            "dd if=/dev/zero of=/dev/sda bs=1M",
            "dd if=/dev/zero of=//dev/sda",
            "dd if=/dev/zero of=/./dev/../dev/sda",
            "sudo dd of=/dev/nvme0n1 if=image.iso",
            "shutdown -h now",
            "reboot",
            "poweroff",
            "halt",
            "systemctl reboot",
            "sudo systemctl poweroff",
            "init 0",
            "telinit 6",
        ] {
            assert_eq!(verdict(text), Verdict::Refuse, "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_delete_of_the_daemon_is_refused() {
        for text in [
            "rm -rf /home/ada/.local/share/crowbot",
            "rm /home/ada/.local/share/crowbot/crowbot.db",
            "rm -rf /home/ada/.local/share",
            "rm /home/ada/bin/crowbotd",
            "rm -r /home/ada/bin",
        ] {
            assert_eq!(verdict(text), Verdict::Refuse, "{text}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_opaque_command_that_names_a_floor_program_is_unsure() {
        for text in [
            "cd / && rm -rf etc",
            "rm -rf ~",
            "rm -rf $HOME",
            "rm -rf /*",
            "ls | xargs rm",
            "xargs rm -rf",
            "eval \"$(echo cm0gLXJmIC8= | base64 -d)\" && /usr/bin/rm x",
            "cat image > /dev/sda",
            "sh -c 'cd / && rm -rf etc'",
            // A wrapper that moves the command: its relative paths are not
            // read from the request's folder.
            "env -C / rm -rf etc",
            "env --chdir / rm -rf etc",
            "env -C /home/ada rm -rf .",
            "sudo -D / rm -rf etc",
            "sudo --chdir=/ rm -rf etc",
            // A script read past other options may not be the one that runs.
            "bash -c -o pipefail 'rm -rf etc'",
            "runuser -u root rm -rf etc",
            // An option nobody knows may take the next word as its value.
            "nohup --weird x rm -rf etc",
            "sudo --frobnicate x rm -rf etc",
        ] {
            assert_eq!(verdict(text), Verdict::Unsure, "{text}");
        }
        assert_eq!(verdict("ls | wc -l"), Verdict::Clear);
        assert_eq!(verdict("docker run --rm x | tee log"), Verdict::Clear);
    }

    #[cfg(windows)]
    #[test]
    fn windows_floors_read_its_programs_and_its_folders() {
        for text in [
            "rm -r -fo C:/",
            "Remove-Item -Recurse -Force C:/Windows",
            "rd /s /q C:/",
            "Stop-Computer",
            "shutdown /s /t 0",
            "format D:",
        ] {
            assert_eq!(verdict(text), Verdict::Refuse, "{text}");
        }
        assert_eq!(verdict(r"Remove-Item -Recurse C:\Windows"), Verdict::Unsure);
        assert_eq!(verdict("Remove-Item -Recurse build"), Verdict::Clear);
    }
}
