//! Names and string literals as they are written in SQL.

/// A name as DDL writes it: bare when it is a regular identifier, in double
/// quotes otherwise. Dialect 1 has no quoted identifiers, and a database in
/// it no names that would need them.
pub fn quote(name: &str, dialect: u32) -> String {
    if dialect < 3 || (is_regular(name) && !is_reserved(name)) {
        name.to_string()
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

/// A string literal
pub fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// What the server stores a bare name as: upper case, starting with a letter
fn is_regular(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_' || c == '$')
}

fn is_reserved(name: &str) -> bool {
    RESERVED.binary_search(&name).is_ok()
}

/// The reserved words of Firebird 2.5 through 5, together: a name that is
/// reserved in any of them is quoted, which is right in all of them.
#[rustfmt::skip]
const RESERVED: &[&str] = &[
    "ADD", "ADMIN", "ALL", "ALTER", "AND", "ANY", "AS", "AT", "AVG",
    "BEGIN", "BETWEEN", "BIGINT", "BINARY", "BIT_LENGTH", "BLOB", "BOOLEAN", "BOTH", "BY",
    "CASE", "CAST", "CHAR", "CHARACTER", "CHARACTER_LENGTH", "CHAR_LENGTH", "CHECK", "CLOSE", "COLLATE", "COLUMN",
    "COMMENT", "COMMIT", "CONNECT", "CONSTRAINT", "CORR", "COUNT", "COVAR_POP", "COVAR_SAMP", "CREATE", "CROSS",
    "CURRENT", "CURRENT_CONNECTION", "CURRENT_DATE", "CURRENT_ROLE", "CURRENT_TIME", "CURRENT_TIMESTAMP",
    "CURRENT_TRANSACTION", "CURRENT_USER", "CURSOR",
    "DATE", "DAY", "DEC", "DECFLOAT", "DECIMAL", "DECLARE", "DEFAULT", "DELETE", "DELETING", "DETERMINISTIC",
    "DISCONNECT", "DISTINCT", "DOUBLE", "DROP",
    "ELSE", "END", "ESCAPE", "EXECUTE", "EXISTS", "EXTERNAL", "EXTRACT",
    "FALSE", "FETCH", "FILTER", "FLOAT", "FOR", "FOREIGN", "FROM", "FULL", "FUNCTION",
    "GDSCODE", "GLOBAL", "GRANT", "GROUP",
    "HAVING", "HOUR",
    "IN", "INDEX", "INNER", "INSENSITIVE", "INSERT", "INSERTING", "INT", "INT128", "INTEGER", "INTO", "IS",
    "JOIN",
    "LATERAL", "LEADING", "LEFT", "LIKE", "LOCAL", "LOCALTIME", "LOCALTIMESTAMP", "LONG", "LOWER",
    "MAX", "MAXIMUM_SEGMENT", "MERGE", "MIN", "MINUTE", "MONTH",
    "NATIONAL", "NATURAL", "NCHAR", "NO", "NOT", "NULL", "NUMERIC",
    "OCTET_LENGTH", "OF", "OFFSET", "ON", "ONLY", "OPEN", "OR", "ORDER", "OUTER", "OVER",
    "PARAMETER", "PLAN", "POSITION", "POST_EVENT", "PRECISION", "PRIMARY", "PROCEDURE", "PUBLICATION",
    "RDB$DB_KEY", "RDB$ERROR", "RDB$GET_CONTEXT", "RDB$GET_TRANSACTION_CN", "RDB$RECORD_VERSION",
    "RDB$ROLE_IN_USE", "RDB$SET_CONTEXT", "RDB$SYSTEM_PRIVILEGE", "REAL", "RECORD_VERSION", "RECREATE",
    "RECURSIVE", "REFERENCES", "REGR_AVGX", "REGR_AVGY", "REGR_COUNT", "REGR_INTERCEPT", "REGR_R2",
    "REGR_SLOPE", "REGR_SXX", "REGR_SXY", "REGR_SYY", "RELEASE", "RESETTING", "RETURN", "RETURNING_VALUES",
    "RETURNS", "REVOKE", "RIGHT", "ROLLBACK", "ROW", "ROWS", "ROW_COUNT",
    "SAVEPOINT", "SCROLL", "SECOND", "SELECT", "SENSITIVE", "SET", "SIMILAR", "SMALLINT", "SOME", "SQLCODE",
    "SQLSTATE", "START", "STDDEV_POP", "STDDEV_SAMP", "SUM",
    "TABLE", "THEN", "TIME", "TIMESTAMP", "TIMEZONE_HOUR", "TIMEZONE_MINUTE", "TO", "TRAILING", "TRIGGER",
    "TRIM", "TRUE",
    "UNBOUNDED", "UNION", "UNIQUE", "UNKNOWN", "UPDATE", "UPDATING", "UPPER", "USER", "USING",
    "VALUE", "VALUES", "VARBINARY", "VARCHAR", "VARIABLE", "VARYING", "VAR_POP", "VAR_SAMP", "VIEW",
    "WHEN", "WHERE", "WHILE", "WINDOW", "WITH", "WITHOUT",
    "YEAR",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserved_words_are_sorted() {
        // They are searched by bisection
        assert!(RESERVED.windows(2).all(|w| w[0] < w[1]), "not sorted, or a word twice");
    }

    #[test]
    fn quotes_what_would_not_read_back() {
        assert_eq!(quote("PRODUTO", 3), "PRODUTO");
        assert_eq!(quote("RDB$RELATIONS", 3), "RDB$RELATIONS");
        assert_eq!(quote("Produto", 3), "\"Produto\"");
        assert_eq!(quote("ITEM VENDA", 3), "\"ITEM VENDA\"");
        assert_eq!(quote("1A", 3), "\"1A\"");
        assert_eq!(quote("VALUE", 3), "\"VALUE\"");
        assert_eq!(quote("A\"B", 3), "\"A\"\"B\"");
        assert_eq!(quote("VALUE", 1), "VALUE");
        assert_eq!(literal("it's"), "'it''s'");
    }
}
