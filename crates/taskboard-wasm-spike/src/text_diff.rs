//! The edit between two values of the soft-keyboard textarea (see
//! `keyboard.rs`), kept apart so it's tested natively.

/// An edit from one textarea value to the next: delete `deleted` characters
/// and insert `inserted`, `after` characters before the end.
#[derive(Debug, PartialEq, Eq)]
pub struct Edit {
    pub deleted: usize,
    pub inserted: String,
    pub after: usize,
}

pub fn diff(old: &str, new: &str) -> Edit {
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
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
        inserted: new[prefix..new.len() - after].iter().collect(),
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
}
