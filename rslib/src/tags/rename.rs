// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use super::is_tag_separator;
use super::matcher::TagMatcher;
use crate::prelude::*;
use crate::tags::register::normalize_tag_name;

impl Collection {
    /// Rename a given tag and its children on all notes that reference it,
    /// returning changed note count.
    pub fn rename_tag(&mut self, old_prefix: &str, new_prefix: &str) -> Result<OpOutput<usize>> {
        self.transact(Op::RenameTag, |col| {
            col.rename_tag_inner(old_prefix, new_prefix)
        })
    }
}

impl Collection {
    fn rename_tag_inner(&mut self, old_prefix: &str, new_prefix: &str) -> Result<usize> {
        require!(
            !new_prefix.contains(is_tag_separator),
            "replacement name can not contain a space",
        );
        require!(
            !new_prefix.trim().is_empty(),
            "replacement name must not be empty",
        );

        let usn = self.usn()?;

        // ensure normalized+matching parent case, but not case of existing tag.
        // The matching of parent case is mainly to be consistent with the way
        // decks are handled.
        let new_prefix = normalize_tag_name(new_prefix)?;
        let new_prefix = self
            .adjusted_case_for_parents(&new_prefix)?
            .map(Into::into)
            .unwrap_or(new_prefix);

        // gather tags that need replacing
        let mut re = TagMatcher::new(old_prefix)?;
        let matched_notes = self
            .storage
            .get_note_tags_by_predicate(|tags| re.is_match(tags))?;
        let match_count = matched_notes.len();

        // remove old prefix from the tag list
        for tag in self.storage.get_tags_by_predicate(|tag| re.is_match(tag))? {
            // Take the entry's new name from the entry itself. `into_new_tags()`
            // only collects what the note replacements below produce, so a tag
            // no note carries would be removed here and never registered again.
            re.replace(&tag.name, &new_prefix);
            self.remove_single_tag_undoable(tag)?;
        }

        // replace tags
        for mut note in matched_notes {
            let original = note.clone();
            note.tags = re.replace(&note.tags, &new_prefix);
            note.set_modified(usn);
            self.update_note_tags_undoable(&note, original)?;
        }

        // update tag list
        for tag in re.into_new_tags() {
            self.register_tag_string(tag, usn)?;
        }

        Ok(match_count)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn alltags(col: &Collection) -> Vec<String> {
        col.storage
            .all_tags()
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect()
    }

    /// Add a note carrying `tags`, then clear its tags, leaving the tag-list
    /// entries behind with no note referencing them. This is the state the
    /// collection is in until "Clear Unused Tags" is run.
    fn add_unused_tags(col: &mut Collection, tags: &[&str]) -> Result<()> {
        let nt = col.get_notetype_by_name("Basic")?.unwrap();
        let mut note = nt.new_note();
        note.tags = tags.iter().map(|t| t.to_string()).collect();
        col.add_note(&mut note, DeckId(1))?;
        note.tags.clear();
        col.update_note(&mut note)?;
        Ok(())
    }

    fn add_note_with_tag(col: &mut Collection, tag: &str) -> Result<()> {
        let nt = col.get_notetype_by_name("Basic")?.unwrap();
        let mut note = nt.new_note();
        note.tags = vec![tag.to_string()];
        col.add_note(&mut note, DeckId(1))?;
        Ok(())
    }

    #[test]
    fn rename_renames_the_tag_on_its_notes() -> Result<()> {
        let mut col = Collection::new();
        add_note_with_tag(&mut col, "parent::child")?;

        let count = col.rename_tag("parent::child", "renamed")?.output;

        assert_eq!(count, 1);
        assert_eq!(alltags(&col), &["renamed"]);
        Ok(())
    }

    #[test]
    fn rename_renames_children_too() -> Result<()> {
        let mut col = Collection::new();
        // Both have to be on a note: only the tags a note carries are
        // registered, not the ancestors of their names.
        add_note_with_tag(&mut col, "parent::child")?;
        add_note_with_tag(&mut col, "parent::child::leaf")?;
        assert_eq!(alltags(&col), &["parent::child", "parent::child::leaf"]);

        let count = col.rename_tag("parent::child", "renamed")?.output;

        assert_eq!(count, 2);
        assert_eq!(alltags(&col), &["renamed", "renamed::leaf"]);
        Ok(())
    }

    #[test]
    fn rename_moves_a_subtree_no_note_carries() -> Result<()> {
        let mut col = Collection::new();
        add_unused_tags(&mut col, &["parent::child", "parent::child::leaf"])?;
        assert_eq!(alltags(&col), &["parent::child", "parent::child::leaf"]);

        col.rename_tag("parent::child", "renamed")?;

        assert_eq!(alltags(&col), &["renamed", "renamed::leaf"]);
        Ok(())
    }

    #[test]
    fn rename_keeps_a_descendant_entry_no_note_carries() -> Result<()> {
        let mut col = Collection::new();
        add_note_with_tag(&mut col, "parent::child")?;
        add_unused_tags(&mut col, &["parent::child::leaf"])?;
        assert_eq!(alltags(&col), &["parent::child", "parent::child::leaf"]);

        col.rename_tag("parent::child", "renamed")?;

        // The entry has to be renamed, not dropped: it was removed from the
        // tag list with the rest of the subtree.
        assert_eq!(alltags(&col), &["renamed", "renamed::leaf"]);
        Ok(())
    }

    #[test]
    fn renaming_an_absent_tag_leaves_the_tag_list_alone() -> Result<()> {
        let mut col = Collection::new();
        add_unused_tags(&mut col, &["parent::child"])?;

        let count = col.rename_tag("absent", "renamed")?.output;

        assert_eq!(count, 0);
        assert_eq!(alltags(&col), &["parent::child"]);
        Ok(())
    }

    #[test]
    fn rename_rejects_an_empty_or_spaced_replacement() -> Result<()> {
        let mut col = Collection::new();
        add_note_with_tag(&mut col, "parent")?;

        assert!(col.rename_tag("parent", "  ").is_err());
        assert!(col.rename_tag("parent", "a b").is_err());
        assert_eq!(alltags(&col), &["parent"]);
        Ok(())
    }
}
