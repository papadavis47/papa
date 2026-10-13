//! Markdown to Typst markup, by walking the Markdown syntax tree.
//!
//! The `markdown` crate (the one the app's preview uses, so both read a note
//! the same way) parses the text into an *mdast*: a tree of nodes such as
//! `Paragraph`, `Strong` and `Text`. [`write_body`] walks that tree depth
//! first and writes the Typst for each node.
//!
//! Two rules keep the output valid whatever the note says:
//!
//! - **Structure is written as function calls**, `#strong[…]`, `#list(…)`,
//!   never as Typst's shorthand (`*…*`, `- …`), which depends on spacing and
//!   line starts.
//! - **Text is escaped**: every ASCII punctuation character gets a
//!   backslash, so nothing a father types can turn into markup. Typst
//!   accepts a backslash before any character, so over-escaping is harmless.
//!   See [`escape`].

use std::collections::HashMap;

use markdown::mdast::{AlignKind, Node};
use markdown::{ParseOptions, to_mdast};

/// Writes `markdown` as Typst markup, to go inside a content block (`[…]`).
///
/// The note's own headings are written one level down (`#` becomes level
/// 2), since level 1 is the note's title.
pub fn write_body(markdown: &str, out: &mut String) {
    // GitHub's flavour, as in the preview: tables, task lists,
    // strikethrough, footnotes and bare links. With no other constructs
    // turned on, parsing can't fail (only MDX can), but if it ever did,
    // the text is kept as it was typed.
    let Ok(root) = to_mdast(markdown, &ParseOptions::gfm()) else {
        escape(markdown, out);
        return;
    };
    let mut writer = Writer::new(&root, out);
    writer.blocks(children(&root));
}

/// Writes `text` so Typst shows it as typed.
///
/// Escapes all ASCII punctuation except quotes, which are left for Typst to
/// curl (`"hi"` becomes “hi”). Typst's markup is made of ASCII punctuation
/// (`*bold*`, `= Heading`, `#code`, `$math$`, `<label>`, `// comment`…),
/// and the escapes also stop text from extending a function call written
/// just before it: `#emph[a].b` would be a field access, `#emph[a]\.b` isn't.
pub fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        if c.is_ascii_punctuation() && !matches!(c, '"' | '\'') {
            out.push('\\');
        }
        out.push(c);
    }
}

/// Writes `text` as a Typst string literal: `"…"` with `\` and `"` escaped.
/// For code and URLs, which are data rather than markup.
pub fn string(text: &str, out: &mut String) {
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Walks one note's tree, writing into `out`.
struct Writer<'a> {
    out: &'a mut String,
    /// `[text][ref]` links and images point at a definition elsewhere in
    /// the note: `[ref]: https://…`. By identifier.
    definitions: HashMap<&'a str, &'a str>,
    /// Footnote bodies by identifier. Typst puts a footnote's text where it
    /// is referenced, so the body is written there.
    footnotes: HashMap<&'a str, &'a [Node]>,
}

impl<'a> Writer<'a> {
    fn new(root: &'a Node, out: &'a mut String) -> Self {
        let mut writer = Self {
            out,
            definitions: HashMap::new(),
            footnotes: HashMap::new(),
        };
        writer.collect(root);
        writer
    }

    /// Finds the link definitions and footnotes, which may come after
    /// their first use.
    fn collect(&mut self, node: &'a Node) {
        match node {
            Node::Definition(definition) => {
                self.definitions
                    .insert(&definition.identifier, &definition.url);
            }
            Node::FootnoteDefinition(footnote) => {
                self.footnotes
                    .insert(&footnote.identifier, &footnote.children);
            }
            _ => {}
        }
        for child in children(node) {
            self.collect(child);
        }
    }

    /// Block nodes, one after another, separated by blank lines (Typst's
    /// paragraph break).
    fn blocks(&mut self, nodes: &'a [Node]) {
        let mut first = true;
        for node in nodes {
            // Written where they're used, or not at all.
            if matches!(node, Node::Definition(_) | Node::FootnoteDefinition(_)) {
                continue;
            }
            if !first {
                self.out.push_str("\n\n");
            }
            first = false;
            self.block(node);
        }
    }

    fn block(&mut self, node: &'a Node) {
        match node {
            Node::Paragraph(paragraph) => self.inlines(&paragraph.children),
            Node::Heading(heading) => {
                // Markdown's `#` is level 1, but a note's title is level 1
                // in the PDF. Typst headings go deeper than Markdown's six.
                let level = heading.depth + 1;
                self.out
                    .push_str(&format!("#heading(level: {level}, outlined: false)["));
                self.inlines(&heading.children);
                self.out.push(']');
            }
            Node::Blockquote(quote) => {
                self.out.push_str("#quote(block: true)[");
                self.blocks(&quote.children);
                self.out.push(']');
            }
            Node::List(list) => {
                if list.ordered {
                    let start = list.start.unwrap_or(1);
                    self.out.push_str(&format!("#enum(start: {start}, "));
                } else {
                    self.out.push_str("#list(");
                }
                // A "tight" list has no blank lines between its items.
                self.out.push_str(if list.spread {
                    "tight: false"
                } else {
                    "tight: true"
                });
                for item in &list.children {
                    self.out.push_str(", [");
                    if let Node::ListItem(item) = item {
                        if let Some(done) = item.checked {
                            self.out.push_str(if done {
                                "#checkbox(true) "
                            } else {
                                "#checkbox(false) "
                            });
                        }
                        self.blocks(&item.children);
                    } else {
                        self.block(item);
                    }
                    self.out.push(']');
                }
                self.out.push(')');
            }
            Node::Code(code) => {
                self.out.push_str("#raw(block: true, ");
                if let Some(lang) = &code.lang {
                    self.out.push_str("lang: ");
                    string(lang, self.out);
                    self.out.push_str(", ");
                }
                string(&code.value, self.out);
                self.out.push(')');
            }
            Node::ThematicBreak(_) => self.out.push_str("#rule()"),
            Node::Table(table) => self.table(&table.align, &table.children),
            // Raw HTML isn't rendered, as in the preview: it's shown as typed.
            Node::Html(html) => escape(&html.value, self.out),
            // Anything else that can stand alone holds inline content.
            other => self.inlines(children(other)),
        }
    }

    /// A GitHub table. Its first row is the header.
    fn table(&mut self, align: &[AlignKind], rows: &'a [Node]) {
        // One alignment per column. The trailing comma keeps a single one
        // an array: `(left,)`.
        self.out
            .push_str(&format!("#table(columns: {}, align: (", align.len()));
        for align in align {
            self.out.push_str(match align {
                AlignKind::Center => "center, ",
                AlignKind::Right => "right, ",
                AlignKind::Left | AlignKind::None => "left, ",
            });
        }
        self.out.push(')');
        let mut rows = rows.iter();
        if let Some(header) = rows.next() {
            self.out.push_str(", table.header(");
            // Arguments are code, not markup: no `#` before a call.
            self.cells(header, "strong[");
            self.out.push(')');
        }
        for row in rows {
            self.out.push_str(", ");
            self.cells(row, "[");
        }
        self.out.push(')');
    }

    /// A table row's cells, each opened with `open` and closed with `]`.
    fn cells(&mut self, row: &'a Node, open: &str) {
        for (ix, cell) in children(row).iter().enumerate() {
            if ix > 0 {
                self.out.push_str(", ");
            }
            self.out.push_str(open);
            self.inlines(children(cell));
            self.out.push(']');
        }
    }

    fn inlines(&mut self, nodes: &'a [Node]) {
        for node in nodes {
            self.inline(node);
        }
    }

    fn inline(&mut self, node: &'a Node) {
        match node {
            Node::Text(text) => escape(&text.value, self.out),
            Node::Emphasis(emphasis) => self.wrap("#emph[", &emphasis.children),
            Node::Strong(strong) => self.wrap("#strong[", &strong.children),
            Node::Delete(delete) => self.wrap("#strike[", &delete.children),
            Node::InlineCode(code) => {
                self.out.push_str("#raw(");
                string(&code.value, self.out);
                self.out.push(')');
            }
            Node::Break(_) => self.out.push_str("#linebreak()"),
            Node::Link(link) => self.link(&link.url, &link.children),
            Node::LinkReference(link) => match self.definitions.get(link.identifier.as_str()) {
                Some(url) => self.link(url, &link.children),
                None => self.inlines(&link.children),
            },
            // Images in the text point at the web, which an export doesn't
            // fetch: their description stands in for them. A note's own
            // photos are attached, not linked, and follow its text.
            Node::Image(image) => self.wrap_text("#emph[", &image.alt),
            Node::ImageReference(image) => self.wrap_text("#emph[", &image.alt),
            Node::FootnoteReference(reference) => {
                match self.footnotes.get(reference.identifier.as_str()) {
                    Some(body) => {
                        self.out.push_str("#footnote[");
                        self.blocks(body);
                        self.out.push(']');
                    }
                    None => escape(&format!("[^{}]", reference.identifier), self.out),
                }
            }
            Node::Html(html) => escape(&html.value, self.out),
            // Constructs GitHub's flavour doesn't parse (math, MDX,
            // frontmatter) can't appear; anything else keeps its text.
            other => match children(other) {
                [] => escape(&other.to_string(), self.out),
                nodes => self.inlines(nodes),
            },
        }
    }

    fn wrap(&mut self, open: &str, nodes: &'a [Node]) {
        self.out.push_str(open);
        self.inlines(nodes);
        self.out.push(']');
    }

    fn wrap_text(&mut self, open: &str, text: &str) {
        if text.is_empty() {
            return;
        }
        self.out.push_str(open);
        escape(text, self.out);
        self.out.push(']');
    }

    fn link(&mut self, url: &str, nodes: &'a [Node]) {
        // Typst refuses an empty URL; `[text]()` is just text.
        if url.is_empty() {
            self.inlines(nodes);
            return;
        }
        self.out.push_str("#link(");
        string(url, self.out);
        self.out.push_str(")[");
        self.inlines(nodes);
        self.out.push(']');
    }
}

/// A node's children; empty for a leaf such as `Text`.
fn children(node: &Node) -> &[Node] {
    node.children().map_or(&[], Vec::as_slice)
}
