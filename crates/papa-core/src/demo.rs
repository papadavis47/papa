//! The demo vault behind `papa --demo`: an invented family, so the app can
//! be tried, and screenshots taken, without any real child's notes.
//!
//! [`create`] makes it from scratch in a folder of its own, [`dir`], every
//! time the demo starts, and [`paths::use_demo_dir`](crate::paths::use_demo_dir)
//! sends the app there, settings included. Nothing the demo does touches
//! the real vault, and nothing done in the demo is kept for the next one.
//!
//! The notes are dated relative to today, so the demo always looks like a
//! journal someone is keeping now. Its photos are illustrations, compiled
//! into the binary with `include_bytes!`, so an installed Papa has them too.

use std::path::{Path, PathBuf};

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp, Zoned};

use crate::{ChildDraft, Error, NoteDraft, Result, Store, files, images, paths};

/// The demo's folder: `papa-demo` in the system's temporary folder.
pub fn dir() -> PathBuf {
    std::env::temp_dir().join("papa-demo")
}

/// Makes a fresh demo vault in `dir`, replacing whatever an earlier demo
/// left there, and returns the vault's path. Blocks until it's written.
///
/// It runs before the app's window opens, from plain synchronous `main`,
/// so it brings a small runtime of its own to wait on the store with: the
/// same `block_on` the store's thread uses.
pub fn create(dir: &Path) -> Result<PathBuf> {
    // Only ever the demo's own folder, which holds nothing worth keeping.
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|reason| Error::WriteFile {
            path: dir.to_owned(),
            reason,
        })?;
    }
    files::create_private_dir(dir)?;
    let vault = paths::demo_vault(dir);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(Error::StartThread)?;
    runtime.block_on(async {
        let store = Store::open(&vault).await?;
        seed(&store, &Zoned::now()).await?;
        store.close().await
    })?;
    Ok(vault)
}

/// The demo's children, in sidebar order, with the first three of the
/// colours the app offers for a new child.
const CHILDREN: [(&str, u32); 3] = [("Maya", 0xe0795b), ("Leo", 0x5b9be0), ("Nora", 0x6bbf7a)];

const LABELS: [&str; 5] = ["Firsts", "Funny things", "School", "Trips", "Health"];

/// One of the demo's notes.
struct Sample {
    /// How long ago it was written, and at what time of day.
    days_ago: i64,
    at: (i8, i8),
    title: &'static str,
    /// Names from [`CHILDREN`] and [`LABELS`].
    children: &'static [&'static str],
    labels: &'static [&'static str],
    photos: &'static [&'static [u8]],
    body: &'static str,
}

const BIKE: &[u8] = include_bytes!("../demo/bike.jpg");
const CAKE: &[u8] = include_bytes!("../demo/cake.jpg");
const DRAWING: &[u8] = include_bytes!("../demo/drawing.jpg");
const SANDCASTLE: &[u8] = include_bytes!("../demo/sandcastle.jpg");

/// Adds the demo's children, labels, notes and photos to `store`, dating
/// the notes back from `now`.
async fn seed(store: &Store, now: &Zoned) -> Result<()> {
    let mut children = Vec::new();
    for (name, color) in CHILDREN {
        let draft = ChildDraft {
            name: name.into(),
            color,
        };
        children.push((name, store.add_child(draft).await?.id));
    }
    let mut labels = Vec::new();
    for name in LABELS {
        labels.push((name, store.add_label(name).await?.id));
    }

    for sample in SAMPLES {
        let draft = NoteDraft {
            title: sample.title.into(),
            body: sample.body.trim().into(),
            children: ids(&children, sample.children),
            labels: ids(&labels, sample.labels),
        };
        let at = written_at(now.date(), now.time_zone(), sample.days_ago, sample.at);
        let note = store.add_note_at(draft, at).await?;
        for photo in sample.photos {
            store
                .add_image(note.id, images::prepare(photo.to_vec())?)
                .await?;
        }
    }
    Ok(())
}

/// The ids of the children or labels `names`. A name that isn't one of
/// the demo's is a typo in [`SAMPLES`]; the tests seed them all, so it
/// fails them.
fn ids<Id: Copy>(all: &[(&str, Id)], names: &[&str]) -> Vec<Id> {
    names
        .iter()
        .map(|name| {
            all.iter()
                .find(|(n, _)| n == name)
                .map(|&(_, id)| id)
                .unwrap_or_else(|| panic!("the demo note's {name:?} isn't one of the demo's"))
        })
        .collect()
}

/// `days_ago` days before `today`, at `(hour, minute)` local time.
fn written_at(today: Date, tz: &TimeZone, days_ago: i64, (hour, minute): (i8, i8)) -> Timestamp {
    // Can't fail: the dates are a few years back from today, and a time
    // in a daylight-saving gap is moved forward rather than refused.
    today
        .checked_sub(Span::new().days(days_ago))
        .and_then(|date| date.at(hour, minute, 0, 0).to_zoned(tz.clone()))
        .expect("the demo's dates are in range")
        .timestamp()
}

const SAMPLES: &[Sample] = &[
    Sample {
        days_ago: 2,
        at: (19, 40),
        title: "Leo rode without training wheels",
        children: &["Leo"],
        labels: &["Firsts"],
        photos: &[BIKE],
        body: r#"
We took the training wheels off on Saturday and he wanted them back on within five minutes. By Sunday afternoon he was doing laps of the park path on his own.

What worked, in the end:

1. Lowering the seat so both feet reached the ground.
2. Letting him scoot along without pedalling until he found his balance.
3. Me jogging behind and **not** holding on, even though he kept asking.

> "Dad, you can let go now. Oh. You already did."

He fell twice, got up both times, and told everyone at dinner that he "basically didn't fall at all."
"#,
    },
    Sample {
        days_ago: 6,
        at: (21, 15),
        title: "Things Nora said this week",
        children: &["Nora"],
        labels: &["Funny things"],
        photos: &[],
        body: r#"
- Looking at the moon in the afternoon: *"The moon forgot to go home."*
- On why she couldn't tidy up: *"My hands are too tired. My feet can do it."*
- To the cat: *"You're not allowed to be a cat at dinner."*
- After her bath, very seriously: "I'm clean on the outside now."

She's three and a half and has an opinion about everything, especially socks.
"#,
    },
    Sample {
        days_ago: 12,
        at: (18, 5),
        title: "Maya's science fair volcano",
        children: &["Maya"],
        labels: &["School"],
        photos: &[],
        body: r#"
Maya's project this year was *Why do volcanoes erupt?* She did nearly all of it herself; I was in charge of buying baking soda.

## What she learned
- Magma is lighter than the rock around it, so it rises.
- The gas in it is what makes an eruption explosive, a bit like a shaken bottle.
- There are volcanoes under the sea that nobody has ever seen.

## What I learned
- Vinegar gets *everywhere*.
- She explains things better than I do. Her judge asked a question she hadn't prepared for and she said, "I don't know yet, but here's how I'd find out."

She got a ribbon for **best explanation**, which she says is better than first place.
"#,
    },
    Sample {
        days_ago: 25,
        at: (8, 30),
        title: "Leo's drawing for the fridge",
        children: &["Leo"],
        labels: &[],
        photos: &[DRAWING],
        body: r#"
Leo drew "our house, but the good version." The good version has a dog, which we don't have, and the sun is smiling even though he didn't draw a face on it. "You can tell," he said.

It's on the fridge now, next to the shopping list, and he checks every morning that it's still there.
"#,
    },
    Sample {
        days_ago: 41,
        at: (16, 50),
        title: "Rainy Sunday: pancakes and a pillow fort",
        children: &["Maya", "Leo", "Nora"],
        labels: &[],
        photos: &[],
        body: r#"
Rain all day, so we stayed in. Pancakes for breakfast (Leo flipped one onto the floor, the cat was delighted), then the biggest pillow fort we've built.

Maya was the architect, Leo was in charge of "security," and Nora fell asleep inside it before lunch. We read three chapters of the dragon book in there with a torch.

Note for next time: the sofa cushions don't fit back the way they came off.
"#,
    },
    Sample {
        days_ago: 63,
        at: (20, 10),
        title: "Leo turned seven",
        children: &["Leo"],
        labels: &[],
        photos: &[CAKE],
        body: r#"
He asked for a chocolate cake "with all the sprinkles," and Maya made sure that's what he got. Seven candles, one wish, and he blew them out in a single go, which he has mentioned several times since.

**Presents he liked most:** the bike (of course), a torch that turns into a lantern, and a card from Nora that was mostly glitter.

When I tucked him in he said seven feels "about the same as six, but taller."
"#,
    },
    Sample {
        days_ago: 94,
        at: (7, 45),
        title: "First day of school",
        children: &["Maya", "Leo"],
        labels: &["School", "Firsts"],
        photos: &[],
        body: r#"
Leo's first day of proper school and Maya's first day in year four.

Maya walked him to his classroom door and told him where the toilets were and which slide is the fast one. He didn't look back once. (I did.)

At pick-up:
- **Leo:** "It was good. We had fish fingers. Can I go again tomorrow?"
- **Maya:** "My teacher has a snake. A real one."
"#,
    },
    Sample {
        days_ago: 121,
        at: (21, 30),
        title: "Beach week",
        children: &["Maya", "Leo", "Nora"],
        labels: &["Trips"],
        photos: &[SANDCASTLE],
        body: r#"
A week at the coast. Highlights, as voted at dinner on the last night:

- **Maya:** finding a crab "as big as my hand" (it was smaller than her hand).
- **Leo:** the sandcastle. It had a moat, four towers and a shell on top, and the tide took it at 6:12 pm. He timed it.
- **Nora:** "the sea," every time anyone asked.

Next year: more sun cream, fewer sandwiches with actual sand in them.
"#,
    },
    Sample {
        days_ago: 152,
        at: (12, 20),
        title: "Nora's first swimming lesson",
        children: &["Nora"],
        labels: &["Firsts"],
        photos: &[],
        body: r#"
She refused to get in for the first ten minutes, then refused to get out at the end. Somewhere in between she blew bubbles, kicked her legs and splashed the instructor on purpose.

On the way home: "I'm a fish now. A small fish. Next week a big fish."
"#,
    },
    Sample {
        days_ago: 203,
        at: (19, 0),
        title: "Maya lost her front tooth",
        children: &["Maya"],
        labels: &["Firsts"],
        photos: &[],
        body: r#"
It had been wobbly for two weeks and finally came out in an apple at lunch. She was very calm about it and very interested in the gap.

She wrote the tooth fairy a note asking whether teeth are "recycled," and if so, what into. The tooth fairy left a coin and a reply saying they become stars, which Maya accepted, *for now*.
"#,
    },
    Sample {
        days_ago: 261,
        at: (9, 10),
        title: "Ear infection, again",
        children: &["Nora"],
        labels: &["Health"],
        photos: &[],
        body: r#"
Third one this winter. Up most of the night; doctor this morning.

- Antibiotics for seven days, with food.
- Paracetamol for the pain, as needed.
- Back in two weeks if it hasn't cleared, and to talk about a referral if it happens again.

She was brave at the doctor's and asked for a sticker for her bear too.
"#,
    },
    Sample {
        days_ago: 330,
        at: (20, 45),
        title: "Leo's questions at bedtime",
        children: &["Leo"],
        labels: &["Funny things"],
        photos: &[],
        body: r#"
Tonight's, all in about four minutes:

1. Do fish get thirsty?
2. If you dig all the way down, what's at the bottom?
3. Why is it called a *bed*room if it's also a toy room?
4. Were you ever six?
5. Did you have dinosaurs?

I answered the first four as well as I could. The fifth one hurt.
"#,
    },
    Sample {
        days_ago: 402,
        at: (18, 30),
        title: "Camping at the lake",
        children: &["Maya", "Leo"],
        labels: &["Trips"],
        photos: &[],
        body: r#"
First time camping with just the two older ones. The tent went up on the second try.

**Good:** toasting marshmallows, seeing more stars than either of them knew there were, Maya spotting a heron.

**Less good:** Leo deciding at 2 am that he could hear a bear. It was me, unzipping the tent.

They both want to go again, "but with a bigger tent and no bears."
"#,
    },
    Sample {
        days_ago: 498,
        at: (21, 0),
        title: "Maya read her first chapter book",
        children: &["Maya"],
        labels: &["Firsts", "School"],
        photos: &[],
        body: r#"
She finished it under the covers with a torch, then came downstairs at nine o'clock to tell me how it ended, in full, with voices.

She's already started the next one in the series and has asked for a library card "of my own, with my name on it."
"#,
    },
    Sample {
        days_ago: 712,
        at: (17, 25),
        title: "Nora's first steps",
        children: &["Nora"],
        labels: &["Firsts"],
        photos: &[],
        body: r#"
Four steps, from the sofa to Maya, who had been coaching her all week. Then she sat down hard and laughed.

Maya says she taught her. Leo says he taught her *before* Maya did. Both of them cried a bit, and so did I.
"#,
    },
    Sample {
        days_ago: 1068,
        at: (22, 15),
        title: "Why I'm keeping this journal",
        children: &[],
        labels: &[],
        photos: &[],
        body: r#"
Most of what happens in a week is gone by the next one. I want to remember the small things: what they said, what they were scared of, what made them laugh so hard they couldn't breathe.

One day I might share some of these with them. Until then, they're just for me.
"#,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NoteFilter, NoteQuery};

    fn noon(date: &str) -> Zoned {
        format!("{date}T12:00[UTC]").parse().unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn seeds_an_invented_family() {
        let store = Store::open_in_memory().await.unwrap();
        seed(&store, &noon("2026-10-16")).await.unwrap();

        let children = store.children().await.unwrap();
        let names: Vec<_> = children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Maya", "Leo", "Nora"]);
        assert_eq!(store.labels().await.unwrap().len(), LABELS.len());

        let all = store.notes(NoteQuery::default()).await.unwrap();
        assert_eq!(all.len(), SAMPLES.len());
        // Newest first, two days before "today", at the sample's time.
        assert_eq!(all[0].title, "Leo rode without training wheels");
        assert_eq!(
            all[0].created_at,
            "2026-10-14T19:40:00Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(all[0].images, 1);
        assert_eq!(all.last().unwrap().title, "Why I'm keeping this journal");
        let photos: usize = all.iter().map(|note| note.images).sum();
        assert_eq!(photos, 4);

        // Notes are filed under their children.
        let nora = children[2].id;
        let query = NoteQuery {
            filter: NoteFilter::Child(nora),
            ..NoteQuery::default()
        };
        let hers = store.notes(query).await.unwrap();
        let expected = SAMPLES.iter().filter(|s| s.children.contains(&"Nora"));
        assert_eq!(hers.len(), expected.count());
        assert!(hers.iter().all(|note| note.children.contains(&nora)));
    }

    #[test]
    fn notes_are_written_in_local_time() {
        let tz = TimeZone::get("America/New_York").unwrap();
        let today = Date::new(2026, 3, 9).unwrap();
        // The day before the clocks went forward on 8 March: still EST,
        // five hours behind UTC.
        let at = written_at(today, &tz, 2, (19, 40));
        assert_eq!(at, "2026-03-08T00:40:00Z".parse::<Timestamp>().unwrap());
    }

    #[test]
    fn a_new_demo_replaces_the_last() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("papa-demo");
        let vault = create(&dir).unwrap();
        // Something left over from the last demo.
        std::fs::write(dir.join("leftover"), "x").unwrap();
        let again = create(&dir).unwrap();
        assert_eq!(vault, again);
        assert!(!dir.join("leftover").exists());

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let notes = runtime.block_on(async {
            let store = Store::open(&vault).await.unwrap();
            store.notes(NoteQuery::default()).await.unwrap()
        });
        // Seeded once, not on top of the last demo's notes.
        assert_eq!(notes.len(), SAMPLES.len());
    }
}
