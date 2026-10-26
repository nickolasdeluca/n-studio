//! Info requests: `op_info_sql` (statement plan) and `op_info_database`
//! (performance counters). Both replies are the same clumplet stream:
//! `[item: u8][length: u16 le][value]`, repeated, closed by `isc_info_end`.

use bytes::Bytes;
use rsfbclient_core::{ibase, FbError};
use std::collections::BTreeMap;

use crate::util::*;

/// Database info items. `rsfbclient-core` carries the statement items but
/// none of these; values from Firebird's `ibase.h`.
pub mod db_item {
    pub const READS: u8 = 5;
    pub const WRITES: u8 = 6;
    pub const FETCHES: u8 = 7;
    pub const MARKS: u8 = 8;
    pub const PAGE_SIZE: u8 = 14;
    pub const NUM_BUFFERS: u8 = 15;
    pub const CURRENT_MEMORY: u8 = 17;
    pub const MAX_MEMORY: u8 = 18;
    pub const READ_SEQ_COUNT: u8 = 23;
    pub const READ_IDX_COUNT: u8 = 24;
    pub const INSERT_COUNT: u8 = 25;
    pub const UPDATE_COUNT: u8 = 26;
    pub const DELETE_COUNT: u8 = 27;
    pub const BACKOUT_COUNT: u8 = 28;
    pub const PURGE_COUNT: u8 = 29;
    pub const EXPUNGE_COUNT: u8 = 30;
}

/// Every item `DatabaseCounters` is built from, in one request.
pub const COUNTER_ITEMS: &[u8] = &[
    db_item::READS,
    db_item::WRITES,
    db_item::FETCHES,
    db_item::MARKS,
    db_item::PAGE_SIZE,
    db_item::NUM_BUFFERS,
    db_item::CURRENT_MEMORY,
    db_item::MAX_MEMORY,
    db_item::READ_SEQ_COUNT,
    db_item::READ_IDX_COUNT,
    db_item::INSERT_COUNT,
    db_item::UPDATE_COUNT,
    db_item::DELETE_COUNT,
    db_item::BACKOUT_COUNT,
    db_item::PURGE_COUNT,
    db_item::EXPUNGE_COUNT,
];

/// Splits an info reply into its items, in the order the server sent them.
pub fn parse_info_items(mut data: Bytes) -> Result<Vec<(u8, Bytes)>, FbError> {
    let mut items = Vec::new();

    loop {
        // A reply that fills the buffer exactly can end without isc_info_end.
        if data.remaining() == 0 {
            return Ok(items);
        }

        let item = data.get_u8()?;
        match item as u32 {
            ibase::isc_info_end => return Ok(items),
            ibase::isc_info_truncated => {
                return Err("info reply truncated: it does not fit the requested buffer".into())
            }
            _ => {}
        }

        let len = data.get_u16_le()? as usize;
        if data.remaining() < len {
            return Err("info reply: item length runs past the end of the reply".into());
        }
        let value = data.slice(..len);
        data.advance(len)?;

        // isc_info_error carries the code of the item the server rejected,
        // e.g. one this server version does not know.
        if item as u32 == ibase::isc_info_error {
            return Err(format!("server rejected an info item: {:02x?}", &value[..]).into());
        }

        items.push((item, value));
    }
}

/// Little-endian ("VAX") integer of 1 to 8 bytes, as info values are encoded.
pub fn vax_integer(value: &[u8]) -> Result<i64, FbError> {
    if value.is_empty() || value.len() > 8 {
        return Err(format!("info value of {} bytes is not an integer", value.len()).into());
    }
    let mut n: i64 = 0;
    for (i, b) in value.iter().enumerate() {
        n |= (*b as i64) << (8 * i);
    }
    // Sign-extend from the value's own width.
    let shift = 64 - 8 * value.len() as u32;
    Ok((n << shift) >> shift)
}

/// Per-table record counters for one relation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TableCounters {
    pub seq_reads: i64,
    pub idx_reads: i64,
    pub inserts: i64,
    pub updates: i64,
    pub deletes: i64,
    pub backouts: i64,
    pub purges: i64,
    pub expunges: i64,
}

impl TableCounters {
    fn field(&mut self, item: u8) -> Option<&mut i64> {
        Some(match item {
            db_item::READ_SEQ_COUNT => &mut self.seq_reads,
            db_item::READ_IDX_COUNT => &mut self.idx_reads,
            db_item::INSERT_COUNT => &mut self.inserts,
            db_item::UPDATE_COUNT => &mut self.updates,
            db_item::DELETE_COUNT => &mut self.deletes,
            db_item::BACKOUT_COUNT => &mut self.backouts,
            db_item::PURGE_COUNT => &mut self.purges,
            db_item::EXPUNGE_COUNT => &mut self.expunges,
            _ => return None,
        })
    }

    fn is_zero(&self) -> bool {
        *self == Self::default()
    }
}

/// The attachment's performance counters, as `isc_database_info` reports
/// them. Page and record counts are cumulative for the attachment: what one
/// statement cost is the difference of two snapshots (`since`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DatabaseCounters {
    /// Pages read from disk.
    pub reads: i64,
    /// Pages written to disk.
    pub writes: i64,
    /// Pages fetched from the page cache.
    pub fetches: i64,
    /// Pages marked dirty in the cache.
    pub marks: i64,
    pub page_size: i64,
    /// Page buffers allocated to the cache.
    pub num_buffers: i64,
    /// Server memory in use, bytes. Not cumulative.
    pub current_memory: i64,
    /// Peak server memory, bytes. Not cumulative.
    pub max_memory: i64,
    /// Record counters by relation id, only for relations with activity.
    pub tables: BTreeMap<u16, TableCounters>,
}

impl DatabaseCounters {
    /// Builds the counters from an `op_info_database` reply to `COUNTER_ITEMS`.
    pub fn parse(data: Bytes) -> Result<Self, FbError> {
        let mut c = Self::default();

        for (item, value) in parse_info_items(data)? {
            let scalar = match item {
                db_item::READS => &mut c.reads,
                db_item::WRITES => &mut c.writes,
                db_item::FETCHES => &mut c.fetches,
                db_item::MARKS => &mut c.marks,
                db_item::PAGE_SIZE => &mut c.page_size,
                db_item::NUM_BUFFERS => &mut c.num_buffers,
                db_item::CURRENT_MEMORY => &mut c.current_memory,
                db_item::MAX_MEMORY => &mut c.max_memory,
                _ => {
                    parse_table_counts(item, &value, &mut c.tables)?;
                    continue;
                }
            };
            *scalar = vax_integer(&value)?;
        }

        Ok(c)
    }

    /// What happened between `before` and `self`: page and record counters
    /// become deltas, memory and configuration keep `self`'s values.
    pub fn since(&self, before: &Self) -> Self {
        let mut tables = BTreeMap::new();
        for (id, after) in &self.tables {
            let b = before.tables.get(id).copied().unwrap_or_default();
            let delta = TableCounters {
                seq_reads: after.seq_reads - b.seq_reads,
                idx_reads: after.idx_reads - b.idx_reads,
                inserts: after.inserts - b.inserts,
                updates: after.updates - b.updates,
                deletes: after.deletes - b.deletes,
                backouts: after.backouts - b.backouts,
                purges: after.purges - b.purges,
                expunges: after.expunges - b.expunges,
            };
            if !delta.is_zero() {
                tables.insert(*id, delta);
            }
        }

        Self {
            reads: self.reads - before.reads,
            writes: self.writes - before.writes,
            fetches: self.fetches - before.fetches,
            marks: self.marks - before.marks,
            page_size: self.page_size,
            num_buffers: self.num_buffers,
            current_memory: self.current_memory,
            max_memory: self.max_memory,
            tables,
        }
    }
}

/// A per-table count item: `[relation id: u16 le][count: i32 le]`, repeated.
///
/// The server writes a count wider than 32 bits as 8 bytes, which makes the
/// entry width ambiguous — fbclient-based tools read 6-byte entries too. A
/// length that is not a multiple of 6 is reported rather than guessed at.
fn parse_table_counts(
    item: u8,
    value: &[u8],
    tables: &mut BTreeMap<u16, TableCounters>,
) -> Result<(), FbError> {
    if value.len() % 6 != 0 {
        return Err(format!(
            "info item {item}: {} bytes is not a whole number of 6-byte table entries \
             (a count past 2^31?)",
            value.len()
        )
        .into());
    }

    for entry in value.chunks_exact(6) {
        let relation_id = u16::from_le_bytes([entry[0], entry[1]]);
        let count = vax_integer(&entry[2..])?;
        let counters = tables.entry(relation_id).or_default();
        match counters.field(item) {
            Some(field) => *field = count,
            None => return Err(format!("info item {item} is not a table counter").into()),
        }
    }

    Ok(())
}

/// The plan text out of an `op_info_sql` reply to one plan item, or `None`
/// when the statement has no plan (DDL, `EXECUTE PROCEDURE`, ...).
/// The classic and the explained plan, from one reply asking for both.
pub fn parse_plans(data: Bytes) -> Result<(Option<Bytes>, Option<Bytes>), FbError> {
    let (mut classic, mut explained) = (None, None);
    for (item, value) in parse_info_items(data)? {
        let value = if value.is_empty() { None } else { Some(value) };
        match item as u32 {
            ibase::isc_info_sql_get_plan => classic = value,
            ibase::isc_info_sql_explain_plan => explained = value,
            _ => {}
        }
    }
    Ok((classic, explained))
}

pub fn parse_plan(data: Bytes) -> Result<Option<Bytes>, FbError> {
    for (item, value) in parse_info_items(data)? {
        if item as u32 == ibase::isc_info_sql_get_plan
            || item as u32 == ibase::isc_info_sql_explain_plan
        {
            return Ok(if value.is_empty() { None } else { Some(value) });
        }
    }
    Ok(None)
}

/// Rows an executed statement touched, from `isc_info_sql_records`
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecordCounts {
    pub selected: i64,
    pub inserted: i64,
    pub updated: i64,
    pub deleted: i64,
}

impl RecordCounts {
    /// The reply nests a second clumplet stream inside its one item.
    pub fn parse(data: Bytes) -> Result<Self, FbError> {
        let mut counts = Self::default();
        for (item, value) in parse_info_items(data)? {
            if item as u32 != ibase::isc_info_sql_records {
                continue;
            }
            for (item, value) in parse_info_items(value)? {
                let n = vax_integer(&value)?;
                match item as u32 {
                    ibase::isc_info_req_select_count => counts.selected = n,
                    ibase::isc_info_req_insert_count => counts.inserted = n,
                    ibase::isc_info_req_update_count => counts.updated = n,
                    ibase::isc_info_req_delete_count => counts.deleted = n,
                    _ => {}
                }
            }
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_counts_parse_the_nested_items() {
        let reply = Bytes::from_static(&[
            23, 29, 0, //
            13, 4, 0, 0, 0, 0, 0, // selected 0
            14, 4, 0, 0, 0, 0, 0, // inserted 0
            15, 4, 0, 0x2c, 0x01, 0, 0, // updated 300
            16, 4, 0, 7, 0, 0, 0, // deleted 7
            1, // end of the nested stream
            1,
        ]);
        let counts = RecordCounts::parse(reply).unwrap();
        assert_eq!(counts, RecordCounts { selected: 0, inserted: 0, updated: 300, deleted: 7 });
    }

    #[test]
    fn vax_integers_sign_extend_from_their_width() {
        assert_eq!(vax_integer(&[0x10, 0x27, 0, 0]).unwrap(), 10_000);
        assert_eq!(vax_integer(&[0xff, 0xff]).unwrap(), -1);
        assert_eq!(vax_integer(&[0x00, 0x40]).unwrap(), 16_384);
        assert!(vax_integer(&[]).is_err());
    }

    #[test]
    fn counters_parse_scalars_and_tables() {
        let reply = Bytes::from_static(&[
            db_item::READS, 4, 0, 0x0a, 0, 0, 0, //
            db_item::PAGE_SIZE, 4, 0, 0x00, 0x40, 0, 0, //
            db_item::READ_IDX_COUNT, 12, 0, //
            0x80, 0x00, 0x05, 0, 0, 0, // relation 128: 5
            0x81, 0x00, 0x07, 0, 0, 0, // relation 129: 7
            1, // isc_info_end
        ]);
        let c = DatabaseCounters::parse(reply).unwrap();
        assert_eq!(c.reads, 10);
        assert_eq!(c.page_size, 16_384);
        assert_eq!(c.tables[&128].idx_reads, 5);
        assert_eq!(c.tables[&129].idx_reads, 7);
    }

    #[test]
    fn since_keeps_only_tables_that_moved() {
        let mut before = DatabaseCounters { fetches: 100, ..Default::default() };
        before.tables.insert(1, TableCounters { seq_reads: 3, ..Default::default() });
        before.tables.insert(2, TableCounters { seq_reads: 9, ..Default::default() });

        let mut after = DatabaseCounters { fetches: 160, max_memory: 42, ..Default::default() };
        after.tables.insert(1, TableCounters { seq_reads: 3, ..Default::default() });
        after.tables.insert(2, TableCounters { seq_reads: 19, ..Default::default() });

        let d = after.since(&before);
        assert_eq!(d.fetches, 60);
        assert_eq!(d.max_memory, 42);
        assert_eq!(d.tables.len(), 1);
        assert_eq!(d.tables[&2].seq_reads, 10);
    }

    #[test]
    fn truncated_and_rejected_items_are_errors() {
        assert!(parse_info_items(Bytes::from_static(&[2])).is_err());
        assert!(parse_info_items(Bytes::from_static(&[3, 1, 0, 26])).is_err());
    }

    #[test]
    fn plan_text_is_extracted() {
        let reply = Bytes::from_static(b"\x16\x0b\x00\nPLAN (T N)\x01");
        assert_eq!(&parse_plan(reply).unwrap().unwrap()[..], b"\nPLAN (T N)");
        assert_eq!(parse_plan(Bytes::from_static(b"\x16\x00\x00\x01")).unwrap(), None);
    }
}
