//! The server's services manager: backup, restore, validation, sweep,
//! statistics and the server's log, each run on the server and read as the
//! text it writes. A service is a connection of its own, to the server and
//! not to a database: every path it is given is a path on the server.

use std::sync::atomic::{AtomicBool, Ordering};

use rsfbclient_core::ibase;
use rsfbclient_rust::ServiceClient;
use serde::{Deserialize, Serialize};

use crate::{ConnectParams, Error, Result};

/// What a backup leaves out or does differently (`gbak -b`)
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupOptions {
    /// Only the metadata, no rows
    pub metadata_only: bool,
    /// Do not collect garbage while reading: faster, and the usual choice
    /// before a restore
    pub no_garbage_collect: bool,
    /// Carry on past checksum errors
    pub ignore_checksums: bool,
    /// Carry on past transactions in limbo
    pub ignore_limbo: bool,
    /// A backup only this platform reads
    pub non_transportable: bool,
    /// External tables become tables of the database
    pub convert_external: bool,
}

/// How a database is restored (`gbak -c` / `gbak -r`)
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RestoreOptions {
    /// Write over a database that is there; without it, one must not be
    pub replace: bool,
    /// Indexes are restored inactive
    pub deactivate_indexes: bool,
    /// Shadows are not restored
    pub no_shadows: bool,
    /// Check and not null constraints are not checked on the way in
    pub no_validity: bool,
    /// Each table is committed as it is restored, so what can be saved is
    pub one_at_a_time: bool,
    /// Pages are filled, leaving no room for new record versions
    pub use_all_space: bool,
    /// Page size of the new database; the backup's own without it
    pub page_size: Option<u32>,
}

/// Which pages the statistics look at (`gstat`)
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatisticsOptions {
    /// Only the header page: the transaction counters, the sweep interval
    pub header_only: bool,
    pub data_pages: bool,
    pub index_pages: bool,
    /// Record and version counts: reads every record
    pub record_versions: bool,
    pub system_relations: bool,
}

/// A database setting the services manager changes, as `gfix` does
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "property", content = "value", rename_all = "snake_case")]
pub enum Property {
    /// Transactions between automatic sweeps; 0 turns them off (`gfix -h`)
    SweepInterval(u32),
    /// Each write waits for the disk (`gfix -w sync`) or not (`-w async`)
    ForcedWrites(bool),
    /// The cache this database asks for, in pages; 0 for the server's (`gfix -buffers`)
    PageBuffers(u32),
    /// `gfix -mode read_only` or `read_write`
    ReadOnly(bool),
    /// Room is kept on each page for new record versions (`gfix -use reserve`), or
    /// pages are filled (`-use full`)
    ReserveSpace(bool),
}

/// Who may still be connected to a database shut down
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownMode {
    /// SYSDBA and the owner, as many connections as they like
    Multi,
    /// SYSDBA or the owner, one connection
    Single,
    /// No one: the file can be copied
    Full,
}

/// What the server does with the work still open when it shuts down
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownWait {
    /// Waits the seconds given, then ends every connection (`gfix -shut -force`)
    Force,
    /// Refuses new connections, and fails if any remain after the seconds (`-attach`)
    Attachments,
    /// Refuses new transactions, and fails if any remain after the seconds (`-tran`)
    Transactions,
}

/// Who may connect once a database is brought back online
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlineMode {
    /// Everyone
    Normal,
    Multi,
    Single,
}

/// One thing asked of the services manager
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "task", rename_all = "snake_case")]
pub enum ServiceTask {
    Backup { database: String, file: String, options: BackupOptions },
    Restore { file: String, database: String, options: RestoreOptions },
    /// Looks for corruption and changes nothing. `online` (Firebird 3 and
    /// later) validates a database in use; without it the server wants the
    /// database to itself.
    Validate { database: String, online: bool },
    Sweep { database: String },
    Statistics { database: String, options: StatisticsOptions },
    /// The server's own log (`firebird.log`)
    ServerLog,
    /// Changes the database's settings, each in turn
    Properties { database: String, changes: Vec<Property> },
    /// Takes the database out of use (`gfix -shut`)
    Shutdown { database: String, mode: ShutdownMode, wait: ShutdownWait, seconds: u32 },
    /// Brings a database that was shut down back (`gfix -online`)
    Online { database: String, mode: OnlineMode },
    /// Prepares a corrupt database to be backed up: what cannot be read is
    /// taken out, so it is lost (`gfix -mend -full`)
    Mend { database: String },
    /// A physical backup of the pages changed since the backup of the level
    /// below (`nbackup -b`); level 0 is the whole database. Firebird 2.5+.
    PhysicalBackup { database: String, file: String, level: u32, no_triggers: bool },
    /// A database put back together from a level 0 backup and the levels on
    /// top of it, in order (`nbackup -r`)
    PhysicalRestore { files: Vec<String>, database: String },
}

/// How a service's run ended
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceEnd {
    Finished,
    /// The caller stopped listening. The server is told by the detach; what
    /// it had done by then stays done.
    Cancelled,
}

/// A user as the services manager lists them (Firebird 2.5, which has no
/// `SEC$USERS`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceUser {
    pub name: String,
    pub first_name: String,
    pub middle_name: String,
    pub last_name: String,
}

pub(crate) fn attach(params: &ConnectParams) -> Result<ServiceClient> {
    Ok(ServiceClient::connect(
        &params.host,
        params.port,
        &params.user,
        &params.password,
        params.role.as_deref(),
        params.auth,
    )?)
}

/// A start block: an action, then its arguments
pub(crate) struct Spb(pub(crate) Vec<u8>);

impl Spb {
    pub(crate) fn action(action: u32) -> Self {
        Spb(vec![action as u8])
    }

    pub(crate) fn string(mut self, item: u32, value: &str) -> Result<Self> {
        let len = u16::try_from(value.len())
            .map_err(|_| Error::Client { message: "a service argument is too long".into() })?;
        self.0.push(item as u8);
        self.0.extend_from_slice(&len.to_le_bytes());
        self.0.extend_from_slice(value.as_bytes());
        Ok(self)
    }

    fn number(mut self, item: u32, value: u32) -> Self {
        self.0.push(item as u8);
        self.0.extend_from_slice(&value.to_le_bytes());
        self
    }

    fn flag(mut self, item: u32) -> Self {
        self.0.push(item as u8);
        self
    }

    /// An item whose value is one byte: a mode
    fn byte(mut self, item: u32, value: u32) -> Self {
        self.0.push(item as u8);
        self.0.push(value as u8);
        self
    }
}

fn bits(flags: &[(bool, u32)]) -> u32 {
    flags.iter().filter(|(on, _)| *on).fold(0, |all, (_, bit)| all | bit)
}

fn required(what: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Client { message: format!("{what} is required") });
    }
    Ok(())
}

/// The block that starts `task`
fn start_block(task: &ServiceTask) -> Result<Vec<u8>> {
    let spb = match task {
        ServiceTask::Backup { database, file, options } => {
            required("the database", database)?;
            required("the backup file", file)?;
            let o = bits(&[
                (options.ignore_checksums, ibase::isc_spb_bkp_ignore_checksums),
                (options.ignore_limbo, ibase::isc_spb_bkp_ignore_limbo),
                (options.metadata_only, ibase::isc_spb_bkp_metadata_only),
                (options.no_garbage_collect, ibase::isc_spb_bkp_no_garbage_collect),
                (options.non_transportable, ibase::isc_spb_bkp_non_transportable),
                (options.convert_external, ibase::isc_spb_bkp_convert),
            ]);
            Spb::action(ibase::isc_action_svc_backup)
                .string(ibase::isc_spb_dbname, database)?
                .string(ibase::isc_spb_bkp_file, file)?
                .flag(ibase::isc_spb_verbose)
                .number(ibase::isc_spb_options, o)
        }
        ServiceTask::Restore { file, database, options } => {
            required("the backup file", file)?;
            required("the database", database)?;
            let o = bits(&[
                (options.deactivate_indexes, ibase::isc_spb_res_deactivate_idx),
                (options.no_shadows, ibase::isc_spb_res_no_shadow),
                (options.no_validity, ibase::isc_spb_res_no_validity),
                (options.one_at_a_time, ibase::isc_spb_res_one_at_a_time),
                (options.use_all_space, ibase::isc_spb_res_use_all_space),
                (options.replace, ibase::isc_spb_res_replace),
                (!options.replace, ibase::isc_spb_res_create),
            ]);
            let mut spb = Spb::action(ibase::isc_action_svc_restore)
                .string(ibase::isc_spb_bkp_file, file)?
                .string(ibase::isc_spb_dbname, database)?
                .flag(ibase::isc_spb_verbose);
            if let Some(size) = options.page_size {
                spb = spb.number(ibase::isc_spb_res_page_size, size);
            }
            spb.number(ibase::isc_spb_options, o)
        }
        ServiceTask::Validate { database, online } => {
            required("the database", database)?;
            if *online {
                Spb::action(ibase::isc_action_svc_validate).string(ibase::isc_spb_dbname, database)?
            } else {
                // Validate, every record, and mend nothing (`gfix -v -full -n`)
                let o = ibase::isc_spb_rpr_validate_db | ibase::isc_spb_rpr_full | ibase::isc_spb_rpr_check_db;
                Spb::action(ibase::isc_action_svc_repair)
                    .string(ibase::isc_spb_dbname, database)?
                    .number(ibase::isc_spb_options, o)
            }
        }
        ServiceTask::Sweep { database } => {
            required("the database", database)?;
            Spb::action(ibase::isc_action_svc_repair)
                .string(ibase::isc_spb_dbname, database)?
                .number(ibase::isc_spb_options, ibase::isc_spb_rpr_sweep_db)
        }
        ServiceTask::Statistics { database, options } => {
            required("the database", database)?;
            let o = if options.header_only {
                ibase::isc_spb_sts_hdr_pages
            } else {
                bits(&[
                    (options.data_pages, ibase::isc_spb_sts_data_pages),
                    (options.index_pages, ibase::isc_spb_sts_idx_pages),
                    (options.record_versions, ibase::isc_spb_sts_record_versions),
                    (options.system_relations, ibase::isc_spb_sts_sys_relations),
                ])
            };
            Spb::action(ibase::isc_action_svc_db_stats)
                .string(ibase::isc_spb_dbname, database)?
                .number(ibase::isc_spb_options, o)
        }
        ServiceTask::ServerLog => Spb::action(ibase::isc_action_svc_get_fb_log),
        ServiceTask::Properties { database, changes } => {
            required("the database", database)?;
            if changes.is_empty() {
                return Err(Error::Client { message: "nothing to change".into() });
            }
            let mut spb = Spb::action(ibase::isc_action_svc_properties).string(ibase::isc_spb_dbname, database)?;
            for change in changes {
                spb = match *change {
                    Property::SweepInterval(n) => spb.number(ibase::isc_spb_prp_sweep_interval, n),
                    Property::PageBuffers(n) => spb.number(ibase::isc_spb_prp_page_buffers, n),
                    Property::ForcedWrites(on) => spb.byte(
                        ibase::isc_spb_prp_write_mode,
                        if on { ibase::isc_spb_prp_wm_sync } else { ibase::isc_spb_prp_wm_async },
                    ),
                    Property::ReadOnly(on) => spb.byte(
                        ibase::isc_spb_prp_access_mode,
                        if on { ibase::isc_spb_prp_am_readonly } else { ibase::isc_spb_prp_am_readwrite },
                    ),
                    Property::ReserveSpace(on) => spb.byte(
                        ibase::isc_spb_prp_reserve_space,
                        if on { ibase::isc_spb_prp_res } else { ibase::isc_spb_prp_res_use_full },
                    ),
                };
            }
            spb
        }
        ServiceTask::Shutdown { database, mode, wait, seconds } => {
            required("the database", database)?;
            let mode = match mode {
                ShutdownMode::Multi => ibase::isc_spb_prp_sm_multi,
                ShutdownMode::Single => ibase::isc_spb_prp_sm_single,
                ShutdownMode::Full => ibase::isc_spb_prp_sm_full,
            };
            let wait = match wait {
                ShutdownWait::Force => ibase::isc_spb_prp_force_shutdown,
                ShutdownWait::Attachments => ibase::isc_spb_prp_attachments_shutdown,
                ShutdownWait::Transactions => ibase::isc_spb_prp_transactions_shutdown,
            };
            Spb::action(ibase::isc_action_svc_properties)
                .string(ibase::isc_spb_dbname, database)?
                .byte(ibase::isc_spb_prp_shutdown_mode, mode)
                .number(wait, *seconds)
        }
        ServiceTask::Online { database, mode } => {
            required("the database", database)?;
            let mode = match mode {
                OnlineMode::Normal => ibase::isc_spb_prp_sm_normal,
                OnlineMode::Multi => ibase::isc_spb_prp_sm_multi,
                OnlineMode::Single => ibase::isc_spb_prp_sm_single,
            };
            Spb::action(ibase::isc_action_svc_properties)
                .string(ibase::isc_spb_dbname, database)?
                .byte(ibase::isc_spb_prp_online_mode, mode)
        }
        ServiceTask::Mend { database } => {
            required("the database", database)?;
            Spb::action(ibase::isc_action_svc_repair)
                .string(ibase::isc_spb_dbname, database)?
                .number(ibase::isc_spb_options, ibase::isc_spb_rpr_mend_db | ibase::isc_spb_rpr_full)
        }
        ServiceTask::PhysicalBackup { database, file, level, no_triggers } => {
            required("the database", database)?;
            required("the backup file", file)?;
            let spb = Spb::action(ibase::isc_action_svc_nbak)
                .string(ibase::isc_spb_dbname, database)?
                .string(ibase::isc_spb_nbk_file, file)?
                .number(ibase::isc_spb_nbk_level, *level);
            if *no_triggers {
                spb.number(ibase::isc_spb_options, ibase::isc_spb_nbk_no_triggers)
            } else {
                spb
            }
        }
        ServiceTask::PhysicalRestore { files, database } => {
            required("the database", database)?;
            if files.iter().all(|f| f.trim().is_empty()) {
                return Err(Error::Client { message: "the backup files are required".into() });
            }
            let mut spb = Spb::action(ibase::isc_action_svc_nrest).string(ibase::isc_spb_dbname, database)?;
            for file in files.iter().filter(|f| !f.trim().is_empty()) {
                spb = spb.string(ibase::isc_spb_nbk_file, file)?;
            }
            spb
        }
    };
    Ok(spb.0)
}

/// One answer to a query, as far as this module reads them
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Answer {
    /// Bytes of output (`isc_info_svc_to_eof`), or of one text item
    pub(crate) data: Vec<u8>,
    /// Whether an output item came at all
    pub(crate) output: bool,
    /// The server gave up waiting for more: the service is still running
    pub(crate) timed_out: bool,
    /// More of the same is waiting to be asked for
    pub(crate) truncated: bool,
    /// Nothing was written yet by a service that has not ended: what a
    /// trace session answers while the server is idle
    pub(crate) not_ready: bool,
}

const INFO_END: u8 = ibase::isc_info_end as u8;
const INFO_TRUNCATED: u8 = ibase::isc_info_truncated as u8;
const INFO_NOT_READY: u8 = ibase::isc_info_data_not_ready as u8;
pub(crate) const INFO_TO_EOF: u8 = ibase::isc_info_svc_to_eof as u8;
const INFO_LINE: u8 = ibase::isc_info_svc_line as u8;
const INFO_TIMEOUT: u8 = ibase::isc_info_svc_timeout as u8;
const INFO_VERSION: u8 = ibase::isc_info_svc_server_version as u8;
const INFO_USERS: u8 = ibase::isc_info_svc_get_users as u8;

fn malformed() -> Error {
    Error::Client { message: "the server's answer about a service could not be read".into() }
}

/// Reads an info buffer holding text items
pub(crate) fn parse_answer(buf: &[u8]) -> Result<Answer> {
    let mut answer = Answer::default();
    let mut at = 0;
    while at < buf.len() {
        match buf[at] {
            INFO_END => break,
            INFO_TRUNCATED => {
                answer.truncated = true;
                at += 1;
            }
            INFO_TIMEOUT => {
                answer.timed_out = true;
                at += 1;
            }
            INFO_NOT_READY => {
                answer.not_ready = true;
                at += 1;
            }
            INFO_TO_EOF | INFO_LINE | INFO_VERSION | INFO_USERS => {
                let len = buf.get(at + 1..at + 3).ok_or_else(malformed)?;
                let len = u16::from_le_bytes([len[0], len[1]]) as usize;
                let data = buf.get(at + 3..at + 3 + len).ok_or_else(malformed)?;
                answer.data.extend_from_slice(data);
                answer.output = true;
                at += 3 + len;
            }
            _ => return Err(malformed()),
        }
    }
    Ok(answer)
}

/// The server writes in its own character set, which nothing names: UTF-8
/// when it reads as such, and byte for byte (Latin-1) when it does not.
pub(crate) fn text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Seconds the server waits for output before answering that it has none
/// yet: how often a cancel is noticed
const WAIT_SECONDS: u32 = 1;

pub(crate) fn timeout_item() -> Vec<u8> {
    let mut send = vec![INFO_TIMEOUT, 4, 0];
    send.extend_from_slice(&WAIT_SECONDS.to_le_bytes());
    send
}

/// Splits what arrived into whole lines, keeping the unfinished one
struct Lines {
    rest: Vec<u8>,
}

impl Lines {
    fn feed(&mut self, data: &[u8], on_line: &mut dyn FnMut(&str)) {
        self.rest.extend_from_slice(data);
        while let Some(end) = self.rest.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.rest.drain(..=end).collect();
            let line = &line[..line.len() - 1];
            on_line(&text(line.strip_suffix(b"\r").unwrap_or(line)));
        }
    }

    fn finish(&mut self, on_line: &mut dyn FnMut(&str)) {
        if !self.rest.is_empty() {
            on_line(&text(&std::mem::take(&mut self.rest)));
        }
    }
}

/// Runs `task` on the server, handing over each line it writes as it
/// arrives. `cancel` is looked at about once a second.
pub fn run_service(
    params: &ConnectParams,
    task: &ServiceTask,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<ServiceEnd> {
    // The server takes one change of a database's settings at a time:
    // Firebird 3 and later refuse two together as an "incompatible switch
    // combination", and 2.5 makes only one of them
    if let ServiceTask::Properties { database, changes } = task {
        if changes.len() > 1 {
            for change in changes {
                let one = ServiceTask::Properties { database: database.clone(), changes: vec![change.clone()] };
                match run_one(params, &one, cancel, &mut on_line)? {
                    ServiceEnd::Finished => {}
                    other => return Ok(other),
                }
            }
            return Ok(ServiceEnd::Finished);
        }
    }
    run_one(params, task, cancel, &mut on_line)
}

/// One service action, run to its end
fn run_one(
    params: &ConnectParams,
    task: &ServiceTask,
    cancel: &AtomicBool,
    mut on_line: &mut dyn FnMut(&str),
) -> Result<ServiceEnd> {
    let block = start_block(task)?;
    let mut service = attach(params)?;
    service.start(&block)?;

    let send = timeout_item();
    let mut lines = Lines { rest: Vec::new() };
    loop {
        if cancel.load(Ordering::Relaxed) {
            lines.finish(&mut on_line);
            return Ok(ServiceEnd::Cancelled);
        }
        let answer = match service.query(&send, &[INFO_TO_EOF]) {
            Ok(buf) => parse_answer(&buf)?,
            Err(e) => {
                lines.finish(&mut on_line);
                return Err(e.into());
            }
        };
        lines.feed(&answer.data, &mut on_line);
        // Nothing more, and not for want of waiting or of room: it ended
        if answer.data.is_empty() && !answer.timed_out && !answer.truncated {
            lines.finish(&mut on_line);
            return Ok(ServiceEnd::Finished);
        }
    }
}

/// The server's version as it names itself, such as `LI-V4.0.5.3140 Firebird 4.0`
pub fn server_version(params: &ConnectParams) -> Result<String> {
    let mut service = attach(params)?;
    let answer = parse_answer(&service.query(&[], &[INFO_VERSION])?)?;
    Ok(text(&answer.data))
}

/// The users of the server's security database, through the services
/// manager: the only way to list them before Firebird 3
pub fn service_users(params: &ConnectParams) -> Result<Vec<ServiceUser>> {
    let mut service = attach(params)?;
    service.start(&[ibase::isc_action_svc_display_user as u8])?;
    let mut data = Vec::new();
    // A list too long for one answer comes in several
    for _ in 0..10_000 {
        let answer = parse_answer(&service.query(&[], &[INFO_USERS])?)?;
        data.extend_from_slice(&answer.data);
        if !answer.truncated {
            return parse_users(&data);
        }
    }
    Err(malformed())
}

/// The user list: items in a row, a user starting at each user name
fn parse_users(data: &[u8]) -> Result<Vec<ServiceUser>> {
    const USERNAME: u8 = ibase::isc_spb_sec_username as u8;
    const FIRST: u8 = ibase::isc_spb_sec_firstname as u8;
    const MIDDLE: u8 = ibase::isc_spb_sec_middlename as u8;
    const LAST: u8 = ibase::isc_spb_sec_lastname as u8;
    const USER_ID: u8 = ibase::isc_spb_sec_userid as u8;
    const GROUP_ID: u8 = ibase::isc_spb_sec_groupid as u8;

    let mut users: Vec<ServiceUser> = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let item = data[at];
        at += 1;
        match item {
            USER_ID | GROUP_ID => at += 4,
            USERNAME | FIRST | MIDDLE | LAST => {
                let len = data.get(at..at + 2).ok_or_else(malformed)?;
                let len = u16::from_le_bytes([len[0], len[1]]) as usize;
                let value = text(data.get(at + 2..at + 2 + len).ok_or_else(malformed)?);
                at += 2 + len;
                if item == USERNAME {
                    users.push(ServiceUser {
                        name: value,
                        first_name: String::new(),
                        middle_name: String::new(),
                        last_name: String::new(),
                    });
                } else if let Some(user) = users.last_mut() {
                    match item {
                        FIRST => user.first_name = value,
                        MIDDLE => user.middle_name = value,
                        _ => user.last_name = value,
                    }
                }
            }
            INFO_END => break,
            _ => return Err(malformed()),
        }
    }
    Ok(users)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backup_names_the_database_the_file_and_its_options() {
        let block = start_block(&ServiceTask::Backup {
            database: "db".into(),
            file: "bk".into(),
            options: BackupOptions { metadata_only: true, no_garbage_collect: true, ..Default::default() },
        })
        .unwrap();
        assert_eq!(block, [1, 106, 2, 0, b'd', b'b', 5, 2, 0, b'b', b'k', 107, 108, 0x0c, 0, 0, 0]);
    }

    #[test]
    fn a_restore_creates_unless_told_to_replace() {
        let restore = |options| {
            start_block(&ServiceTask::Restore { file: "bk".into(), database: "db".into(), options }).unwrap()
        };
        let created = restore(RestoreOptions::default());
        assert_eq!(created[created.len() - 5..], [108, 0, 0x20, 0, 0]);
        let replaced = restore(RestoreOptions { replace: true, page_size: Some(8192), ..Default::default() });
        assert_eq!(replaced[replaced.len() - 10..], [10, 0, 0x20, 0, 0, 108, 0, 0x10, 0, 0]);
    }

    #[test]
    fn a_validation_mends_nothing() {
        let block = start_block(&ServiceTask::Validate { database: "db".into(), online: false }).unwrap();
        assert_eq!(block[block.len() - 5..], [108, 0x91, 0, 0, 0]);
        let online = start_block(&ServiceTask::Validate { database: "db".into(), online: true }).unwrap();
        assert_eq!(online, [30, 106, 2, 0, b'd', b'b']);
    }

    /// The database's name as every block starts it after its action
    const DB: [u8; 5] = [106, 2, 0, b'd', b'b'];

    #[test]
    fn settings_are_changed_each_in_its_own_shape() {
        let block = start_block(&ServiceTask::Properties {
            database: "db".into(),
            changes: vec![
                Property::SweepInterval(20_000),
                Property::ForcedWrites(true),
                Property::PageBuffers(2048),
                Property::ReadOnly(false),
                Property::ReserveSpace(false),
            ],
        })
        .unwrap();
        let mut expected = vec![8];
        expected.extend(DB);
        // A number is four bytes; a mode, one
        expected.extend([6, 0x20, 0x4e, 0, 0, 12, 38, 5, 0, 8, 0, 0, 13, 40, 11, 35]);
        assert_eq!(block, expected);
        assert!(start_block(&ServiceTask::Properties { database: "db".into(), changes: vec![] }).is_err());
    }

    #[test]
    fn a_shutdown_says_its_mode_and_how_long_it_waits() {
        let block = start_block(&ServiceTask::Shutdown {
            database: "db".into(),
            mode: ShutdownMode::Single,
            wait: ShutdownWait::Attachments,
            seconds: 60,
        })
        .unwrap();
        assert_eq!(block[block.len() - 7..], [44, 2, 42, 60, 0, 0, 0]);
        let online = start_block(&ServiceTask::Online { database: "db".into(), mode: OnlineMode::Normal }).unwrap();
        assert_eq!(online[online.len() - 2..], [45, 0]);
    }

    #[test]
    fn mending_is_a_repair_that_mends() {
        let block = start_block(&ServiceTask::Mend { database: "db".into() }).unwrap();
        assert_eq!(block[0], 3);
        assert_eq!(block[block.len() - 5..], [108, 0x84, 0, 0, 0]);
    }

    #[test]
    fn a_physical_backup_names_its_level_and_a_restore_its_files_in_order() {
        let backup = start_block(&ServiceTask::PhysicalBackup {
            database: "db".into(),
            file: "l1".into(),
            level: 1,
            no_triggers: true,
        })
        .unwrap();
        let mut expected = vec![20];
        expected.extend(DB);
        expected.extend([6, 2, 0, b'l', b'1', 5, 1, 0, 0, 0, 108, 1, 0, 0, 0]);
        assert_eq!(backup, expected);
        let restore = start_block(&ServiceTask::PhysicalRestore {
            files: vec!["l0".into(), " ".into(), "l1".into()],
            database: "db".into(),
        })
        .unwrap();
        let mut expected = vec![21];
        expected.extend(DB);
        expected.extend([6, 2, 0, b'l', b'0', 6, 2, 0, b'l', b'1']);
        assert_eq!(restore, expected);
        assert!(start_block(&ServiceTask::PhysicalRestore { files: vec![" ".into()], database: "db".into() }).is_err());
    }

    #[test]
    fn a_task_without_its_database_is_refused_here() {
        assert!(start_block(&ServiceTask::Sweep { database: " ".into() }).is_err());
    }

    #[test]
    fn answers_say_whether_more_is_coming() {
        let done = parse_answer(&[63, 0, 0, 1]).unwrap();
        assert!(done.output && done.data.is_empty() && !done.timed_out && !done.truncated);
        let waiting = parse_answer(&[63, 0, 0, 64, 1]).unwrap();
        assert!(waiting.timed_out);
        let idle = parse_answer(&[63, 0, 0, 4, 1]).unwrap();
        assert!(idle.not_ready && idle.data.is_empty());
        let more = parse_answer(&[63, 2, 0, b'a', b'\n', 2, 1]).unwrap();
        assert_eq!(more.data, b"a\n");
        assert!(more.truncated);
        assert!(parse_answer(&[63, 9, 0, b'a']).is_err());
    }

    #[test]
    fn output_is_handed_over_line_by_line() {
        let mut got = Vec::new();
        let mut lines = Lines { rest: Vec::new() };
        let mut push = |l: &str| got.push(l.to_string());
        lines.feed(b"one\r\ntw", &mut push);
        lines.feed(b"o\n\nthr", &mut push);
        lines.finish(&mut push);
        assert_eq!(got, ["one", "two", "", "thr"]);
    }

    #[test]
    fn text_that_is_not_utf8_is_read_byte_for_byte() {
        assert_eq!(text(b"n\xe3o"), "não");
        assert_eq!(text("não".as_bytes()), "não");
    }

    #[test]
    fn users_start_at_their_names() {
        let data = [7, 1, 0, b'A', 10, 1, 0, b'f', 12, 1, 0, b'l', 5, 0, 0, 0, 0, 6, 0, 0, 0, 0, 7, 1, 0, b'B'];
        let users = parse_users(&data).unwrap();
        assert_eq!(users.len(), 2);
        assert_eq!((users[0].name.as_str(), users[0].first_name.as_str(), users[0].last_name.as_str()), ("A", "f", "l"));
        assert_eq!(users[1].name, "B");
    }
}

