//! Prints an object's DDL and its dependencies, from the database the
//! `FB_*` variables name: `cargo run -p fb-meta --example ddl -- table TIPOS`

use std::time::Instant;

use fb_core::{ConnectParams, Connection};
use fb_meta::{dependencies, extract, ObjectKind};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let (Some(kind), Some(name)) = (args.next(), args.next()) else {
        return Err("usage: ddl <kind> <name>".into());
    };
    let kind = ObjectKind::ALL
        .into_iter()
        .find(|k| format!("{k:?}").eq_ignore_ascii_case(&kind))
        .ok_or_else(|| format!("unknown kind {kind:?}"))?;
    let params = ConnectParams::from_env().ok_or("set FB_HOST, FB_DB, FB_USER and FB_PASS")?;
    let conn = Connection::open(&params)?;

    let t = Instant::now();
    print!("{}", extract::script(&conn, kind, &name)?);
    eprintln!("-- DDL in {:?}", t.elapsed());

    let t = Instant::now();
    let deps = dependencies(&conn, kind, &name)?;
    eprintln!("-- dependencies in {:?}", t.elapsed());
    for (title, list) in [("uses", &deps.uses), ("used by", &deps.used_by)] {
        println!("\n-- {title}: {}", list.len());
        for d in list {
            let detail = d.detail.as_ref().map(|x| format!(".{x}")).unwrap_or_default();
            println!("--   {:<16} {}{detail} {}", d.what, d.name, d.fields.join(", "));
        }
    }
    Ok(())
}
