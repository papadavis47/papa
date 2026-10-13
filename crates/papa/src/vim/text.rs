//! A note's text as Vim sees it: characters and lines.

use std::ops::Range;

/// A snapshot of the text, indexed by character.
///
/// The text field counts in UTF-8 bytes, where "é" is two bytes and an
/// emoji four. Motions count in characters: `l` moves one character,
/// however many bytes it takes. So the snapshot keeps both: the characters,
/// and the byte offset where each one starts.
pub struct Text {
    chars: Vec<char>,
    /// `offsets[i]` is where character `i` starts. One extra entry at the
    /// end holds the text's length, so `offsets[len]` is valid too.
    offsets: Vec<usize>,
}

impl Text {
    pub fn new(text: &str) -> Self {
        let (offsets, chars): (Vec<usize>, Vec<char>) = text.char_indices().unzip();
        let mut offsets = offsets;
        offsets.push(text.len());
        Self { chars, offsets }
    }

    /// The number of characters.
    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn char(&self, ix: usize) -> Option<char> {
        self.chars.get(ix).copied()
    }

    /// The byte offset of character `ix`.
    pub fn byte(&self, ix: usize) -> usize {
        self.offsets[ix.min(self.len())]
    }

    pub fn bytes(&self, range: Range<usize>) -> Range<usize> {
        self.byte(range.start)..self.byte(range.end)
    }

    /// The character at byte `offset`, or the one it falls inside.
    pub fn ix(&self, offset: usize) -> usize {
        match self.offsets.binary_search(&offset) {
            Ok(ix) => ix,
            Err(ix) => ix.saturating_sub(1),
        }
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.chars[range].iter().collect()
    }

    fn is_newline(&self, ix: usize) -> bool {
        self.char(ix) == Some('\n')
    }

    /// The first character of `ix`'s line.
    pub fn line_start(&self, ix: usize) -> usize {
        let ix = ix.min(self.len());
        self.chars[..ix]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |newline| newline + 1)
    }

    /// The newline that ends `ix`'s line, or the end of the text on the
    /// last line.
    pub fn line_end(&self, ix: usize) -> usize {
        let ix = ix.min(self.len());
        self.chars[ix..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.len(), |newline| ix + newline)
    }

    /// Which line `ix` is on, from 0.
    pub fn line_of(&self, ix: usize) -> usize {
        let ix = ix.min(self.len());
        self.chars[..ix].iter().filter(|&&c| c == '\n').count()
    }

    pub fn line_count(&self) -> usize {
        self.line_of(self.len()) + 1
    }

    /// The first character of line `line` (from 0), or of the last line.
    pub fn line(&self, line: usize) -> usize {
        let mut start = 0;
        for _ in 0..line {
            let end = self.line_end(start);
            if end == self.len() {
                break;
            }
            start = end + 1;
        }
        start
    }

    /// How far `ix` is from the start of its line, in characters.
    pub fn column(&self, ix: usize) -> usize {
        ix - self.line_start(ix)
    }

    /// The first character of `ix`'s line that isn't a space or a tab.
    pub fn first_non_blank(&self, ix: usize) -> usize {
        let start = self.line_start(ix);
        let end = self.line_end(ix);
        (start..end)
            .find(|&i| !matches!(self.chars[i], ' ' | '\t'))
            .unwrap_or(end)
    }

    /// The last character of `ix`'s line: where Normal mode's cursor stops.
    /// On an empty line, the line's start.
    pub fn last_char(&self, ix: usize) -> usize {
        let start = self.line_start(ix);
        self.line_end(ix).saturating_sub(1).max(start)
    }

    /// `ix`, moved back onto a character if it's past the end of its line.
    /// In Normal mode the cursor sits *on* a character, never after the last.
    pub fn clamp_normal(&self, ix: usize) -> usize {
        ix.min(self.last_char(ix))
    }

    /// The end of a range that includes the character at `ix`: one past it,
    /// unless `ix` is a newline or the end, which an inclusive motion stops
    /// short of.
    pub fn inclusive_end(&self, ix: usize) -> usize {
        if ix < self.len() && !self.is_newline(ix) {
            ix + 1
        } else {
            ix
        }
    }

    /// Whether `ix` is the start of an empty line, which Vim treats as a
    /// word of its own.
    pub fn is_empty_line(&self, ix: usize) -> bool {
        self.is_newline(ix) && (ix == 0 || self.is_newline(ix - 1))
    }

    /// The text with `range` replaced by `with`.
    pub fn replace(&self, range: Range<usize>, with: &str) -> Text {
        let mut text = self.slice(0..range.start);
        text.push_str(with);
        text.push_str(&self.slice(range.end..self.len()));
        Text::new(&text)
    }

    /// Normal mode's cursor at `ix`, as a caret in front of the character
    /// it's on. The editor draws the block over that character.
    pub fn caret(&self, ix: usize) -> Range<usize> {
        let ix = self.clamp_normal(ix);
        ix..ix
    }
}

/// What kind of character this is, for the word motions. A word is a run
/// of letters, digits and underscores, or a run of other non-blank
/// characters: `don't` is three words, `don`, `'` and `t`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CharClass {
    Blank,
    Word,
    Punctuation,
}

impl CharClass {
    pub fn of(c: char) -> Self {
        if c.is_whitespace() {
            Self::Blank
        } else if c.is_alphanumeric() || c == '_' {
            Self::Word
        } else {
            Self::Punctuation
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_characters_line_up() {
        let text = Text::new("né\n🙂x");
        assert_eq!(text.len(), 5);
        assert_eq!(text.byte(2), 3, "é is two bytes");
        assert_eq!(text.byte(4), 8, "🙂 is four");
        assert_eq!(text.ix(8), 4);
        assert_eq!(text.ix(2), 1, "inside é: the character it's in");
        assert_eq!(text.byte(text.len()), 9);
    }

    #[test]
    fn lines() {
        let text = Text::new("ab\n\n  cd");
        assert_eq!(text.line_count(), 3);
        assert_eq!((text.line_start(1), text.line_end(1)), (0, 2));
        assert_eq!((text.line_start(3), text.line_end(3)), (3, 3));
        assert_eq!(text.line(2), 4);
        assert_eq!(text.line(9), 4, "past the last line: the last line");
        assert_eq!(text.first_non_blank(7), 6);
        assert_eq!(text.last_char(0), 1);
        assert_eq!(text.last_char(3), 3, "empty line");
        assert!(text.is_empty_line(3));
        assert!(!text.is_empty_line(2));
    }
}
