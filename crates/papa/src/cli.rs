//! Papa's command line. There's very little of it, so it's parsed by hand
//! rather than with a crate.

/// What `papa --help` prints.
pub const USAGE: &str = "\
Papa: a private, local-first journal for fathers.

Usage: papa [--demo]

Options:
  --demo         Open a demo vault with an invented family, made fresh
                 each time. Your own vault and settings aren't touched.
  -h, --help     Print this help.
  -V, --version  Print the version.";

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Open the app: the demo vault, or the usual one.
    Run {
        demo: bool,
    },
    Help,
    Version,
}

/// Reads the arguments after the program's name. A mistake comes back as
/// the message to print.
pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut demo = false;
    for arg in args {
        match arg.as_str() {
            "--demo" => demo = true,
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            // Older macOS versions pass a process serial number to apps
            // opened from the Finder.
            _ if arg.starts_with("-psn_") => {}
            _ => return Err(format!("unexpected argument {arg:?}")),
        }
    }
    Ok(Command::Run { demo })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Command, String> {
        super::parse(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn opens_the_vault_without_arguments() {
        assert_eq!(parse(&[]), Ok(Command::Run { demo: false }));
        assert_eq!(parse(&["-psn_0_12345"]), Ok(Command::Run { demo: false }));
    }

    #[test]
    fn opens_the_demo() {
        assert_eq!(parse(&["--demo"]), Ok(Command::Run { demo: true }));
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(parse(&["--demo", "--help"]), Ok(Command::Help));
        assert_eq!(parse(&["-V"]), Ok(Command::Version));
    }

    #[test]
    fn refuses_anything_else() {
        assert_eq!(
            parse(&["--demo", "notes.db"]),
            Err(r#"unexpected argument "notes.db""#.into())
        );
        assert!(parse(&["-demo"]).is_err());
    }
}
