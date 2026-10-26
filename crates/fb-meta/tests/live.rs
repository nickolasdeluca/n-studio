//! Against a live server, named by the `FB_*` environment variables (see
//! `ConnectParams::from_env`). Skipped without them. With `FB_FIXTURE=1` the
//! database is taken to be the one docker/seed builds, and checked object by
//! object.

use fb_core::{ConnectParams, Connection};
use fb_meta::{counts, objects, DbObject, Generation, ObjectKind};

fn connect() -> Option<Connection> {
    let Some(params) = ConnectParams::from_env() else {
        eprintln!("FB_DB not set: skipping live test");
        return None;
    };
    Some(Connection::open(&params).expect("connect"))
}

fn names(objects: &[DbObject]) -> Vec<&str> {
    objects.iter().map(|o| o.name.as_str()).collect()
}

#[test]
fn counts_agree_with_the_lists() {
    let Some(conn) = connect() else { return };
    let counts = counts(&conn).unwrap();
    let kinds: Vec<_> = Generation::of(conn.info()).kinds().collect();
    assert_eq!(counts.iter().map(|(k, _)| *k).collect::<Vec<_>>(), kinds);

    for (kind, count) in counts {
        let list = objects(&conn, kind).unwrap();
        assert_eq!(list.len() as u64, count, "{kind:?}");
        let mut sorted = names(&list);
        sorted.sort();
        assert_eq!(names(&list), sorted, "{kind:?} comes back by name");
        for o in &list {
            assert!(!o.name.is_empty() && o.name.trim() == o.name, "{kind:?} {o:?}");
            assert!(!o.name.starts_with("RDB$") && !o.name.starts_with("MON$") || kind == ObjectKind::Index,
                "{kind:?}: system object {o:?}");
        }
    }
}

#[test]
fn the_fixture_lists_what_it_seeded() {
    if std::env::var("FB_FIXTURE").as_deref() != Ok("1") {
        return;
    }
    let Some(conn) = connect() else { return };
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let major = conn.info().major;
    let list = |kind| objects(&conn, kind).unwrap();

    assert_eq!(names(&list(ObjectKind::Domain)), ["D_FLAG", "D_ID", "D_MONEY"]);
    let mut tables = vec!["BULK_ROWS", "TIPOS"];
    if major >= 3 {
        tables.push("TIPOS_FB3");
    }
    if major >= 4 {
        tables.push("TIPOS_FB4");
    }
    assert_eq!(names(&list(ObjectKind::Table)), tables);
    assert_eq!(names(&list(ObjectKind::View)), ["V_TIPOS"]);
    assert_eq!(names(&list(ObjectKind::Procedure)), ["P_FIXTURE", "P_SEED_BULK"]);
    // The identity column's sequence is the engine's, not the user's
    assert_eq!(names(&list(ObjectKind::Generator)), ["GEN_TIPOS_ID"]);
    assert_eq!(names(&list(ObjectKind::Exception)), ["E_FIXTURE"]);
    assert_eq!(
        list(ObjectKind::Trigger),
        [DbObject { name: "TIPOS_BI".into(), parent: Some("TIPOS".into()), description: None }]
    );
    let indices = list(ObjectKind::Index);
    let idx = indices.iter().find(|o| o.name == "IDX_BULK_NOME").expect("IDX_BULK_NOME");
    assert_eq!(idx.parent.as_deref(), Some("BULK_ROWS"));
    if fb3 {
        assert_eq!(names(&list(ObjectKind::Package)), ["PKG_FIXTURE"]);
        // PKG_FIXTURE.DOBRO belongs under its package
        assert_eq!(names(&list(ObjectKind::Function)), Vec::<&str>::new());
    }
}
