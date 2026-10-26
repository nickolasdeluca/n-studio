//! Prints the explorer tree's counts for the database the `FB_*` variables
//! name, and the objects of one kind if given:
//! `cargo run -p fb-meta --example tree -- [table|view|…]`

use std::time::Instant;

use fb_core::{ConnectParams, Connection};
use fb_meta::{counts, objects, ObjectKind};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let params = ConnectParams::from_env().ok_or("set FB_HOST, FB_DB, FB_USER and FB_PASS")?;
    let conn = Connection::open(&params)?;
    let info = conn.info();
    println!("Firebird {} — ODS {}.{}, dialect {}, {}", info.engine_version, info.ods_major, info.ods_minor, info.dialect, info.charset);

    let t = Instant::now();
    let counts = counts(&conn)?;
    println!("counts in {:?}", t.elapsed());
    for (kind, n) in &counts {
        println!("  {:<10} {n:>6}", format!("{kind:?}"));
    }

    if let Some(arg) = std::env::args().nth(1) {
        let kind = ObjectKind::ALL
            .into_iter()
            .find(|k| format!("{k:?}").eq_ignore_ascii_case(&arg))
            .ok_or_else(|| format!("unknown kind {arg:?}"))?;
        let t = Instant::now();
        let list = objects(&conn, kind)?;
        println!("{} {kind:?} in {:?}", list.len(), t.elapsed());
        for o in list.iter().take(15) {
            println!("  {:<31} {:<31} {}", o.name, o.parent.as_deref().unwrap_or(""), o.description.as_deref().unwrap_or(""));
        }
    }
    Ok(())
}
