//! The target of a command request, reduced to one simple command or marked
//! opaque (decision D3 of `tasks/prd-crow-bot-harness.md`).
//!
//! A rule matches structure, never text: the parser decodes what a shell
//! would decode in a word (quotes, ANSI-C quoting, escapes, line
//! continuations) and gives up on everything a shell would compute or
//! combine (pipes, lists, redirections, substitutions, expansions, globs,
//! subshells, here-documents, `eval`). Giving up is the safe answer: an
//! opaque command matches no allow rule.

/// A command as the daemon can match it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    Simple(Simple),
    /// The text holds more than one simple command, or something a shell
    /// computes before it runs one, or it was cut.
    Opaque,
}

/// One program and its arguments, as the shell passes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Simple {
    pub program: String,
    pub args: Vec<String>,
}

/// The rules of the shell a command is read with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// A POSIX shell, with the decoding Bash adds (`$'...'`).
    Posix,
    /// PowerShell and `cmd.exe`, where a backslash is a plain character.
    Windows,
}

/// Words a POSIX shell reads as syntax at the start of a command.
const KEYWORDS: [&str; 20] = [
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "select", "coproc", "[[", "]]", "{", "}", "eval",
];

/// The command a request runs, as the daemon can match it. A target that
/// was cut is opaque, whatever it holds.
pub fn parse(text: &str, cut: bool) -> Parsed {
    if cut {
        return Parsed::Opaque;
    }
    parse_native(text)
}

/// The command read with the shells of this system. On Windows an agent
/// may run it in Git Bash or in PowerShell, so it is simple only when both
/// read the same words.
pub fn parse_native(text: &str) -> Parsed {
    if cfg!(windows) {
        let posix = parse_with(text, Dialect::Posix);
        if posix == parse_with(text, Dialect::Windows) {
            posix
        } else {
            Parsed::Opaque
        }
    } else {
        parse_with(text, Dialect::Posix)
    }
}

pub fn parse_with(text: &str, dialect: Dialect) -> Parsed {
    let words = match dialect {
        Dialect::Posix => posix_words(text),
        Dialect::Windows => windows_words(text),
    };
    let Some(mut words) = words else {
        return Parsed::Opaque;
    };
    if words.is_empty() {
        return Parsed::Opaque;
    }
    let program = words.remove(0);
    if program.is_empty()
        || program.contains('\0')
        || KEYWORDS.contains(&program.as_str())
        || words.iter().any(|word| word.contains('\0'))
    {
        return Parsed::Opaque;
    }
    Parsed::Simple(Simple {
        program,
        args: words,
    })
}

/// One word being read, and whether a shell would assign or expand it.
#[derive(Default)]
struct Word {
    text: String,
    /// A character was read since the word started, quoted or not.
    started: bool,
    /// The word so far is a name made of plain characters, so an `=` makes
    /// it an assignment.
    plain_name: bool,
}

impl Word {
    fn push(&mut self, character: char, quoted: bool) {
        if !self.started {
            self.plain_name = !quoted && (character.is_ascii_alphabetic() || character == '_');
        } else if quoted || !(character.is_ascii_alphanumeric() || character == '_') {
            self.plain_name = false;
        }
        self.started = true;
        self.text.push(character);
    }
}

/// The words of a POSIX command, or `None` when it is not one simple command.
fn posix_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = Word::default();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            ' ' | '\t' => {
                if word.started {
                    words.push(std::mem::take(&mut word).text);
                }
            }
            '\n' | '\r' => {
                // A list of commands, unless nothing follows.
                if chars.clone().any(|rest| !rest.is_whitespace()) {
                    return None;
                }
                if word.started {
                    words.push(std::mem::take(&mut word).text);
                }
            }
            '\\' => match chars.next() {
                // A line continuation joins the two lines.
                Some('\n') => {}
                Some(escaped) => word.push(escaped, true),
                None => return None,
            },
            '\'' => {
                word.started = true;
                word.plain_name = false;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(quoted) => word.push(quoted, true),
                        None => return None,
                    }
                }
            }
            '"' => {
                word.started = true;
                word.plain_name = false;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        // Expansions happen inside double quotes too.
                        Some('$' | '`') | None => return None,
                        Some('\\') => match chars.next() {
                            Some('\n') => {}
                            Some(escaped @ ('$' | '`' | '"' | '\\')) => word.push(escaped, true),
                            Some(other) => {
                                word.push('\\', true);
                                word.push(other, true);
                            }
                            None => return None,
                        },
                        Some(quoted) => word.push(quoted, true),
                    }
                }
            }
            '$' => {
                // Only ANSI-C quoting gives a fixed text; every other `$`
                // expands something.
                chars.next_if_eq(&'\'')?;
                word.started = true;
                word.plain_name = false;
                ansi_c(&mut chars, &mut word)?;
            }
            '|' | '&' | ';' | '<' | '>' | '(' | ')' | '`' | '*' | '?' | '[' | '{' | '}' | '!' => {
                return None;
            }
            '~' | '#' if !word.started => return None,
            '=' if word.plain_name && words.is_empty() => return None,
            other => word.push(other, false),
        }
    }
    if word.started {
        words.push(word.text);
    }
    Some(words)
}

/// Decodes `$'...'` after its opening quote, as Bash does.
fn ansi_c(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, word: &mut Word) -> Option<()> {
    loop {
        match chars.next()? {
            '\'' => return Some(()),
            '\\' => {
                let escaped = chars.next()?;
                let decoded = match escaped {
                    'a' => '\u{7}',
                    'b' => '\u{8}',
                    'e' | 'E' => '\u{1b}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'v' => '\u{b}',
                    '\\' | '\'' | '"' | '?' => escaped,
                    'c' => {
                        let control = chars.next()?;
                        char::from(u8::try_from(u32::from(control) & 0x1f).ok()?)
                    }
                    '0'..='7' => {
                        let mut value = escaped.to_digit(8)?;
                        for _ in 0..2 {
                            match chars.peek().and_then(|digit| digit.to_digit(8)) {
                                Some(digit) => {
                                    value = value * 8 + digit;
                                    chars.next();
                                }
                                None => break,
                            }
                        }
                        char::from(u8::try_from(value & 0xff).ok()?)
                    }
                    'x' => hex(chars, 2)?,
                    'u' => hex(chars, 4)?,
                    'U' => hex(chars, 8)?,
                    other => {
                        word.push('\\', true);
                        other
                    }
                };
                word.push(decoded, true);
            }
            other => word.push(other, true),
        }
    }
}

/// Up to `digits` hexadecimal digits, as one character.
fn hex(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, digits: usize) -> Option<char> {
    let mut value = 0_u32;
    let mut read = 0;
    while read < digits {
        match chars.peek().and_then(|digit| digit.to_digit(16)) {
            Some(digit) => {
                value = value.checked_mul(16)?.checked_add(digit)?;
                chars.next();
                read += 1;
            }
            None => break,
        }
    }
    if read == 0 {
        return None;
    }
    char::from_u32(value)
}

/// The words of a PowerShell or `cmd.exe` command, or `None` when it is not
/// one simple command. Only quotes are decoded; every character either
/// shell computes with is opaque.
fn windows_words(text: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = Word::default();
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        match character {
            ' ' | '\t' => {
                if word.started {
                    words.push(std::mem::take(&mut word).text);
                }
            }
            '\n' | '\r' => {
                if chars.clone().any(|rest| !rest.is_whitespace()) {
                    return None;
                }
                if word.started {
                    words.push(std::mem::take(&mut word).text);
                }
            }
            quote @ ('\'' | '"') => {
                word.started = true;
                loop {
                    match chars.next() {
                        Some(end) if end == quote => break,
                        Some('$' | '`' | '%' | '^') | None => return None,
                        Some(quoted) => word.push(quoted, true),
                    }
                }
            }
            '|' | '&' | ';' | '<' | '>' | '(' | ')' | '`' | '^' | '%' | '$' | '@' | '{' | '}'
            | '*' | '?' | '[' | '!' => return None,
            '~' | '#' if !word.started => return None,
            '=' if word.plain_name && words.is_empty() => return None,
            other => word.push(other, false),
        }
    }
    if word.started {
        words.push(word.text);
    }
    Some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple(program: &str, args: &[&str]) -> Parsed {
        Parsed::Simple(Simple {
            program: program.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        })
    }

    fn posix(text: &str) -> Parsed {
        parse_with(text, Dialect::Posix)
    }

    #[test]
    fn a_plain_command_is_a_program_and_its_arguments() {
        assert_eq!(
            posix("cargo test --locked"),
            simple("cargo", &["test", "--locked"])
        );
        assert_eq!(posix("  git   status  "), simple("git", &["status"]));
        assert_eq!(posix("ls\n"), simple("ls", &[]));
        assert_eq!(
            posix(r#"git commit -m "a b" -m 'c d'"#),
            simple("git", &["commit", "-m", "a b", "-m", "c d"])
        );
        assert_eq!(posix("echo ''"), simple("echo", &[""]));
        assert_eq!(
            posix("cargo test -- --nocapture a=b"),
            simple("cargo", &["test", "--", "--nocapture", "a=b"])
        );
        assert_eq!(posix(r#"printf "\n""#), simple("printf", &["\\n"]));
    }

    #[test]
    fn what_a_shell_combines_or_computes_is_opaque() {
        for text in [
            "ls | wc -l",
            "make && make install",
            "make || true",
            "cd x; ls",
            "sleep 1 &",
            "ls\nrm -rf x",
            "echo hi > out.txt",
            "sort < in.txt",
            "cat 2>&1 x",
            "ls &> log",
            "cat <<EOF\nhi\nEOF",
            "cat <<< hi",
            "echo $(whoami)",
            "echo `whoami`",
            "echo \"$(whoami)\"",
            "echo $HOME",
            "echo ${HOME}",
            "echo \"$HOME\"",
            "diff <(ls a) <(ls b)",
            "(cd x && ls)",
            "{ ls; }",
            "eval rm -rf /",
            "eval",
            "rm -rf *",
            "ls ?.txt",
            "ls [ab].txt",
            "echo {a,b}",
            "rm -rf ~",
            "cat ~/notes",
            "FOO=bar cargo test",
            "PATH=/tmp:$PATH ls",
            "if true; then ls; fi",
            "for x in a; do ls; done",
            "! ls",
            "ls # comment",
            "echo $\"hi\"",
            "echo 'open",
            "echo \"open",
            "echo $'open",
            "ls \\",
            "",
            "   ",
            "''",
        ] {
            assert_eq!(posix(text), Parsed::Opaque, "{text:?}");
        }
    }

    #[test]
    fn an_assignment_is_only_a_name_before_an_equals_sign() {
        assert_eq!(posix("a=b"), Parsed::Opaque);
        assert_eq!(posix("'a'=b"), simple("a=b", &[]));
        assert_eq!(posix("./a=b"), simple("./a=b", &[]));
        assert_eq!(posix("env A=1 ls"), simple("env", &["A=1", "ls"]));
    }

    #[test]
    fn the_program_is_the_name_a_shell_decodes() {
        assert_eq!(posix("$'r\\x6d' -rf /"), simple("rm", &["-rf", "/"]));
        assert_eq!(posix("$'\\162\\155' x"), simple("rm", &["x"]));
        assert_eq!(posix("$'\\u0072m' x"), simple("rm", &["x"]));
        assert_eq!(posix("$'rm\\t'"), simple("rm\t", &[]));
        assert_eq!(posix("r\\m -rf /"), simple("rm", &["-rf", "/"]));
        assert_eq!(posix("r\\\nm -rf /"), simple("rm", &["-rf", "/"]));
        assert_eq!(posix("rm \\\n  -rf /"), simple("rm", &["-rf", "/"]));
        assert_eq!(posix("\"r\"'m' -rf /"), simple("rm", &["-rf", "/"]));
        assert_eq!(posix("\"r\\m\""), simple("r\\m", &[]));
        assert_eq!(posix("\\~ x"), simple("~", &["x"]));
        assert_eq!(posix("echo \\*"), simple("echo", &["*"]));
        // A NUL ends a word in a shell: such a name is not the one it reads.
        assert_eq!(posix("$'rm\\0x'"), Parsed::Opaque);
    }

    #[test]
    fn a_cut_target_is_opaque() {
        let long = format!(
            "echo {}",
            "x".repeat(crowbot_protocol::APPROVAL_TARGET_BYTES)
        );
        assert_eq!(parse(&long, true), Parsed::Opaque);
        assert_eq!(parse("cargo test", true), Parsed::Opaque);
        assert_eq!(parse("cargo test", false), simple("cargo", &["test"]));
    }

    #[test]
    fn windows_reads_a_backslash_as_a_character() {
        let windows = |text| parse_with(text, Dialect::Windows);
        assert_eq!(
            windows(r"Remove-Item -Recurse C:\Windows"),
            simple("Remove-Item", &["-Recurse", r"C:\Windows"])
        );
        assert_eq!(
            windows(r#"dir "C:\Program Files""#),
            simple("dir", &[r"C:\Program Files"])
        );
        for text in [
            "dir | more",
            "a; b",
            "echo $env:USERPROFILE",
            "echo %USERPROFILE%",
            "echo `n",
            "echo ^& x",
            "Get-ChildItem *.txt",
            "& { rm x }",
            "@(1)",
        ] {
            assert_eq!(windows(text), Parsed::Opaque, "{text:?}");
        }
        // A command both shells read the same way is simple for both.
        assert_eq!(
            parse_with("cargo test", Dialect::Posix),
            windows("cargo test")
        );
        assert_ne!(parse_with(r"rm C:\x", Dialect::Posix), windows(r"rm C:\x"));
    }

    /// A generator of byte strings, so the property test needs no crate.
    struct Bytes(u64);

    impl Bytes {
        fn next(&mut self) -> u64 {
            // xorshift64*
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn string(&mut self) -> Vec<u8> {
            const SHELL: &[u8] = b" \t\n\\'\"$`|&;<>(){}[]*?!~#=-_/.:x0aUuxc7r\0";
            let length = usize::try_from(self.next() % 48).unwrap_or(0);
            (0..length)
                .map(|_| {
                    let pick = self.next();
                    if pick.is_multiple_of(3) {
                        pick.to_le_bytes()[1]
                    } else {
                        SHELL[usize::try_from(pick >> 8).unwrap_or(0) % SHELL.len()]
                    }
                })
                .collect()
        }
    }

    #[test]
    fn any_byte_string_is_parsed_without_a_panic() {
        let mut bytes = Bytes(0x9e37_79b9_7f4a_7c15);
        for _ in 0..200_000 {
            let raw = bytes.string();
            let text = String::from_utf8_lossy(&raw);
            for dialect in [Dialect::Posix, Dialect::Windows] {
                if let Parsed::Simple(simple) = parse_with(&text, dialect) {
                    assert!(!simple.program.is_empty());
                }
            }
            let _ = parse(&text, false);
        }
        // Every truncation of a command with each kind of quoting.
        let quoted = "$'r\\x6d\\u0041\\U0001F600\\cA\\101' \"a\\\"b\" 'c' d\\\ne";
        for end in 0..=quoted.len() {
            if quoted.is_char_boundary(end) {
                let _ = posix(&quoted[..end]);
            }
        }
    }
}
