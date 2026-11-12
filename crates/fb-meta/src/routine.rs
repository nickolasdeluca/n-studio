//! Procedures, stored functions, packages and external functions (UDFs).

use fb_core::{Connection, Result};
use serde::Serialize;

use crate::row::{inline, rows, whole};
use crate::table::{not_found, unexpected};
use crate::types::{type_joins, DataType, TYPE_COLUMNS};
use crate::Generation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineKind {
    Procedure,
    Function,
}

/// A parameter, or a function's result
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    /// Empty for a function's result
    pub name: String,
    pub data_type: DataType,
    /// The domain it is declared with, when that is a domain of the user's
    pub domain: Option<String>,
    /// Declared `TYPE OF <domain>`: the domain's type without its checks
    pub type_of: bool,
    /// Declared `TYPE OF COLUMN <table>.<column>`
    pub type_of_column: Option<(String, String)>,
    pub not_null: bool,
    /// As written: `= 0` or `DEFAULT 0`
    pub default: Option<String>,
    pub description: Option<String>,
}

/// A procedure or a stored function
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub name: String,
    pub kind: RoutineKind,
    pub inputs: Vec<Param>,
    /// A procedure's output parameters; a function's one result
    pub outputs: Vec<Param>,
    /// The body, from after `AS`. `None` for an external routine, and for
    /// one whose source was removed from the database.
    pub source: Option<String>,
    pub deterministic: bool,
    /// An external routine's `ENGINE` and `EXTERNAL NAME`
    pub external: Option<(String, Option<String>)>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Package {
    pub name: String,
    /// From after `AS`
    pub header: Option<String>,
    pub body: Option<String>,
    pub description: Option<String>,
}

/// An external function, declared with DECLARE EXTERNAL FUNCTION
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Udf {
    pub name: String,
    pub module: Option<String>,
    pub entry_point: Option<String>,
    /// The inputs, in order
    pub arguments: Vec<UdfArgument>,
    pub returns: Option<UdfArgument>,
    /// The result comes back in this input (`RETURNS PARAMETER n`)
    pub returns_parameter: Option<i64>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UdfArgument {
    pub data_type: DataType,
    /// How it is passed: 0 by value, 1 by reference, 2 by descriptor, 3 a
    /// blob structure, 4 a scalar array, 5 by reference and nullable.
    /// Negative: the engine frees the result (FREE_IT).
    pub mechanism: i64,
}

pub fn procedure(conn: &Connection, name: &str) -> Result<Routine> {
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let (external, standalone) = if fb3 {
        ("trim(p.rdb$engine_name), trim(p.rdb$entrypoint)", "and p.rdb$package_name is null")
    } else {
        ("cast(null as char(1)), cast(null as char(1))", "")
    };
    let sql = format!(
        "select p.rdb$procedure_source, {description}, {external} \
         from rdb$procedures p where p.rdb$procedure_name = ? {standalone}",
        description = inline("p.rdb$description"),
    );
    let row = rows(conn, &sql, vec![name.into()])?.pop().ok_or_else(|| not_found("procedure", name))?;
    let description =
        whole(conn, row.lob(1), "select rdb$description from rdb$procedures where rdb$procedure_name = ?", &[name])?;

    let standalone = if fb3 { "and a.rdb$package_name is null" } else { "" };
    let params = format!(
        "select trim(a.rdb$parameter_name), a.rdb$parameter_type, trim(a.rdb$field_source), \
                a.rdb$parameter_mechanism, a.rdb$null_flag, {default}, {description}, \
                trim(a.rdb$relation_name), trim(a.rdb$field_name), {TYPE_COLUMNS} \
         from rdb$procedure_parameters a \
         join rdb$fields f on f.rdb$field_name = a.rdb$field_source \
         {type_joins} \
         where a.rdb$procedure_name = ? {standalone} \
         order by a.rdb$parameter_type, a.rdb$parameter_number",
        default = inline("a.rdb$default_source"),
        description = inline("a.rdb$description"),
        type_joins = type_joins("coalesce(a.rdb$collation_id, f.rdb$collation_id)"),
    );
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for row in rows(conn, &params, vec![name.into()])? {
        let param_name = row.text(0).ok_or_else(|| unexpected("parameter"))?;
        let of_param = |column: &str| {
            format!(
                "select a.{column} from rdb$procedure_parameters a \
                 where a.rdb$procedure_name = ? and a.rdb$parameter_name = ? {standalone}"
            )
        };
        let source = row.text(2).unwrap_or_default();
        let param = Param {
            data_type: DataType::read(&row, 9),
            type_of: row.int(3) == Some(1),
            type_of_column: row.text(7).zip(row.text(8)),
            not_null: row.flag(4),
            default: whole(conn, row.lob(5), &of_param("rdb$default_source"), &[name, param_name.as_str()])?,
            description: whole(conn, row.lob(6), &of_param("rdb$description"), &[name, param_name.as_str()])?,
            domain: (!source.starts_with("RDB$")).then_some(source),
            name: param_name,
        };
        if row.int(1) == Some(1) {
            outputs.push(param);
        } else {
            inputs.push(param);
        }
    }
    Ok(Routine {
        name: name.to_string(),
        kind: RoutineKind::Procedure,
        inputs,
        outputs,
        source: row.text(0),
        deterministic: false,
        external: row.text(2).map(|engine| (engine, row.text(3))),
        description,
    })
}

/// A stored function (Firebird 3 and later)
pub fn function(conn: &Connection, name: &str) -> Result<Routine> {
    let sql = format!(
        "select fn.rdb$function_source, {description}, trim(fn.rdb$engine_name), trim(fn.rdb$entrypoint), \
                fn.rdb$deterministic_flag, fn.rdb$return_argument \
         from rdb$functions fn where fn.rdb$function_name = ? and fn.rdb$package_name is null",
        description = inline("fn.rdb$description"),
    );
    let row = rows(conn, &sql, vec![name.into()])?.pop().ok_or_else(|| not_found("function", name))?;
    let description = whole(
        conn,
        row.lob(1),
        "select rdb$description from rdb$functions where rdb$function_name = ? and rdb$package_name is null",
        &[name],
    )?;
    let returns = row.int(5).unwrap_or(0);

    let args = format!(
        "select trim(a.rdb$argument_name), a.rdb$argument_position, trim(a.rdb$field_source), \
                a.rdb$argument_mechanism, a.rdb$null_flag, {default}, {description}, \
                trim(a.rdb$relation_name), trim(a.rdb$field_name), {TYPE_COLUMNS} \
         from rdb$function_arguments a \
         join rdb$fields f on f.rdb$field_name = a.rdb$field_source \
         {type_joins} \
         where a.rdb$function_name = ? and a.rdb$package_name is null \
         order by a.rdb$argument_position",
        default = inline("a.rdb$default_source"),
        description = inline("a.rdb$description"),
        type_joins = type_joins("coalesce(a.rdb$collation_id, f.rdb$collation_id)"),
    );
    let (mut inputs, mut outputs) = (Vec::new(), Vec::new());
    for row in rows(conn, &args, vec![name.into()])? {
        let position = row.int(1).unwrap_or(0);
        let position_text = position.to_string();
        let of_arg = |column: &str| {
            format!(
                "select a.{column} from rdb$function_arguments a \
                 where a.rdb$function_name = ? and a.rdb$argument_position = cast(? as integer) \
                   and a.rdb$package_name is null"
            )
        };
        let source = row.text(2).unwrap_or_default();
        let param = Param {
            name: row.text(0).unwrap_or_default(),
            data_type: DataType::read(&row, 9),
            type_of: row.int(3) == Some(1),
            type_of_column: row.text(7).zip(row.text(8)),
            not_null: row.flag(4),
            default: whole(conn, row.lob(5), &of_arg("rdb$default_source"), &[name, position_text.as_str()])?,
            description: whole(conn, row.lob(6), &of_arg("rdb$description"), &[name, position_text.as_str()])?,
            domain: (!source.starts_with("RDB$")).then_some(source),
        };
        if position == returns {
            outputs.push(param);
        } else {
            inputs.push(param);
        }
    }
    Ok(Routine {
        name: name.to_string(),
        kind: RoutineKind::Function,
        inputs,
        outputs,
        source: row.text(0),
        deterministic: row.flag(4),
        external: row.text(2).map(|engine| (engine, row.text(3))),
        description,
    })
}

/// A package (Firebird 3 and later)
pub fn package(conn: &Connection, name: &str) -> Result<Package> {
    let sql = format!(
        "select p.rdb$package_header_source, p.rdb$package_body_source, {description} \
         from rdb$packages p where p.rdb$package_name = ?",
        description = inline("p.rdb$description"),
    );
    let row = rows(conn, &sql, vec![name.into()])?.pop().ok_or_else(|| not_found("package", name))?;
    let description =
        whole(conn, row.lob(2), "select rdb$description from rdb$packages where rdb$package_name = ?", &[name])?;
    Ok(Package { name: name.to_string(), header: row.text(0), body: row.text(1), description })
}

/// An external function. Its arguments' types are kept with the arguments,
/// not in `rdb$fields`.
pub fn udf(conn: &Connection, name: &str) -> Result<Udf> {
    let fb3 = Generation::of(conn.info()) == Generation::Fb3;
    let legacy = if fb3 { "and fn.rdb$legacy_flag = 1" } else { "" };
    let sql = format!(
        "select trim(fn.rdb$module_name), trim(fn.rdb$entrypoint), fn.rdb$return_argument, {description} \
         from rdb$functions fn where fn.rdb$function_name = ? {legacy}",
        description = inline("fn.rdb$description"),
    );
    let row = rows(conn, &sql, vec![name.into()])?.pop().ok_or_else(|| not_found("external function", name))?;
    let description =
        whole(conn, row.lob(3), "select rdb$description from rdb$functions where rdb$function_name = ?", &[name])?;
    let returns_at = row.int(2).unwrap_or(0);

    let standalone = if fb3 { "and f.rdb$package_name is null" } else { "" };
    let args = format!(
        "select f.rdb$argument_position, f.rdb$mechanism, f.rdb$field_type, f.rdb$field_sub_type, \
                f.rdb$field_length, f.rdb$field_precision, f.rdb$field_scale, f.rdb$character_length, \
                trim(cs.rdb$character_set_name), cs.rdb$bytes_per_character \
         from rdb$function_arguments f \
         left join rdb$character_sets cs on cs.rdb$character_set_id = f.rdb$character_set_id \
         where f.rdb$function_name = ? {standalone} \
         order by f.rdb$argument_position"
    );
    let mut arguments = Vec::new();
    let mut returns = None;
    for row in rows(conn, &args, vec![name.into()])? {
        let int = |i: usize| row.int(i).unwrap_or(0);
        let length = int(4);
        let mut data_type = DataType {
            code: int(2),
            sub_type: int(3),
            length,
            precision: int(5),
            scale: int(6),
            char_length: match int(7) {
                0 => length / int(9).max(1),
                n => n,
            },
            ..Default::default()
        };
        if data_type.is_text() {
            data_type.charset = row.text(8);
        }
        let argument = UdfArgument { data_type, mechanism: int(1) };
        // Position 0 is a result of its own; otherwise the result is one
        // of the inputs, which stays in the list
        if int(0) == 0 && returns_at == 0 {
            returns = Some(argument);
        } else {
            if int(0) == returns_at {
                returns = Some(argument.clone());
            }
            arguments.push(argument);
        }
    }
    Ok(Udf {
        name: name.to_string(),
        module: row.text(0),
        entry_point: row.text(1),
        arguments,
        returns,
        returns_parameter: (returns_at != 0).then_some(returns_at),
        description,
    })
}
