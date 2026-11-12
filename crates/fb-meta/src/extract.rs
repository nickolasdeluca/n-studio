//! An object's DDL, read from a live database.

use fb_core::{Connection, Result};

use crate::ddl::{self, Context, Statement};
use crate::table::TriggerScope;
use crate::{object, routine, table, ObjectKind};

/// The statements that would create one object again. A table comes with
/// its constraints, indices and triggers; a generator with its value.
pub fn statements(conn: &Connection, kind: ObjectKind, name: &str) -> Result<Vec<Statement>> {
    let ctx = Context::of(conn.info());
    Ok(match kind {
        ObjectKind::Table => {
            let mut statements = ddl::table(&table::table(conn, name)?, &ctx);
            for trigger in table::triggers(conn, TriggerScope::On(name), true)? {
                statements.extend(ddl::trigger(&trigger, &ctx));
            }
            statements
        }
        ObjectKind::View => {
            let mut statements = ddl::view(&table::view(conn, name)?, &ctx);
            for trigger in table::triggers(conn, TriggerScope::On(name), true)? {
                statements.extend(ddl::trigger(&trigger, &ctx));
            }
            statements
        }
        ObjectKind::Trigger => ddl::trigger(&table::trigger(conn, name)?, &ctx),
        ObjectKind::Index => ddl::index(&table::index(conn, name)?, &ctx),
        ObjectKind::Procedure => ddl::routine(&routine::procedure(conn, name)?, &ctx),
        ObjectKind::Function => ddl::routine(&routine::function(conn, name)?, &ctx),
        ObjectKind::Package => ddl::package(&routine::package(conn, name)?, &ctx),
        ObjectKind::Udf => ddl::udf(&routine::udf(conn, name)?, &ctx),
        ObjectKind::Domain => ddl::domain(&object::domain(conn, name)?, &ctx),
        ObjectKind::Generator => ddl::generator(&object::generator(conn, name)?, &ctx),
        ObjectKind::Exception => ddl::exception(&object::exception(conn, name)?, &ctx),
        ObjectKind::Role => ddl::role(&object::role(conn, name)?, &ctx),
    })
}

/// One object's DDL as a script
pub fn script(conn: &Connection, kind: ObjectKind, name: &str) -> Result<String> {
    Ok(ddl::script(&statements(conn, kind, name)?))
}
