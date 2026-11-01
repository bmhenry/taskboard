//! The edit between two values of the soft-keyboard textarea (see
//! `keyboard.rs`), kept apart so it's tested natively.

use unicode_segmentation::UnicodeSegmentation;

/// An edit from one textarea value to the next: delete `deleted` grapheme
/// clusters and insert `inserted`, `after` grapheme clusters before the end.
///
/// The counts are in graphemes because that's what one backspace or arrow key
/// moves over, in the browser and in iced alike: a flag, a skin-toned or ZWJ
/// emoji, or a letter with a combining mark is one keypress, not several.
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub deleted: usize,
    pub inserted: String,
    pub after: usize,
}

pub fn diff(old: &str, new: &str) -> Edit {
    let old: Vec<&str> = old.graphemes(true).collect();
    let new: Vec<&str> = new.graphemes(true).collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let after = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    Edit {
        deleted: old.len() - prefix - after,
        inserted: new[prefix..new.len() - after].concat(),
        after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(deleted: usize, inserted: &str, after: usize) -> Edit {
        Edit {
            deleted,
            inserted: inserted.to_string(),
            after,
        }
    }

    #[test]
    fn typing_appends() {
        assert_eq!(diff(" ", " h"), edit(0, "h", 0));
        assert_eq!(diff(" aa", " aaa"), edit(0, "a", 0));
    }

    #[test]
    fn backspace_deletes_including_the_sentinel() {
        assert_eq!(diff(" ab", " a"), edit(1, "", 0));
        assert_eq!(diff(" ", ""), edit(1, "", 0));
    }

    #[test]
    fn composition_updates_and_autocorrect_replace() {
        assert_eq!(diff(" teh", " the "), edit(2, "he ", 0));
        assert_eq!(diff(" héllo", " héllö"), edit(1, "ö", 0));
    }

    #[test]
    fn an_edit_before_the_end_keeps_the_tail() {
        assert_eq!(diff(" abcd", " aXcd"), edit(1, "X", 2));
    }

    #[test]
    fn multi_scalar_graphemes_count_as_one() {
        // A flag (two regional indicators), a skin-toned ZWJ emoji and an e
        // with a combining acute accent are one backspace each.
        assert_eq!(diff(" a\u{1F1F3}\u{1F1FF}", " a"), edit(1, "", 0));
        assert_eq!(
            diff(" a\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}", " a"),
            edit(1, "", 0)
        );
        assert_eq!(diff(" cafe\u{301}", " caf"), edit(1, "", 0));
        // And one arrow press each when the edit is before them.
        assert_eq!(
            diff(" x\u{1F1F3}\u{1F1FF}", " xy\u{1F1F3}\u{1F1FF}"),
            edit(0, "y", 1)
        );
        assert_eq!(
            diff(
                " a\u{1F469}\u{1F3FD}\u{200D}\u{1F4BB}\u{1F1F3}\u{1F1FF}",
                " \u{1F1F3}\u{1F1FF}"
            ),
            edit(2, "", 1)
        );
    }

    #[test]
    fn deleting_one_of_a_repeated_char_in_the_middle() {
        // Which "l" went is ambiguous, but every choice is the same edit to
        // the text iced holds.
        assert_eq!(diff(" hello", " helo"), edit(1, "", 1));
        assert_eq!(diff(" aaab", " aab"), edit(1, "", 1));
        assert_eq!(diff(" abba", " aba"), edit(1, "", 1));
    }
}
