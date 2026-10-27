use super::*;
use crate::api::{EntryKind, TreeEntry, TreeOf};

fn tree(h: &Harness, object: TreeOf, page: u32, limit: u32) -> (Box<[TreeEntry]>, bool) {
    let Answer::GitTree { entries, truncated } = h.inspect(&Read::GitTree { object, page, limit }).unwrap() else {
        unreachable!("tree entries")
    };
    (entries, truncated)
}
fn rooted() -> Harness {
    let mut configured = setup();
    configured.tree =
        files(&[(b"z.txt", b"last"), (b"docs/a.md", b"a"), (b"docs/nest/b.md", b"b"), (b"a.txt", b"first")]);
    Harness::with(CALM, configured)
}
#[test]
fn a_root_tree_lists_files_and_directories_in_name_order() {
    let h = rooted();
    let (first, more) = tree(&h, TreeOf::Commit(FIRST), 1, 2);
    assert_eq!((&*first[0].name, first[0].kind, first[0].mode), (b"a.txt".as_slice(), EntryKind::Blob, 0o100_644));
    assert_eq!((&*first[1].name, first[1].kind, first[1].mode), (b"docs".as_slice(), EntryKind::Tree, 0o040_000));
    assert!(more);
    let (second, more) = tree(&h, TreeOf::Commit(FIRST), 2, 2);
    assert_eq!(&*second[0].name, b"z.txt");
    assert!(!more);
}
#[test]
fn a_subtree_is_read_by_its_id() {
    let h = rooted();
    let (root, _) = tree(&h, TreeOf::Commit(FIRST), 1, 2);
    let (docs, more) = tree(&h, TreeOf::Tree(root[1].object), 1, 2);
    assert!(!more);
    assert_eq!(&*docs[0].name, b"a.md");
    assert_eq!((&*docs[1].name, docs[1].kind), (b"nest".as_slice(), EntryKind::Tree));
    let (nested, _) = tree(&h, TreeOf::Tree(docs[1].object), 1, 2);
    assert_eq!(&*nested[0].name, b"b.md");
    assert_eq!(nested[0].kind, EntryKind::Blob);
    assert_eq!(
        h.inspect(&Read::GitTree { object: TreeOf::Tree([0; 20]), page: 1, limit: 1 }),
        Err(Error::Missing(What::File))
    );
    assert_eq!(
        h.inspect(&Read::GitTree { object: TreeOf::Commit(99), page: 1, limit: 1 }),
        Err(Error::Missing(What::Commit))
    );
}
#[test]
fn equal_directories_have_equal_ids_across_commits() {
    let mut h = rooted();
    let (before, _) = tree(&h, TreeOf::Commit(FIRST), 1, 2);
    let (docs, _) = tree(&h, TreeOf::Tree(before[1].object), 1, 2);
    let head = crate::advance(&mut h.domain, &h.env, REPOSITORY, MAIN, b"docs/a.md", b"new", ENGINE).unwrap();
    let (after, _) = tree(&h, TreeOf::Commit(head), 1, 2);
    let (new_docs, _) = tree(&h, TreeOf::Tree(after[1].object), 1, 2);
    assert_eq!(before[0].object, after[0].object);
    assert_ne!(before[1].object, after[1].object);
    assert_eq!(docs[1].object, new_docs[1].object);
    assert_eq!(tree(&h, TreeOf::Commit(FIRST), 2, 2), tree(&h, TreeOf::Commit(head), 2, 2));
    let mut configured = setup();
    configured.id = 3;
    configured.name = copy_of(b"ai/other");
    configured.tree = files(&[(b"same/b.md", b"b")]);
    let first = crate::repository(&mut h.domain, &CALM, configured);
    let Answer::GitTree { entries, .. } = h
        .domain
        .inspect(&CALM, b"ai/other", &Read::GitTree { object: TreeOf::Commit(first), page: 1, limit: 2 })
        .unwrap()
    else {
        unreachable!("tree")
    };
    assert_eq!(entries[0].object, docs[1].object, "directory identity depends only on entries");
}
#[test]
fn a_tree_is_paged_with_truncated() {
    let h = rooted();
    for (page, name, truncated) in [
        (0, Some(b"a.txt".as_slice()), true),
        (1, Some(b"a.txt".as_slice()), true),
        (2, Some(b"docs".as_slice()), true),
        (3, Some(b"z.txt".as_slice()), false),
        (4, None, false),
    ] {
        let (entries, more) = tree(&h, TreeOf::Commit(FIRST), page, 1);
        if let Some(name) = name {
            assert_eq!(&*entries[0].name, name);
        } else {
            assert!(entries.is_empty());
        }
        assert_eq!(more, truncated);
    }
}
