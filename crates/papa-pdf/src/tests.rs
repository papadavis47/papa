//! Tests lay documents out and read the result back from the pages, so they
//! check what a reader would see rather than the markup we write.

use std::io::Cursor;

use typst::layout::{Frame, FrameItem};

use super::*;

fn note(title: &str, body: &str) -> Note {
    Note {
        title: title.to_owned(),
        meta: Some("Mar 3 2026".to_owned()),
        body: body.to_owned(),
        photos: Vec::new(),
    }
}

fn one(note: Note) -> Document {
    Document {
        notes: vec![note],
        ..Document::default()
    }
}

/// Each page's text, with whitespace removed: where lines wrap and how
/// words are spaced is the layout's business.
fn pages_text(document: Document) -> Vec<String> {
    layout(document)
        .unwrap()
        .pages()
        .iter()
        .map(|page| {
            let mut text = String::new();
            frame_text(&page.frame, &mut text);
            text.retain(|c| !c.is_whitespace());
            text
        })
        .collect()
}

fn frame_text(frame: &Frame, out: &mut String) {
    for (_, item) in frame.items() {
        match item {
            FrameItem::Text(text) => out.push_str(&text.text),
            FrameItem::Group(group) => frame_text(&group.frame, out),
            _ => {}
        }
    }
}

fn count_images(frame: &Frame) -> usize {
    frame
        .items()
        .map(|(_, item)| match item {
            FrameItem::Image(..) => 1,
            FrameItem::Group(group) => count_images(&group.frame),
            _ => 0,
        })
        .sum()
}

fn squeeze(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

fn png(width: u32, height: u32) -> Photo {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([200, 120, 80]));
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    Photo {
        format: PhotoFormat::Png,
        bytes,
        width,
        height,
    }
}

#[test]
fn titles_print_exactly_as_typed() {
    // Every one of these means something in Typst's markup.
    let title =
        r#"= #x *b* _i_ `r` $m$ <l> @r ~ // /* -- --- ... -? \ [a](b) {c} 1. - + / a: b; c"#;
    let pages = pages_text(one(note(title, "")));
    assert!(pages[0].starts_with(&squeeze(title)), "{}", pages[0]);
}

#[test]
fn text_in_the_body_prints_as_typed() {
    // Markdown escapes (`\*`) make Markdown's own symbols literal too.
    let body = r"Costs $5 or $10 <b> @home #1 ~ok // not a comment, \*not bold\* -- 2. one + two = 3; f(x).y [z]";
    let printed = r"Costs $5 or $10 <b> @home #1 ~ok // not a comment, *not bold* -- 2. one + two = 3; f(x).y [z]";
    let pages = pages_text(one(note("", body)));
    // The date under the title, then the text, then the page number.
    assert_eq!(pages[0], format!("Mar32026{}1", squeeze(printed)));
}

#[test]
fn quotes_are_curled() {
    let pages = pages_text(one(note("", r#"She said "no" and didn't"#)));
    assert_eq!(pages[0], "Mar32026Shesaid“no”anddidn’t1");
}

#[test]
fn markdown_constructs_lay_out() {
    let body = "\
# Heading
## Smaller heading

Some *emphasis*, **strong**, ~~struck~~ and `code \"with\" quotes`.\\
A hard break, a [link](https://example.com), a [reference][ref] and a footnote[^1].

> A quote
> over two lines.

1. one
2. two
   - nested
   - list

3. after a gap

- [x] done
- [ ] to do

```rust
fn main() { println!(\"hi\\n\"); }
```

| Left | Middle | Right |
|:-----|:------:|------:|
| a    | b      | c     |
| one cell |

---

<b>raw html</b> and ![a photo of the sea](https://example.com/sea.jpg)

Emoji 🚲 and 漢字.

[ref]: https://example.org
[^1]: The footnote.
";
    let text = pages_text(one(note("All of it", body))).concat();
    for expected in [
        "Heading",
        "Smallerheading",
        "emphasis,strong,struckandcode\"with\"quotes.",
        "Ahardbreak,alink,areferenceandafootnote1",
        "Aquoteovertwolines.",
        "1.one2.two",
        "nested",
        "3.afteragap",
        "fnmain(){println!(\"hi\\n\");}",
        "LeftMiddleRightabconecell",
        "<b>rawhtml</b>andaphotoofthesea",
        "Emoji🚲and漢字.",
        "1Thefootnote.",
    ] {
        assert!(text.contains(expected), "{expected:?} missing from {text}");
    }
    // Link definitions aren't printed.
    assert!(!text.contains("example.org"), "{text}");
}

#[test]
fn a_notes_own_headings_sit_below_its_title() {
    let (markup, _) = markup::write(&one(note("Title", "# Heading")));
    assert!(markup.contains("#heading(level: 2, outlined: false)[Heading]"));
}

#[test]
fn a_book_has_a_cover_and_contents() {
    let document = Document {
        cover: Some(Cover {
            title: "Emma".to_owned(),
            subtitle: "2 notes".to_owned(),
        }),
        notes: vec![
            note("First bike ride", "She refused the training wheels."),
            note("School play", "A tree, with one line."),
        ],
        paper: Paper::Letter,
    };
    let pages = pages_text(document);
    assert_eq!(pages[0], "Emma2notes");
    assert!(pages[1].starts_with("Contents"), "{}", pages[1]);
    assert!(pages[1].contains("Firstbikeride"), "{}", pages[1]);
    assert!(pages[1].contains("Schoolplay"), "{}", pages[1]);
    // The cover and contents have no page number; the count still
    // includes them, so the notes' page shows 3, as the contents say.
    assert_eq!(
        pages[2],
        "FirstbikerideMar32026Sherefusedthetrainingwheels.∗∗∗SchoolplayMar32026Atree,withoneline.3"
    );
    assert_eq!(pages.len(), 3);
}

#[test]
fn a_single_note_starts_on_the_first_page() {
    let pages = pages_text(one(note("First bike ride", "Off she went.")));
    assert_eq!(pages, ["FirstbikerideMar32026Offshewent.1"]);
}

#[test]
fn every_photo_is_placed() {
    let mut first = note("Beach", "Sand everywhere.");
    first.photos = vec![png(40, 30)];
    let mut second = note("Park", "");
    // A tall one and a wide one, in a grid.
    second.photos = vec![png(30, 40), png(40, 30)];
    let document = Document {
        notes: vec![first, second],
        ..Document::default()
    };

    let pages = layout(document).unwrap();

    let images: usize = pages
        .pages()
        .iter()
        .map(|page| count_images(&page.frame))
        .sum();
    assert_eq!(images, 3);
}

#[test]
fn render_writes_a_pdf() {
    let pdf = render(one(note("Hello", "World"))).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
}

#[test]
fn nothing_to_export_is_an_error() {
    assert!(matches!(render(Document::default()), Err(Error::Empty)));
}
