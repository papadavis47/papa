//! Motions: where a key moves the cursor. Pure functions of the text and
//! the cursor, so each one is tested on a plain string.

use super::text::{CharClass, Text};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// `h`, ←, Backspace
    Left,
    /// `l`, →
    Right,
    /// `j`, ↓
    Down,
    /// `k`, ↑
    Up,
    /// `w`: the start of the next word.
    WordForward,
    /// `b`: the start of this word, or of the previous one.
    WordBackward,
    /// `e`: the end of this word, or of the next one.
    WordEnd,
    /// `0`
    LineStart,
    /// `^`: the first character that isn't blank.
    FirstNonBlank,
    /// `$`
    LineEnd,
    /// Enter: the first non-blank of the next line.
    NextLine,
    /// `gg`: the first line, or line *count*.
    FirstLine,
    /// `G`: the last line, or line *count*.
    LastLine,
}

/// What an operator like `d` does with the text between the cursor and
/// where a motion lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Up to the target, without it: `dw` keeps the next word's first
    /// letter.
    Exclusive,
    /// Up to and including the target: `de` deletes the word's last letter.
    Inclusive,
    /// Whole lines, from the cursor's to the target's: `dj` deletes two.
    Linewise,
}

impl Motion {
    pub fn kind(self) -> Kind {
        match self {
            Self::Left
            | Self::Right
            | Self::WordForward
            | Self::WordBackward
            | Self::LineStart
            | Self::FirstNonBlank => Kind::Exclusive,
            Self::WordEnd | Self::LineEnd => Kind::Inclusive,
            Self::Down | Self::Up | Self::NextLine | Self::FirstLine | Self::LastLine => {
                Kind::Linewise
            }
        }
    }

    /// Moves up or down a line, keeping the column.
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Down | Self::Up)
    }

    /// Where the motion lands from `from`. `count` is the number typed
    /// before it, if any: most motions repeat that many times, while `gg`
    /// and `G` take it as a line number. `column` is where `j` and `k`
    /// aim: Vim remembers it, so moving through a short line and back
    /// returns to the column you started in.
    ///
    /// The target may be past the end of a line, or of the text; Normal
    /// mode moves the cursor back onto a character, but an operator uses
    /// it as it is.
    pub fn target(self, text: &Text, from: usize, count: Option<usize>, column: usize) -> usize {
        let times = count.unwrap_or(1);
        match self {
            Self::Left => from.saturating_sub(times).max(text.line_start(from)),
            Self::Right => (from + times).min(text.line_end(from)),
            Self::Down | Self::Up => {
                let line = text.line_of(from);
                let line = if self == Self::Down {
                    line + times
                } else {
                    line.saturating_sub(times)
                };
                at_column(text, text.line(line), column)
            }
            Self::WordForward => repeat(times, from, |ix| word_forward(text, ix)),
            Self::WordBackward => repeat(times, from, |ix| word_backward(text, ix)),
            Self::WordEnd => repeat(times, from, |ix| word_end(text, ix)),
            Self::LineStart => text.line_start(from),
            Self::FirstNonBlank => text.first_non_blank(from),
            Self::LineEnd => {
                let line = text.line(text.line_of(from) + times - 1);
                text.last_char(line)
            }
            Self::NextLine => {
                let line = text.line(text.line_of(from) + times);
                text.first_non_blank(line)
            }
            Self::FirstLine => text.first_non_blank(text.line(times - 1)),
            Self::LastLine => {
                let line = count.unwrap_or(text.line_count()) - 1;
                text.first_non_blank(text.line(line))
            }
        }
    }
}

fn repeat(times: usize, from: usize, step: impl Fn(usize) -> usize) -> usize {
    (0..times).fold(from, |ix, _| step(ix))
}

/// Column `column` of the line starting at `start`, or its last character
/// if it's shorter. `usize::MAX` is "the end of every line", as after `$`.
fn at_column(text: &Text, start: usize, column: usize) -> usize {
    (start.saturating_add(column)).min(text.last_char(start))
}

fn class(text: &Text, ix: usize) -> CharClass {
    text.char(ix).map_or(CharClass::Blank, CharClass::of)
}

/// `w`: past the rest of this word, then past blanks, to the next word.
/// An empty line counts as a word, so `w` stops on it.
fn word_forward(text: &Text, from: usize) -> usize {
    let len = text.len();
    let mut ix = from;
    let start = class(text, ix);
    if start != CharClass::Blank {
        while ix < len && class(text, ix) == start {
            ix += 1;
        }
    }
    while ix < len && class(text, ix) == CharClass::Blank {
        if ix != from && text.is_empty_line(ix) {
            break;
        }
        ix += 1;
    }
    ix
}

/// `b`: back over blanks, then to the start of the word there.
fn word_backward(text: &Text, from: usize) -> usize {
    let mut ix = from;
    // Step off the current character, then back over blanks.
    while ix > 0 {
        ix -= 1;
        if class(text, ix) != CharClass::Blank || text.is_empty_line(ix) {
            break;
        }
    }
    if text.is_empty_line(ix) {
        return ix;
    }
    let word = class(text, ix);
    while ix > 0 && class(text, ix - 1) == word {
        ix -= 1;
    }
    ix
}

/// `e`: forward at least one character, over blanks (empty lines too),
/// then to the last character of the word there.
fn word_end(text: &Text, from: usize) -> usize {
    let len = text.len();
    let mut ix = from + 1;
    while ix < len && class(text, ix) == CharClass::Blank {
        ix += 1;
    }
    if ix >= len {
        return len.saturating_sub(1).max(from);
    }
    let word = class(text, ix);
    while ix + 1 < len && class(text, ix + 1) == word {
        ix += 1;
    }
    ix
}

/// The end of the word under `from`, for `cw`: Vim changes to the end of
/// the word rather than up to the next one.
pub fn end_of_word(text: &Text, from: usize) -> usize {
    let word = class(text, from);
    let mut ix = from;
    while ix + 1 < text.len() && class(text, ix + 1) == word {
        ix += 1;
    }
    ix
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where `motion` takes the cursor, written as `|` in `text`.
    fn moves(text: &str, motion: Motion, count: Option<usize>) -> String {
        let from = text.find('|').expect("a cursor");
        let plain = text.replace('|', "");
        let snapshot = Text::new(&plain);
        let from = snapshot.ix(from);
        let column = snapshot.column(from);
        let to = snapshot.byte(motion.target(&snapshot, from, count, column));
        format!("{}|{}", &plain[..to], &plain[to..])
    }

    #[test]
    fn left_and_right_stay_on_the_line() {
        assert_eq!(moves("ab\nc|d", Motion::Left, None), "ab\n|cd");
        assert_eq!(moves("ab\n|cd", Motion::Left, None), "ab\n|cd");
        assert_eq!(moves("a|bc\nd", Motion::Right, Some(5)), "abc|\nd");
    }

    #[test]
    fn up_and_down_keep_the_column() {
        assert_eq!(
            moves("abc|d\nef\nghij", Motion::Down, None),
            "abcd\ne|f\nghij"
        );
        assert_eq!(
            moves("ab|cd\nef\nghij", Motion::Down, Some(2)),
            "abcd\nef\ngh|ij"
        );
        assert_eq!(
            moves("abcd\nef\ng|hij", Motion::Up, Some(9)),
            "a|bcd\nef\nghij"
        );
        assert_eq!(moves("ab|cd\n\nx", Motion::Down, None), "abcd\n|\nx");
    }

    #[test]
    fn words_forward() {
        assert_eq!(moves("|one two", Motion::WordForward, None), "one |two");
        assert_eq!(moves("|don't", Motion::WordForward, None), "don|'t");
        assert_eq!(moves("|don't", Motion::WordForward, Some(2)), "don'|t");
        assert_eq!(
            moves("o|ne\n  two", Motion::WordForward, None),
            "one\n  |two"
        );
        assert_eq!(
            moves("o|ne\n\ntwo", Motion::WordForward, None),
            "one\n|\ntwo"
        );
        assert_eq!(
            moves("one\n|\ntwo", Motion::WordForward, None),
            "one\n\n|two"
        );
        assert_eq!(moves("o|ne", Motion::WordForward, None), "one|");
    }

    #[test]
    fn words_backward() {
        assert_eq!(moves("one tw|o", Motion::WordBackward, None), "one |two");
        assert_eq!(moves("one |two", Motion::WordBackward, None), "|one two");
        assert_eq!(
            moves("one\n\n|two", Motion::WordBackward, None),
            "one\n|\ntwo"
        );
        assert_eq!(moves("one.|two", Motion::WordBackward, None), "one|.two");
        assert_eq!(moves("|one", Motion::WordBackward, None), "|one");
    }

    #[test]
    fn word_ends() {
        assert_eq!(moves("|one two", Motion::WordEnd, None), "on|e two");
        assert_eq!(moves("on|e two", Motion::WordEnd, None), "one tw|o");
        assert_eq!(moves("on|e\n\n two", Motion::WordEnd, None), "one\n\n tw|o");
        assert_eq!(moves("on|e", Motion::WordEnd, None), "on|e");
    }

    #[test]
    fn within_the_line() {
        assert_eq!(moves("  ab|c\nd", Motion::LineStart, None), "|  abc\nd");
        assert_eq!(moves("  ab|c\nd", Motion::FirstNonBlank, None), "  |abc\nd");
        assert_eq!(moves("|abc\nde", Motion::LineEnd, None), "ab|c\nde");
        assert_eq!(moves("|abc\nde", Motion::LineEnd, Some(2)), "abc\nd|e");
        assert_eq!(moves("a|b\n  c", Motion::NextLine, None), "ab\n  |c");
    }

    #[test]
    fn whole_text() {
        let text = "a\n b\nc|";
        assert_eq!(moves(text, Motion::FirstLine, None), "|a\n b\nc");
        assert_eq!(moves(text, Motion::FirstLine, Some(2)), "a\n |b\nc");
        assert_eq!(moves("|a\n b\nc", Motion::LastLine, None), "a\n b\n|c");
        assert_eq!(moves("|a\n b\nc", Motion::LastLine, Some(2)), "a\n |b\nc");
    }
}
