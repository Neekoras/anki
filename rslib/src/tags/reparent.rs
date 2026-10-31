// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashMap;

use unicase::UniCase;

use super::join_tags;
use super::matcher::TagMatcher;
use crate::prelude::*;

impl Collection {
    /// Reparent the provided tags under a new parent.
    ///
    /// Parents of the provided tags are left alone - only the final component
    /// and its children are moved. If a source tag is the parent of the target
    /// tag, it will remain unchanged. If `new_parent` is not provided, tags
    /// will be reparented to the root element. When reparenting tags, any
    /// children they have are reparented as well.
    ///
    /// For example:
    /// - foo,       bar       -> bar::foo
    /// - foo::bar,  baz       -> baz::bar
    /// - foo,       foo::bar  -> no action
    /// - foo::bar,  none      -> bar
    pub fn reparent_tags(
        &mut self,
        tags_to_reparent: &[String],
        new_parent: Option<String>,
    ) -> Result<OpOutput<usize>> {
        self.transact(Op::ReparentTag, |col| {
            col.reparent_tags_inner(tags_to_reparent, new_parent)
        })
    }

    pub fn reparent_tags_inner(
        &mut self,
        tags_to_reparent: &[String],
        new_parent: Option<String>,
    ) -> Result<usize> {
        let usn = self.usn()?;
        let mut matcher = TagMatcher::new(&join_tags(tags_to_reparent))?;
        let old_to_new_names = old_to_new_names(tags_to_reparent, new_parent);
        if old_to_new_names.is_empty() {
            return Ok(0);
        }
        let matched_notes = self
            .storage
            .get_note_tags_by_predicate(|tags| matcher.is_match(tags))?;
        let match_count = matched_notes.len();

        // remove old prefixes from the tag list
        for tag in self
            .storage
            .get_tags_by_predicate(|tag| matcher.is_match(tag))?
        {
            // Take the entry's new name from the entry itself. `into_new_tags()`
            // only collects what the note replacements below produce, so a tag
            // no note carries would be removed here and never registered again.
            matcher.replace_with_fn(&tag.name, |cap| {
                old_to_new_names
                    .get(&UniCase::new(cap.to_string()))
                    .unwrap()
                    .clone()
            });
            self.remove_single_tag_undoable(tag)?;
        }

        // replace tags
        for mut note in matched_notes {
            let original = note.clone();
            note.tags = matcher.replace_with_fn(&note.tags, |cap| {
                old_to_new_names
                    .get(&UniCase::new(cap.to_string()))
                    .unwrap()
                    .clone()
            });
            note.set_modified(usn);
            self.update_note_tags_undoable(&note, original)?;
        }

        // update tag list
        for tag in matcher.into_new_tags() {
            self.register_tag_string(tag, usn)?;
        }

        Ok(match_count)
    }
}

fn old_to_new_names(
    tags_to_reparent: &[String],
    new_parent: Option<String>,
) -> HashMap<UniCase<String>, String> {
    tags_to_reparent
        .iter()
        // generate resulting names and filter out invalid ones
        .flat_map(|source_tag| {
            reparented_name(source_tag, new_parent.as_deref())
                .map(|output_name| (UniCase::new(source_tag.to_owned()), output_name))
        })
        .collect()
}

/// Arguments are expected in 'human' form with a :: separator.
/// Returns None if new parent is a child of the tag to be reparented.
fn reparented_name(existing_name: &str, new_parent: Option<&str>) -> Option<String> {
    let existing_base = existing_name.rsplit("::").next().unwrap();
    let existing_root = existing_name.split("::").next().unwrap();
    if let Some(new_parent) = new_parent {
        let new_parent_root = new_parent.split("::").next().unwrap();
        if new_parent.starts_with(existing_name) && new_parent_root == existing_root {
            // foo onto foo::bar, or foo onto itself -> no-op
            None
        } else {
            // foo::bar onto baz -> baz::bar
            let new_name = format!("{new_parent}::{existing_base}");
            if new_name != existing_name {
                Some(new_name)
            } else {
                None
            }
        }
    } else {
        // foo::bar onto top level -> bar
        let new_name = existing_base.into();
        if new_name != existing_name {
            Some(new_name)
        } else {
            None
        }
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
    fn reparent_moves_a_subtree_no_note_carries() -> Result<()> {
        let mut col = Collection::new();
        add_unused_tags(&mut col, &["parent::child", "parent::child::leaf"])?;
        assert_eq!(alltags(&col), &["parent::child", "parent::child::leaf"]);

        col.reparent_tags(&["parent::child".into()], Some("target".into()))?;

        assert_eq!(alltags(&col), &["target::child", "target::child::leaf"]);
        Ok(())
    }

    #[test]
    fn reparent_keeps_a_descendant_entry_no_note_carries() -> Result<()> {
        let mut col = Collection::new();
        // A note keeps `parent::child` on it, so notes do match ...
        add_note_with_tag(&mut col, "parent::child")?;
        // ... while `parent::child::leaf` is only a tag-list entry.
        add_unused_tags(&mut col, &["parent::child::leaf"])?;
        assert_eq!(alltags(&col), &["parent::child", "parent::child::leaf"]);

        col.reparent_tags(&["parent::child".into()], Some("target".into()))?;

        // The entry has to be moved, not dropped: it was removed from the tag
        // list with the rest of the subtree.
        assert_eq!(alltags(&col), &["target::child", "target::child::leaf"]);
        Ok(())
    }

    #[test]
    fn reparent_to_the_root_moves_a_subtree_no_note_carries() -> Result<()> {
        let mut col = Collection::new();
        add_unused_tags(&mut col, &["parent::child", "parent::child::leaf"])?;

        col.reparent_tags(&["parent::child".into()], None)?;

        assert_eq!(alltags(&col), &["child", "child::leaf"]);
        Ok(())
    }

    #[test]
    fn reparenting_nothing_leaves_the_tag_list_alone() -> Result<()> {
        let mut col = Collection::new();
        add_unused_tags(&mut col, &["parent::child"])?;

        // a tag that is not in the collection at all
        let count = col
            .reparent_tags(&["absent".into()], Some("target".into()))?
            .output;

        assert_eq!(count, 0);
        assert_eq!(alltags(&col), &["parent::child"]);
        Ok(())
    }

    #[test]
    fn reparent_returns_the_number_of_changed_notes_not_tags() -> Result<()> {
        let mut col = Collection::new();
        add_unused_tags(&mut col, &["parent::child", "parent::child::leaf"])?;

        // The count is a note count, and no note changed.
        let count = col
            .reparent_tags(&["parent::child".into()], Some("target".into()))?
            .output;

        assert_eq!(count, 0);
        assert_eq!(alltags(&col), &["target::child", "target::child::leaf"]);
        Ok(())
    }

    #[test]
    fn dragdrop() -> Result<()> {
        let mut col = Collection::new();
        let nt = col.get_notetype_by_name("Basic")?.unwrap();
        for tag in &[
            "a",
            "ab",
            "another",
            "parent1::child1::grandchild1",
            "parent1::child1",
            "parent1",
            "parent2",
            "yet::another",
        ] {
            let mut note = nt.new_note();
            note.tags.push(tag.to_string());
            col.add_note(&mut note, DeckId(1))?;
        }

        // two decks with the same base name; they both get mapped
        // to parent1::another
        col.reparent_tags(
            &["another".to_string(), "yet::another".to_string()],
            Some("parent1".to_string()),
        )?;

        assert_eq!(
            alltags(&col),
            &[
                "a",
                "ab",
                "parent1",
                "parent1::another",
                "parent1::child1",
                "parent1::child1::grandchild1",
                "parent2",
            ]
        );

        // child and children moved to parent2
        col.reparent_tags(
            &["parent1::child1".to_string()],
            Some("parent2".to_string()),
        )?;

        assert_eq!(
            alltags(&col),
            &[
                "a",
                "ab",
                "parent1",
                "parent1::another",
                "parent2",
                "parent2::child1",
                "parent2::child1::grandchild1",
            ]
        );

        // empty target reparents to root
        col.reparent_tags(&["parent1::another".to_string()], None)?;

        assert_eq!(
            alltags(&col),
            &[
                "a",
                "ab",
                "another",
                "parent1",
                "parent2",
                "parent2::child1",
                "parent2::child1::grandchild1",
            ]
        );

        // parent1 onto parent1::child1 -> no-op
        col.reparent_tags(
            &["parent1".to_string()],
            Some("parent1::child1".to_string()),
        )?;

        assert_eq!(
            alltags(&col),
            &[
                "a",
                "ab",
                "another",
                "parent1",
                "parent2",
                "parent2::child1",
                "parent2::child1::grandchild1",
            ]
        );

        // tags that are prefixes of the new parent are handled correctly
        col.reparent_tags(&["a".to_string()], Some("ab".to_string()))?;

        assert_eq!(
            alltags(&col),
            &[
                "ab",
                "ab::a",
                "another",
                "parent1",
                "parent2",
                "parent2::child1",
                "parent2::child1::grandchild1",
            ]
        );

        // grandchildren can be reparented under the same root
        col.reparent_tags(
            &["parent2::child1::grandchild1".to_string()],
            Some("parent2".to_string()),
        )?;

        assert_eq!(
            alltags(&col),
            &[
                "ab",
                "ab::a",
                "another",
                "parent1",
                "parent2",
                "parent2::child1",
                "parent2::grandchild1",
            ]
        );

        Ok(())
    }
}
