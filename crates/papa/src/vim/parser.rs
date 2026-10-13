//! Turns keys into commands: `2d3w` is "delete six words".
//!
//! Vim's Normal mode is a small language. A command is an optional count,
//! then either a motion (`w`), an operator and a motion (`dw`), an operator
//! twice (`dd`, whole lines) or a command of its own (`x`, `p`, `i`). The
//! parser takes one key at a time and says whether a command is complete,
//! still being typed, or not a command at all.

use super::key::Key;
use super::motion::Motion;

/// Counts above this are taken as this: `99999999dd` shouldn't hang.
const MAX_COUNT: usize = 9_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operator {
    Delete,
    Change,
    Yank,
}

impl Operator {
    fn from_key(key: Key) -> Option<Self> {
        match key {
            Key::Char('d') => Some(Self::Delete),
            Key::Char('c') => Some(Self::Change),
            Key::Char('y') => Some(Self::Yank),
            _ => None,
        }
    }
}

/// What an operator acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// From the cursor to where the motion lands.
    Motion(Motion),
    /// Whole lines, from the cursor's down: `dd`, `cc`, `yy`.
    Lines,
    /// The Visual mode selection.
    Selection,
}

/// Where `i`, `a`, `I`, `A`, `o` and `O` start typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertAt {
    /// `i`: before the cursor.
    Cursor,
    /// `a`: after the cursor.
    AfterCursor,
    /// `I`: before the line's first non-blank.
    LineStart,
    /// `A`: at the end of the line.
    LineEnd,
    /// `o`: on a new line below.
    LineBelow,
    /// `O`: on a new line above.
    LineAbove,
}

/// A complete command. `count` is `None` when none was typed, which a few
/// commands (`G`) treat differently from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Move {
        motion: Motion,
        count: Option<usize>,
    },
    Operate {
        operator: Operator,
        target: Target,
        count: Option<usize>,
    },
    /// `x`
    DeleteChar {
        count: usize,
    },
    Insert(InsertAt),
    /// `p` puts after the cursor, `P` before it.
    Put {
        before: bool,
        count: usize,
    },
    /// `u`
    Undo {
        count: usize,
    },
    /// Ctrl-R
    Redo {
        count: usize,
    },
    /// `v`: start Visual mode, or end it.
    ToggleVisual,
    /// `:`
    StartCommandLine,
    /// Esc: cancel what's being typed, or leave Visual mode.
    Escape,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// More keys are needed: `d` waits for a motion.
    Pending,
    Done(Command),
    /// Not a command. What was typed so far is dropped, as in Vim.
    Invalid,
}

/// The keys typed so far for the next command.
#[derive(Clone, Debug, Default)]
pub struct Parser {
    /// The count being typed. Before an operator, it moves into `operator`.
    count: Option<usize>,
    /// An operator waiting for its motion, with the count typed before it.
    operator: Option<(Operator, Option<usize>)>,
    /// `g` was typed: the next key completes `gg`.
    g: bool,
    /// The keys, to show in the status bar while a command is incomplete.
    typed: String,
}

impl Parser {
    /// The keys of the incomplete command, such as `2d`.
    pub fn typed(&self) -> &str {
        &self.typed
    }

    /// Reads one key. `visual`: Visual mode, where operators act on the
    /// selection instead of waiting for a motion.
    pub fn feed(&mut self, key: Key, visual: bool) -> Parsed {
        let parsed = self.step(key, visual);
        match parsed {
            Parsed::Pending => key.write(&mut self.typed),
            Parsed::Done(_) | Parsed::Invalid => *self = Self::default(),
        }
        parsed
    }

    fn step(&mut self, key: Key, visual: bool) -> Parsed {
        if std::mem::take(&mut self.g) {
            return match key {
                Key::Char('g') => self.motion(Motion::FirstLine),
                _ => Parsed::Invalid,
            };
        }
        match key {
            Key::Char(digit @ '1'..='9') => self.digit(digit),
            // `0` is a motion, unless it continues a count: `10j`.
            Key::Char('0') if self.count.is_some() => self.digit('0'),
            Key::Char('g') => {
                self.g = true;
                Parsed::Pending
            }
            Key::Escape => Parsed::Done(Command::Escape),
            key => match motion(key) {
                Some(motion) => self.motion(motion),
                None => self.command(key, visual),
            },
        }
    }

    fn digit(&mut self, digit: char) -> Parsed {
        let digit = digit.to_digit(10).unwrap_or(0) as usize;
        let count = self.count.unwrap_or(0);
        self.count = Some((count * 10 + digit).min(MAX_COUNT));
        Parsed::Pending
    }

    /// A motion completes a move, or the operator waiting for it.
    fn motion(&mut self, motion: Motion) -> Parsed {
        let count = self.count.take();
        Parsed::Done(match self.operator.take() {
            None => Command::Move { motion, count },
            Some((operator, first)) => Command::Operate {
                operator,
                target: Target::Motion(motion),
                count: multiply(first, count),
            },
        })
    }

    fn command(&mut self, key: Key, visual: bool) -> Parsed {
        let count = self.count.take();
        let times = count.unwrap_or(1);
        if let Some((operator, first)) = self.operator.take() {
            // Only the same operator again: `dd`, `cc`, `yy`.
            return match Operator::from_key(key) {
                Some(again) if again == operator => Parsed::Done(Command::Operate {
                    operator,
                    target: Target::Lines,
                    count: multiply(first, count),
                }),
                _ => Parsed::Invalid,
            };
        }
        if visual {
            let operator = match key {
                Key::Char('x') => Some(Operator::Delete),
                key => Operator::from_key(key),
            };
            return match (operator, key) {
                (Some(operator), _) => Parsed::Done(Command::Operate {
                    operator,
                    target: Target::Selection,
                    count: None,
                }),
                (None, Key::Char('v')) => Parsed::Done(Command::ToggleVisual),
                _ => Parsed::Invalid,
            };
        }
        if let Some(operator) = Operator::from_key(key) {
            self.operator = Some((operator, count));
            return Parsed::Pending;
        }
        let command = match key {
            Key::Char('x') => Command::DeleteChar { count: times },
            Key::Char('i') => Command::Insert(InsertAt::Cursor),
            Key::Char('a') => Command::Insert(InsertAt::AfterCursor),
            Key::Char('I') => Command::Insert(InsertAt::LineStart),
            Key::Char('A') => Command::Insert(InsertAt::LineEnd),
            Key::Char('o') => Command::Insert(InsertAt::LineBelow),
            Key::Char('O') => Command::Insert(InsertAt::LineAbove),
            Key::Char('p') => Command::Put {
                before: false,
                count: times,
            },
            Key::Char('P') => Command::Put {
                before: true,
                count: times,
            },
            Key::Char('u') => Command::Undo { count: times },
            Key::Ctrl('r') => Command::Redo { count: times },
            Key::Char('v') => Command::ToggleVisual,
            Key::Char(':') => Command::StartCommandLine,
            _ => return Parsed::Invalid,
        };
        Parsed::Done(command)
    }
}

/// The motion a key stands for on its own, if any. `0` and `gg` are
/// handled before this: they depend on what came before them.
fn motion(key: Key) -> Option<Motion> {
    Some(match key {
        Key::Char('h') | Key::Left | Key::Backspace => Motion::Left,
        Key::Char('l') | Key::Right => Motion::Right,
        Key::Char('j') | Key::Down => Motion::Down,
        Key::Char('k') | Key::Up => Motion::Up,
        Key::Char('w') => Motion::WordForward,
        Key::Char('b') => Motion::WordBackward,
        Key::Char('e') => Motion::WordEnd,
        Key::Char('0') => Motion::LineStart,
        Key::Char('^') => Motion::FirstNonBlank,
        Key::Char('$') => Motion::LineEnd,
        Key::Char('G') => Motion::LastLine,
        Key::Enter => Motion::NextLine,
        _ => return None,
    })
}

/// `2d3w` deletes six words. No count at all stays `None`.
fn multiply(first: Option<usize>, second: Option<usize>) -> Option<usize> {
    match (first, second) {
        (None, None) => None,
        (first, second) => Some((first.unwrap_or(1) * second.unwrap_or(1)).min(MAX_COUNT)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vim::key::keys;

    /// Feeds `typed` in Normal mode and returns what the last key gave.
    fn parse(typed: &str) -> Parsed {
        parse_in(typed, false)
    }

    fn parse_in(typed: &str, visual: bool) -> Parsed {
        let mut parser = Parser::default();
        let mut last = Parsed::Pending;
        for key in keys(typed) {
            last = parser.feed(key, visual);
        }
        last
    }

    fn done(command: Command) -> Parsed {
        Parsed::Done(command)
    }

    fn moves(motion: Motion, count: Option<usize>) -> Parsed {
        done(Command::Move { motion, count })
    }

    fn operate(operator: Operator, target: Target, count: Option<usize>) -> Parsed {
        done(Command::Operate {
            operator,
            target,
            count,
        })
    }

    #[test]
    fn a_motion_alone_moves() {
        assert_eq!(parse("w"), moves(Motion::WordForward, None));
        assert_eq!(parse("<Down>"), moves(Motion::Down, None));
        assert_eq!(parse("0"), moves(Motion::LineStart, None));
        assert_eq!(parse("gg"), moves(Motion::FirstLine, None));
    }

    #[test]
    fn counts() {
        assert_eq!(parse("3j"), moves(Motion::Down, Some(3)));
        assert_eq!(
            parse("10j"),
            moves(Motion::Down, Some(10)),
            "0 continues a count"
        );
        assert_eq!(parse("12G"), moves(Motion::LastLine, Some(12)));
        assert_eq!(parse("5gg"), moves(Motion::FirstLine, Some(5)));
        assert_eq!(parse("99999999j"), moves(Motion::Down, Some(MAX_COUNT)));
        assert_eq!(parse("3x"), done(Command::DeleteChar { count: 3 }));
    }

    #[test]
    fn an_operator_waits_for_its_motion() {
        assert_eq!(parse("d"), Parsed::Pending);
        assert_eq!(
            parse("dw"),
            operate(Operator::Delete, Target::Motion(Motion::WordForward), None)
        );
        assert_eq!(
            parse("c$"),
            operate(Operator::Change, Target::Motion(Motion::LineEnd), None)
        );
        assert_eq!(
            parse("ygg"),
            operate(Operator::Yank, Target::Motion(Motion::FirstLine), None)
        );
    }

    #[test]
    fn counts_before_and_after_an_operator_multiply() {
        let six_words = operate(
            Operator::Delete,
            Target::Motion(Motion::WordForward),
            Some(6),
        );
        assert_eq!(parse("2d3w"), six_words);
        assert_eq!(parse("6dw"), six_words);
        assert_eq!(parse("d6w"), six_words);
        assert_eq!(
            parse("3dd"),
            operate(Operator::Delete, Target::Lines, Some(3))
        );
    }

    #[test]
    fn an_operator_twice_means_whole_lines() {
        assert_eq!(parse("dd"), operate(Operator::Delete, Target::Lines, None));
        assert_eq!(parse("cc"), operate(Operator::Change, Target::Lines, None));
        assert_eq!(parse("yy"), operate(Operator::Yank, Target::Lines, None));
        assert_eq!(parse("dy"), Parsed::Invalid, "two different operators");
        assert_eq!(parse("dx"), Parsed::Invalid);
    }

    #[test]
    fn commands_of_their_own() {
        assert_eq!(parse("A"), done(Command::Insert(InsertAt::LineEnd)));
        assert_eq!(parse("O"), done(Command::Insert(InsertAt::LineAbove)));
        assert_eq!(
            parse("2P"),
            done(Command::Put {
                before: true,
                count: 2
            })
        );
        assert_eq!(parse("u"), done(Command::Undo { count: 1 }));
        assert_eq!(parse("<C-r>"), done(Command::Redo { count: 1 }));
        assert_eq!(parse(":"), done(Command::StartCommandLine));
        assert_eq!(parse("v"), done(Command::ToggleVisual));
    }

    #[test]
    fn escape_cancels_whatever_was_typed() {
        assert_eq!(parse("2d<Esc>"), done(Command::Escape));
        assert_eq!(parse("g<Esc>"), Parsed::Invalid, "g only completes gg");
    }

    #[test]
    fn an_unknown_key_drops_the_command() {
        assert_eq!(parse("q"), Parsed::Invalid);
        let mut parser = Parser::default();
        for key in keys("2dq") {
            parser.feed(key, false);
        }
        assert_eq!(parser.typed(), "", "nothing left over");
        assert_eq!(
            parser.feed(Key::Char('w'), false),
            moves(Motion::WordForward, None)
        );
    }

    #[test]
    fn the_keys_typed_so_far_are_shown() {
        let mut parser = Parser::default();
        for key in keys("2d3") {
            parser.feed(key, false);
        }
        assert_eq!(parser.typed(), "2d3");
    }

    #[test]
    fn in_visual_mode_operators_act_at_once() {
        assert_eq!(
            parse_in("d", true),
            operate(Operator::Delete, Target::Selection, None)
        );
        assert_eq!(
            parse_in("x", true),
            operate(Operator::Delete, Target::Selection, None)
        );
        assert_eq!(
            parse_in("y", true),
            operate(Operator::Yank, Target::Selection, None)
        );
        assert_eq!(parse_in("2w", true), moves(Motion::WordForward, Some(2)));
        assert_eq!(parse_in("v", true), done(Command::ToggleVisual));
        assert_eq!(parse_in("i", true), Parsed::Invalid);
    }
}
