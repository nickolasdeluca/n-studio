//! `FirebirdConnection` implementation for the pure rust firebird client

use bytes::{BufMut, Bytes, BytesMut};
use std::{
    collections::VecDeque,
    env,
    io::{Read, Write},
    net::TcpStream,
};

use crate::{
    arc4::*,
    blr,
    consts::{AuthPluginType, ProtocolVersion, WireOp},
    info::{parse_plan, parse_plans, DatabaseCounters, RecordCounts, COUNTER_ITEMS},
    srp::*,
    util::*,
    wire::*,
    xsqlda::{parse_xsqlda, xsqlda_to_blr, ColumnDesc, PrepareInfo, XSqlVar, XSQLDA_DESCRIBE_VARS},
};
use rsfbclient_core::*;

type RustDbHandle = DbHandle;
type RustTrHandle = TrHandle;
type RustStmtHandle = StmtHandle;

/// How many rows to request per op_fetch (round-trip). Configurable via
/// FB_FETCH_BATCH; defaults to 200. The crate originally used 1 (one row per round-trip).
fn fetch_batch_size() -> u32 {
    env::var("FB_FETCH_BATCH")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n > 0)
        .unwrap_or(200)
}

/// Result of parsing ONE op_fetch_response.
enum FetchOne {
    /// A row (status=0, messages=1). Blob columns still hold only their id:
    /// their contents are fetched once the whole batch has been read.
    Row(Vec<ParsedColumn>),
    /// End of THIS batch (status=0, messages=0): the server ended the op_fetch
    /// without exhausting the cursor. Re-issuing op_fetch fetches the rest.
    BatchEnd,
    /// End of cursor (status=100). Nothing more to read.
    End,
}

/// Wire tracing for the M0 spike: set FBW_TRACE=1 to dump packets.
pub(crate) fn trace(label: &str, bytes: &[u8]) {
    if env::var("FBW_TRACE").is_err() {
        return;
    }
    eprintln!("[wire] {label} ({} bytes)", bytes.len());
    for (i, chunk) in bytes.chunks(16).take(24).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|x| format!("{x:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '.' })
            .collect();
        eprintln!("  {:04x}  {:<48}  {}", i * 16, hex.join(" "), ascii);
    }
}

/// Firebird's Legacy_Auth password hash: DES crypt with the fixed salt "9z",
/// minus the two salt characters. Identical to what the protocol 11/12 DPB
/// path sends as `isc_dpb_password_enc`.
fn legacy_hash(pass: &str) -> String {
    #[allow(deprecated)]
    let hashed = pwhash::unix_crypt::hash_with("9z", pass).unwrap_or_default();
    hashed.chars().skip(2).collect()
}

/// Firebird client implemented in pure rust
pub struct RustFbClient {
    conn: Option<FirebirdWireConnection>,
    charset: Charset,
}

/// Required configuration for an attachment with the pure rust client
#[derive(Default, Clone)]
pub struct RustFbClientAttachmentConfig {
    pub host: String,
    pub port: u16,
    pub db_name: String,
    pub user: String,
    pub pass: String,
    pub role_name: Option<String>,
    pub auth: AuthPlugin,
}

/// The auth plugin the client leads with
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AuthPlugin {
    /// Srp256, then Srp, then Legacy_Auth: what fbclient offers
    #[default]
    Srp,
    /// Legacy_Auth first: the only way in to servers whose Srp user store is
    /// empty, typically kept compatible with 2.5-era applications
    Legacy,
}

/// A Connection to a firebird server
pub struct FirebirdWireConnection {
    /// Connection socket
    socket: FbStream,

    /// Wire protocol version
    pub(crate) version: ProtocolVersion,

    /// Buffer to read the network data
    buff: Box<[u8]>,

    /// Lazy responses to read
    lazy_count: u32,

    pub(crate) charset: Charset,

    /// Bytes read from the socket that no reply has consumed yet. See
    /// `read_reply`: every reply is parsed out of this buffer.
    pending: Bytes,

    /// Auth plugin reply to send in the first attach DPB, when the server
    /// accepted with op_accept_data instead of op_cond_accept.
    dpb_auth_data: Option<Vec<u8>>,
}

/// Data to keep track about a prepared statement
pub struct StmtHandleData {
    /// Statement handle
    handle: RustStmtHandle,
    /// Output xsqlda
    xsqlda: Vec<XSqlVar>,
    /// The output columns as described, before coercion
    columns: Vec<ColumnDesc>,
    /// Blr representation of the above
    blr: Bytes,
    /// Number of parameters
    param_count: usize,
    /// Rows already fetched in a batch but not yet delivered (batch fetch).
    /// Blobs are resolved as each row is delivered, never mid-batch: the rest
    /// of the batch is still in flight then, ahead of any blob reply.
    prefetched: VecDeque<Vec<ParsedColumn>>,
    /// Cursor exhausted on the server (do not request more batches).
    cursor_eof: bool,
    /// What the last execute selected, inserted, updated and deleted
    records: RecordCounts,
}

impl StmtHandleData {
    /// Output column names (aliases), in select-list order. Known from the
    /// prepare, so a grid can draw its header before any row arrives.
    pub fn column_names(&self) -> impl Iterator<Item = &str> {
        self.xsqlda.iter().map(|v| v.alias_name.as_str())
    }

    /// The output columns with their declared types, as the server
    /// described them at prepare
    pub fn columns(&self) -> &[ColumnDesc] {
        &self.columns
    }

    /// How many `?` parameters the statement takes
    pub fn param_count(&self) -> usize {
        self.param_count
    }

    /// The row counts the last execute reported, read with it
    pub fn records(&self) -> RecordCounts {
        self.records
    }
}

impl RustFbClient {
    ///Construct a new instance of the pure rust client
    pub fn new(charset: Charset) -> Self {
        Self {
            conn: None,
            charset,
        }
    }
}

impl FirebirdClientDbOps for RustFbClient {
    type DbHandle = RustDbHandle;
    type AttachmentConfig = RustFbClientAttachmentConfig;

    fn attach_database(
        &mut self,
        config: &Self::AttachmentConfig,
        dialect: Dialect,
        no_db_triggers: bool,
    ) -> Result<RustDbHandle, FbError> {
        let host = config.host.as_str();
        let port = config.port;
        let db_name = config.db_name.as_str();
        let user = config.user.as_str();
        let pass = config.pass.as_str();
        let role = match &config.role_name {
            Some(ro) => Some(ro.as_str()),
            None => None,
        };

        // Take the existing connection, or connects
        let mut conn = match self.conn.take() {
            Some(conn) => conn,
            None => FirebirdWireConnection::connect(
                host,
                port,
                db_name,
                user,
                pass,
                self.charset.clone(),
                config.auth,
            )?,
        };

        let attach_result =
            conn.attach_database(db_name, user, pass, role, dialect, no_db_triggers);

        // Put the connection back
        self.conn.replace(conn);

        attach_result
    }

    fn detach_database(&mut self, db_handle: &mut RustDbHandle) -> Result<(), FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.detach_database(db_handle))
            .unwrap_or_else(err_client_not_connected)
    }

    fn drop_database(&mut self, db_handle: &mut RustDbHandle) -> Result<(), FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.drop_database(db_handle))
            .unwrap_or_else(err_client_not_connected)
    }

    fn create_database(
        &mut self,
        config: &Self::AttachmentConfig,
        page_size: Option<u32>,
        dialect: Dialect,
    ) -> Result<RustDbHandle, FbError> {
        let host = config.host.as_str();
        let port = config.port;
        let db_name = config.db_name.as_str();
        let user = config.user.as_str();
        let pass = config.pass.as_str();
        let role = match &config.role_name {
            Some(ro) => Some(ro.as_str()),
            None => None,
        };

        // Take the existing connection, or connects
        let mut conn = match self.conn.take() {
            Some(conn) => conn,
            None => FirebirdWireConnection::connect(
                host,
                port,
                db_name,
                user,
                pass,
                self.charset.clone(),
                config.auth,
            )?,
        };

        let attach_result = conn.create_database(db_name, user, pass, page_size, role, dialect);

        // Put the connection back
        self.conn.replace(conn);

        attach_result
    }
}

impl FirebirdClientSqlOps for RustFbClient {
    type DbHandle = RustDbHandle;
    type TrHandle = RustTrHandle;
    type StmtHandle = StmtHandleData;

    fn begin_transaction(
        &mut self,
        db_handle: &mut Self::DbHandle,
        confs: TransactionConfiguration,
    ) -> Result<Self::TrHandle, FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.begin_transaction(db_handle, confs))
            .unwrap_or_else(err_client_not_connected)
    }

    fn transaction_operation(
        &mut self,
        tr_handle: &mut Self::TrHandle,
        op: TrOp,
    ) -> Result<(), FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.transaction_operation(tr_handle, op))
            .unwrap_or_else(err_client_not_connected)
    }

    fn exec_immediate(
        &mut self,
        _db_handle: &mut Self::DbHandle,
        tr_handle: &mut Self::TrHandle,
        dialect: Dialect,
        sql: &str,
    ) -> Result<(), FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.exec_immediate(tr_handle, dialect, sql))
            .unwrap_or_else(err_client_not_connected)
    }

    fn prepare_statement(
        &mut self,
        db_handle: &mut Self::DbHandle,
        tr_handle: &mut Self::TrHandle,
        dialect: Dialect,
        sql: &str,
    ) -> Result<(StmtType, Self::StmtHandle), FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.prepare_statement(db_handle, tr_handle, dialect, sql))
            .unwrap_or_else(err_client_not_connected)
    }

    fn free_statement(
        &mut self,
        stmt_handle: &mut Self::StmtHandle,
        op: FreeStmtOp,
    ) -> Result<(), FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.free_statement(stmt_handle, op))
            .unwrap_or_else(err_client_not_connected)
    }

    fn execute(
        &mut self,
        _db_handle: &mut Self::DbHandle,
        tr_handle: &mut Self::TrHandle,
        stmt_handle: &mut Self::StmtHandle,
        params: Vec<SqlType>,
    ) -> Result<usize, FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.execute(tr_handle, stmt_handle, &params))
            .unwrap_or_else(err_client_not_connected)
    }

    fn execute2(
        &mut self,
        _db_handle: &mut Self::DbHandle,
        tr_handle: &mut Self::TrHandle,
        stmt_handle: &mut Self::StmtHandle,
        params: Vec<SqlType>,
    ) -> Result<Vec<Column>, FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.execute2(tr_handle, stmt_handle, &params))
            .unwrap_or_else(err_client_not_connected)
    }

    fn fetch(
        &mut self,
        _db_handle: &mut Self::DbHandle,
        tr_handle: &mut Self::TrHandle,
        stmt_handle: &mut Self::StmtHandle,
    ) -> Result<Option<Vec<Column>>, FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.fetch(tr_handle, stmt_handle))
            .unwrap_or_else(err_client_not_connected)
    }
}

/// Operations beyond the `rsfbclient-core` traits. Candidates for upstreaming
/// as a trait of their own; inherent methods until then.
impl RustFbClient {
    /// The statement's access plan: the classic one-line `PLAN (...)`, or with
    /// `explained` the FB3+ tree form. `None` for statements without a plan.
    pub fn statement_plan(
        &mut self,
        stmt_handle: &mut StmtHandleData,
        explained: bool,
    ) -> Result<Option<String>, FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.statement_plan(stmt_handle, explained))
            .unwrap_or_else(err_client_not_connected)
    }

    /// The classic plan and, with `explained` (Firebird 3+ only), the
    /// explained one too, in one round trip.
    pub fn statement_plans(
        &mut self,
        stmt_handle: &mut StmtHandleData,
        explained: bool,
    ) -> Result<(Option<String>, Option<String>), FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.statement_plans(stmt_handle, explained))
            .unwrap_or_else(err_client_not_connected)
    }

    /// A snapshot of the attachment's performance counters.
    pub fn database_counters(
        &mut self,
        db_handle: &mut RustDbHandle,
    ) -> Result<DatabaseCounters, FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.database_counters(db_handle))
            .unwrap_or_else(err_client_not_connected)
    }

    /// How many rows an executed statement selected, inserted, updated and
    /// deleted.
    pub fn statement_records(&mut self, stmt_handle: &mut StmtHandleData) -> Result<RecordCounts, FbError> {
        self.conn
            .as_mut()
            .map(|conn| conn.statement_records(stmt_handle))
            .unwrap_or_else(err_client_not_connected)
    }
}

fn err_client_not_connected<T>() -> Result<T, FbError> {
    Err("Client not connected to the server, call `attach_database` to connect".into())
}

impl FirebirdWireConnection {
    /// Start a connection to the firebird server
    pub fn connect(
        host: &str,
        port: u16,
        db_name: &str,
        user: &str,
        pass: &str,
        charset: Charset,
        auth: AuthPlugin,
    ) -> Result<Self, FbError> {
        let socket = TcpStream::connect((host, port))?;

        // System username
        let username =
            env::var("USER").unwrap_or_else(|_| env::var("USERNAME").unwrap_or_default());
        let hostname = socket
            .local_addr()
            .map(|addr| addr.to_string())
            .unwrap_or_default();

        let mut socket = FbStream::Plain(socket);

        // Random key for the srp
        let srp_key: [u8; 32] = rand::random();

        // FB_AUTH=legacy is a development override for tools built on the
        // high-level rsfbclient builder, which cannot set `auth`.
        let legacy = auth == AuthPlugin::Legacy || env::var("FB_AUTH").as_deref() == Ok("legacy");
        let legacy_hashed = legacy.then(|| legacy_hash(pass));
        let req = connect(
            db_name,
            user,
            &username,
            &hostname,
            &srp_key,
            legacy_hashed.as_deref(),
        );
        trace("op_connect ->", &req);
        socket.write_all(&req)?;
        socket.flush()?;

        // May be a bit too much
        let mut buff = vec![0; BUFFER_LENGTH as usize * 2].into_boxed_slice();

        // Nothing is sent lazily before the attach, so no replies are owed.
        let mut pending = Bytes::new();
        let mut no_lazy = 0;

        let ConnectionResponse {
            version,
            auth_plugin,
            auth_in_dpb,
        } = read_reply(&mut socket, &mut buff, &mut pending, &mut no_lazy, |r, _| {
            parse_accept(r)
        })?;

        let mut dpb_auth_data = None;

        match auth_plugin {
            // op_accept_data: the server takes the plugin's reply in the attach
            // DPB and will refuse op_cont_auth with isc_unavailable. No session
            // key is agreed on this path, so the connection stays plaintext —
            // which is what the server asked for by not offering op_cond_accept.
            Some(auth_plugin) if auth_in_dpb => {
                let data = match (auth_plugin.kind, auth_plugin.data) {
                    (AuthPluginType::Srp, Some(data)) => {
                        let srp = SrpClient::<sha1::Sha1>::new(&srp_key, &SRP_GROUP);
                        srp_proof(srp, user, pass, &data)?.0
                    }
                    (AuthPluginType::Srp256, Some(data)) => {
                        let srp = SrpClient::<sha2::Sha256>::new(&srp_key, &SRP_GROUP);
                        srp_proof(srp, user, pass, &data)?.0
                    }
                    (AuthPluginType::Legacy, _) => legacy_hash(pass).into_bytes(),
                    (plugin, None) => {
                        return Err(format!(
                            "Server chose {} without sending its auth data, and op_accept_data \
                             leaves no way to ask for it",
                            plugin.name()
                        )
                        .into())
                    }
                };
                dpb_auth_data = Some(data);
            }
            Some(mut auth_plugin) => loop {
                match auth_plugin.kind {
                    plugin @ AuthPluginType::Srp => {
                        let srp = SrpClient::<sha1::Sha1>::new(&srp_key, &SRP_GROUP);

                        if let Some(data) = auth_plugin.data {
                            let next;
                            (socket, next) = srp_auth(
                                socket,
                                &mut buff,
                                &mut pending,
                                srp,
                                plugin,
                                user,
                                pass,
                                data,
                            )?;
                            match next {
                                Some(next) => auth_plugin = next,
                                None => break,
                            }
                        } else {
                            // Server requested a different authentication method than the client specified
                            // in the initial connection

                            let req = cont_auth(
                                hex::encode(srp.get_a_pub()).as_bytes(),
                                plugin,
                                AuthPluginType::plugin_list(),
                                &[],
                            );
                            trace("op_cont_auth ->", &req);
                            socket.write_all(&req)?;
                            socket.flush()?;

                            auth_plugin = read_reply(
                                &mut socket,
                                &mut buff,
                                &mut pending,
                                &mut no_lazy,
                                |r, _| parse_cont_auth(r),
                            )?;
                        }
                    }
                    AuthPluginType::Legacy => {
                        // Legacy_Auth carries the DES-crypted password as auth
                        // data. No session key is derived, so the connection
                        // stays in plaintext afterwards — servers running
                        // WireCrypt = Required will refuse at this point.
                        let enc_pass = legacy_hash(pass);

                        let req = cont_auth(
                            enc_pass.as_bytes(),
                            AuthPluginType::Legacy,
                            AuthPluginType::plugin_list(),
                            &auth_plugin.keys,
                        );
                        trace("op_cont_auth(legacy) ->", &req);
                        socket.write_all(&req)?;
                        socket.flush()?;

                        // op_response ends the exchange (a failure surfaces as
                        // its error); op_cont_auth asks for another round.
                        let next = read_reply(
                            &mut socket,
                            &mut buff,
                            &mut pending,
                            &mut no_lazy,
                            |r, _| {
                                let mut peek = r.clone();
                                if peek.get_u32()? == WireOp::Response as u32 {
                                    r.advance(4)?;
                                    return Ok(response_body(r)?.map(|_| None));
                                }
                                Ok(Ok(Some(parse_cont_auth(r)?)))
                            },
                        )??;
                        match next {
                            Some(plugin) => auth_plugin = plugin,
                            None => break,
                        }
                    }

                    plugin @ AuthPluginType::Srp256 => {
                        let srp = SrpClient::<sha2::Sha256>::new(&srp_key, &SRP_GROUP);

                        if let Some(data) = auth_plugin.data {
                            let next;
                            (socket, next) = srp_auth(
                                socket,
                                &mut buff,
                                &mut pending,
                                srp,
                                plugin,
                                user,
                                pass,
                                data,
                            )?;
                            match next {
                                Some(next) => auth_plugin = next,
                                None => break,
                            }
                        } else {
                            // Server requested a different authentication method than the client specified
                            // in the initial connection

                            let req = cont_auth(
                                hex::encode(srp.get_a_pub()).as_bytes(),
                                plugin,
                                AuthPluginType::plugin_list(),
                                &[],
                            );
                            trace("op_cont_auth ->", &req);
                            socket.write_all(&req)?;
                            socket.flush()?;

                            auth_plugin = read_reply(
                                &mut socket,
                                &mut buff,
                                &mut pending,
                                &mut no_lazy,
                                |r, _| parse_cont_auth(r),
                            )?;
                        }
                    }
                }
            },
            None => {}
        }

        Ok(Self {
            socket,
            version,
            buff,
            lazy_count: 0,
            charset,
            pending,
            dpb_auth_data,
        })
    }

    /// Create the database and attach, returning a database handle
    pub fn create_database(
        &mut self,
        db_name: &str,
        user: &str,
        pass: &str,
        page_size: Option<u32>,
        role_name: Option<&str>,
        dialect: Dialect,
    ) -> Result<DbHandle, FbError> {
        self.socket.write_all(&create(
            db_name,
            user,
            pass,
            self.version,
            self.charset.clone(),
            page_size,
            role_name.clone(),
            dialect,
            self.dpb_auth_data.take().as_deref(),
        ))?;
        self.socket.flush()?;

        let resp = self.read_response()?;

        Ok(DbHandle(resp.handle))
    }

    /// Connect to a database, returning a database handle
    pub fn attach_database(
        &mut self,
        db_name: &str,
        user: &str,
        pass: &str,
        role_name: Option<&str>,
        dialect: Dialect,
        no_db_triggers: bool,
    ) -> Result<DbHandle, FbError> {
        let req = attach(
            db_name,
            user,
            pass,
            self.version,
            self.charset.clone(),
            role_name.clone(),
            dialect,
            no_db_triggers,
            self.dpb_auth_data.take().as_deref(),
        );
        trace("op_attach ->", &req);
        self.socket.write_all(&req)?;
        self.socket.flush()?;

        // With the auth data in the DPB (op_accept_data), a server that turns
        // it down does not refuse yet: it answers op_cont_auth with the next
        // plugin on its list, and the login fails only once none is left.
        // Each round starts with the server asking for the client's first
        // data (an SRP public key) and ends with it sending its own.
        let mut srp_key: [u8; 32] = rand::random();
        loop {
            let next = self.read_reply(|r, lazy| {
                let mut peek = r.clone();
                if peek.get_u32()? == WireOp::ContAuth as u32 {
                    return Ok(Ok(AttachReply::ContAuth(parse_cont_auth(r)?)));
                }
                Ok(op_response(r, lazy)?.map(AttachReply::Attached))
            })??;
            let plugin = match next {
                AttachReply::Attached(resp) => return Ok(DbHandle(resp.handle)),
                AttachReply::ContAuth(plugin) => plugin,
            };
            let data = match (&plugin.kind, &plugin.data) {
                (AuthPluginType::Srp, Some(data)) => {
                    let srp = SrpClient::<sha1::Sha1>::new(&srp_key, &SRP_GROUP);
                    srp_proof(srp, user, pass, data)?.0
                }
                (AuthPluginType::Srp256, Some(data)) => {
                    let srp = SrpClient::<sha2::Sha256>::new(&srp_key, &SRP_GROUP);
                    srp_proof(srp, user, pass, data)?.0
                }
                (AuthPluginType::Srp | AuthPluginType::Srp256, None) => {
                    srp_key = rand::random();
                    // The public key does not depend on the digest
                    let srp = SrpClient::<sha1::Sha1>::new(&srp_key, &SRP_GROUP);
                    hex::encode(srp.get_a_pub()).into_bytes()
                }
                (AuthPluginType::Legacy, _) => legacy_hash(pass).into_bytes(),
            };
            let req = cont_auth(&data, plugin.kind, AuthPluginType::plugin_list(), &plugin.keys);
            trace("op_cont_auth(attach) ->", &req);
            self.socket.write_all(&req)?;
            self.socket.flush()?;
        }
    }

    /// Disconnect from the database
    pub fn detach_database(&mut self, db_handle: &mut DbHandle) -> Result<(), FbError> {
        self.socket.write_all(&detach(db_handle.0))?;
        self.socket.flush()?;

        self.read_response()?;

        Ok(())
    }

    /// Drop the database
    pub fn drop_database(&mut self, db_handle: &mut DbHandle) -> Result<(), FbError> {
        self.socket.write_all(&drop_database(db_handle.0))?;
        self.socket.flush()?;

        self.read_response()?;

        Ok(())
    }

    /// Start a new transaction, with the specified transaction parameter buffer
    pub fn begin_transaction(
        &mut self,
        db_handle: &mut DbHandle,
        confs: TransactionConfiguration,
    ) -> Result<TrHandle, FbError> {
        let mut tpb = vec![
            ibase::isc_tpb_version3 as u8,
            confs.isolation.into(),
            confs.data_access as u8,
            confs.lock_resolution.into(),
        ];
        if let TrLockResolution::Wait(Some(time)) = confs.lock_resolution {
            tpb.push(ibase::isc_tpb_lock_timeout as u8);
            tpb.push(4 as u8);
            tpb.extend_from_slice(&time.to_le_bytes());
        }

        if let TrIsolationLevel::ReadCommited(rec) = confs.isolation {
            tpb.push(rec as u8);
        }

        self.socket.write_all(&transaction(db_handle.0, &tpb))?;
        self.socket.flush()?;

        let resp = self.read_response()?;

        Ok(TrHandle(resp.handle))
    }

    /// Commit / Rollback a transaction
    pub fn transaction_operation(
        &mut self,
        tr_handle: &mut TrHandle,
        op: TrOp,
    ) -> Result<(), FbError> {
        self.socket
            .write_all(&transaction_operation(tr_handle.0, op))?;
        self.socket.flush()?;

        self.read_response()?;

        Ok(())
    }

    /// Execute a sql immediately, without returning rows
    pub fn exec_immediate(
        &mut self,
        tr_handle: &mut TrHandle,
        dialect: Dialect,
        sql: &str,
    ) -> Result<(), FbError> {
        self.socket.write_all(&exec_immediate(
            tr_handle.0,
            dialect as u32,
            sql,
            &self.charset,
        )?)?;
        self.socket.flush()?;

        self.read_response()?;

        Ok(())
    }

    /// Alloc and prepare a statement
    ///
    /// Returns the statement type, handle and xsqlda describing the columns
    pub fn prepare_statement(
        &mut self,
        db_handle: &mut DbHandle,
        tr_handle: &mut TrHandle,
        dialect: Dialect,
        sql: &str,
    ) -> Result<(StmtType, StmtHandleData), FbError> {
        // Alloc statement
        self.socket.write_all(&allocate_statement(db_handle.0))?;
        // Prepare statement
        self.socket.write_all(&prepare_statement(
            tr_handle.0,
            u32::MAX,
            dialect as u32,
            sql,
            &self.charset,
        )?)?;
        self.socket.flush()?;

        // Both replies are read before either error is reported: a failed
        // allocation is still followed by the prepare's reply, and leaving
        // that unread would hand it to whichever op reads next.
        let (alloc, prepare) = self.read_reply(|r, lazy| {
            let alloc = op_response(r, lazy)?;
            let prepare = op_response(r, lazy)?;
            Ok((alloc, prepare))
        })?;

        let stmt_handle = StmtHandle(alloc?.handle);

        let mut xsqlda = Vec::new();

        let mut resp = prepare?;
        let PrepareInfo {
            stmt_type,
            mut param_count,
            mut truncated,
        } = parse_xsqlda(&mut resp.data, &mut xsqlda)?;

        while truncated {
            // Get more info on the types
            let next_index = (xsqlda.len() as u16).to_le_bytes();

            self.socket.write_all(&info_sql(
                stmt_handle.0,
                &[
                    &[
                        ibase::isc_info_sql_sqlda_start as u8, // Describe a xsqlda
                        2,
                        next_index[0], // Index, first byte
                        next_index[1], // Index, second byte
                    ],
                    &XSQLDA_DESCRIBE_VARS[..], // Data to be returned
                ]
                .concat(),
            ))?;
            self.socket.flush()?;

            let mut data = self.read_response()?.data;

            let parse_resp = parse_xsqlda(&mut data, &mut xsqlda)?;
            truncated = parse_resp.truncated;
            param_count = parse_resp.param_count;
        }

        // Coerce the output columns and transform to blr
        let columns = xsqlda.iter().map(ColumnDesc::from).collect();
        for var in xsqlda.iter_mut() {
            var.coerce()?;
        }
        let blr = xsqlda_to_blr(&xsqlda)?;

        Ok((
            stmt_type,
            StmtHandleData {
                handle: stmt_handle,
                xsqlda,
                columns,
                blr,
                param_count,
                prefetched: VecDeque::new(),
                cursor_eof: false,
                records: RecordCounts::default(),
            },
        ))
    }

    /// Statement plan. See `RustFbClient::statement_plan`.
    pub fn statement_plan(
        &mut self,
        stmt_handle: &mut StmtHandleData,
        explained: bool,
    ) -> Result<Option<String>, FbError> {
        let item = if explained {
            ibase::isc_info_sql_explain_plan
        } else {
            ibase::isc_info_sql_get_plan
        };

        self.socket
            .write_all(&info_sql(stmt_handle.handle.0, &[item as u8]))?;
        self.socket.flush()?;

        let data = self.read_response()?.data;

        match parse_plan(data)? {
            Some(text) => Ok(Some(self.charset.decode(&text[..])?)),
            None => Ok(None),
        }
    }

    /// Both plans. See `RustFbClient::statement_plans`.
    pub fn statement_plans(
        &mut self,
        stmt_handle: &mut StmtHandleData,
        explained: bool,
    ) -> Result<(Option<String>, Option<String>), FbError> {
        let items: &[u8] = if explained {
            &[ibase::isc_info_sql_get_plan as u8, ibase::isc_info_sql_explain_plan as u8]
        } else {
            &[ibase::isc_info_sql_get_plan as u8]
        };
        self.socket.write_all(&info_sql(stmt_handle.handle.0, items))?;
        self.socket.flush()?;

        let data = self.read_response()?.data;
        let (classic, explained) = parse_plans(data)?;
        let decode = |text: Option<Bytes>| text.map(|t| self.charset.decode(&t[..])).transpose();
        Ok((decode(classic)?, decode(explained)?))
    }

    /// Row counts. See `RustFbClient::statement_records`.
    pub fn statement_records(&mut self, stmt_handle: &mut StmtHandleData) -> Result<RecordCounts, FbError> {
        self.socket
            .write_all(&info_sql(stmt_handle.handle.0, &[ibase::isc_info_sql_records as u8]))?;
        self.socket.flush()?;

        let data = self.read_response()?.data;

        RecordCounts::parse(data)
    }

    /// Performance counters. See `RustFbClient::database_counters`.
    pub fn database_counters(&mut self, db_handle: &mut DbHandle) -> Result<DatabaseCounters, FbError> {
        self.socket
            .write_all(&info_database(db_handle.0, COUNTER_ITEMS))?;
        self.socket.flush()?;

        let data = self.read_response()?.data;

        DatabaseCounters::parse(data)
    }

    /// Closes or drops a statement
    pub fn free_statement(
        &mut self,
        stmt_handle: &mut StmtHandleData,
        op: FreeStmtOp,
    ) -> Result<(), FbError> {
        self.socket
            .write_all(&free_statement(stmt_handle.handle.0, op))?;
        // Obs.: Lazy response

        self.lazy_count += 1;

        Ok(())
    }

    /// Execute the prepared statement with parameters
    pub fn execute(
        &mut self,
        tr_handle: &mut TrHandle,
        stmt_handle: &mut StmtHandleData,
        params: &[SqlType],
    ) -> Result<usize, FbError> {
        if params.len() != stmt_handle.param_count {
            return Err(format!(
                "Tried to execute a statement that has {} parameters while providing {}",
                stmt_handle.param_count,
                params.len()
            )
            .into());
        }

        // Reopen the cursor: drop prefetched rows and the batch-fetch EOF flag
        // from the previous execution. Without this, re-executing the same
        // statement would inherit cursor_eof=true and fetch nothing.
        stmt_handle.prefetched.clear();
        stmt_handle.cursor_eof = false;

        // Execute
        let params = blr::params_to_blr(self, tr_handle, params)?;

        // The row counts are asked for in the same round trip: upstream sent
        // the request only after the execute's reply, a second network
        // latency on every statement, selects included.
        self.socket.write_all(&execute(
            tr_handle.0,
            stmt_handle.handle.0,
            &params.blr,
            &params.values,
        ))?;
        self.socket.write_all(&info_sql(
            stmt_handle.handle.0,
            &[ibase::isc_info_sql_records as u8],
        ))?;
        self.socket.flush()?;

        // Both replies are read before either error is reported, as in
        // prepare_statement: the info reply follows a failed execute too.
        let (executed, records) = self.read_reply(|r, lazy| {
            let executed = op_response(r, lazy)?;
            let records = op_response(r, lazy)?;
            Ok((executed, records))
        })?;
        executed?;
        let records = RecordCounts::parse(records?.data)?;
        stmt_handle.records = records;

        Ok((records.inserted + records.updated + records.deleted) as usize)
    }

    /// Execute the prepared statement with parameters, returning data
    pub fn execute2(
        &mut self,
        tr_handle: &mut TrHandle,
        stmt_handle: &mut StmtHandleData,
        params: &[SqlType],
    ) -> Result<Vec<Column>, FbError> {
        if params.len() != stmt_handle.param_count {
            return Err(format!(
                "Tried to execute a statement that has {} parameters while providing {}",
                stmt_handle.param_count,
                params.len()
            )
            .into());
        }

        // Reopen the cursor (same reason as execute): reset the batch-fetch
        // state from the previous execution.
        stmt_handle.prefetched.clear();
        stmt_handle.cursor_eof = false;

        let params = blr::params_to_blr(self, tr_handle, params)?;

        self.socket.write_all(&execute2(
            tr_handle.0,
            stmt_handle.handle.0,
            &params.blr,
            &params.values,
            &stmt_handle.blr,
        ))?;
        self.socket.flush()?;

        let version = self.version;
        let charset = self.charset.clone();
        let xsqlda = &stmt_handle.xsqlda;

        // Success is op_sql_response (the row) followed by op_response; a
        // failure is a lone op_response carrying the error.
        let parsed_cols = self.read_reply(|r, lazy| {
            let op_code = next_op(r, lazy)?;

            if op_code == WireOp::Response as u32 {
                return Ok(match response_body(r)? {
                    Err(e) => Err(e),
                    Ok(_) => Err("execute2: server returned no row and no error".into()),
                });
            }

            if op_code != WireOp::SqlResponse as u32 {
                return err_conn_rejected(op_code);
            }

            let has_row = r.clone().get_u32()? != 0;
            let row = if has_row {
                Some(parse_sql_response(r, xsqlda, version, &charset)?)
            } else {
                r.advance(4)?;
                None
            };

            Ok(op_response(r, lazy)?.and_then(|_| {
                row.ok_or_else(|| FbError::from("execute2: statement returned no row"))
            }))
        })??;

        let mut cols = Vec::with_capacity(parsed_cols.len());

        for pc in parsed_cols {
            cols.push(pc.into_column(self, tr_handle)?);
        }

        Ok(cols)
    }

    /// Fetch ONE row. Served from a buffer filled in batches: when the buffer
    /// empties, a single op_fetch requests `FB_FETCH_BATCH` rows in one
    /// round-trip (it used to be one row per round-trip). Streaming is
    /// preserved — rows come out one at a time, memory bounded to one batch.
    pub fn fetch(
        &mut self,
        tr_handle: &mut TrHandle,
        stmt_handle: &mut StmtHandleData,
    ) -> Result<Option<Vec<Column>>, FbError> {
        let count = fetch_batch_size();
        let mut empty_batches = 0u32;
        while stmt_handle.prefetched.is_empty() && !stmt_handle.cursor_eof {
            self.fetch_batch(stmt_handle, count)?;
            empty_batches += 1;
            // Safety net: a well-behaved server never sends empty batches
            // without exhausting the cursor; guards against a hang if it does.
            if empty_batches > 1000 {
                return Err("fetch: too many empty batches without end of cursor".into());
            }
        }

        let Some(parsed) = stmt_handle.prefetched.pop_front() else {
            return Ok(None);
        };
        let mut cols = Vec::with_capacity(parsed.len());
        for pc in parsed {
            cols.push(pc.into_column(self, tr_handle)?);
        }
        Ok(Some(cols))
    }

    /// Requests `count` rows in one op_fetch and reads every op_fetch_response
    /// up to the one that ends the batch, filling `stmt_handle.prefetched`.
    fn fetch_batch(&mut self, stmt_handle: &mut StmtHandleData, count: u32) -> Result<(), FbError> {
        self.socket
            .write_all(&fetch(stmt_handle.handle.0, &stmt_handle.blr, count))?;
        self.socket.flush()?;

        let version = self.version;
        let charset = self.charset.clone();
        let mut got = 0u32;

        loop {
            let xsqlda = &stmt_handle.xsqlda;
            let reply =
                self.read_reply(|r, lazy| parse_fetch_reply(r, lazy, xsqlda, version, &charset))?;

            match reply {
                FetchOne::Row(cols) => {
                    stmt_handle.prefetched.push_back(cols);
                    got += 1;
                    // Rows are always followed by a terminating op_fetch_response
                    // (BatchEnd or End), which has to be read too. A server
                    // sending more rows than requested would be a protocol bug.
                    if got > count {
                        return Err("server sent more rows than requested in op_fetch".into());
                    }
                }
                FetchOne::BatchEnd => return Ok(()),
                FetchOne::End => {
                    stmt_handle.cursor_eof = true;
                    return Ok(());
                }
            }
        }
    }

    /// Create a new blob, returning the blob handle and id
    pub fn create_blob(
        &mut self,
        tr_handle: &mut TrHandle,
    ) -> Result<(BlobHandle, BlobId), FbError> {
        self.socket.write_all(&create_blob(tr_handle.0))?;
        self.socket.flush()?;

        let resp = self.read_response()?;

        Ok((BlobHandle(resp.handle), BlobId(resp.object_id)))
    }

    /// Put blob segments
    pub fn put_segments(&mut self, blob_handle: BlobHandle, data: &[u8]) -> Result<(), FbError> {
        for segment in data.chunks(crate::blr::MAX_DATA_LENGTH) {
            self.socket
                .write_all(&put_segment(blob_handle.0, segment))?;
            self.socket.flush()?;

            self.read_response()?;
        }

        Ok(())
    }

    /// Open a blob, returning the blob handle
    pub fn open_blob(
        &mut self,
        tr_handle: &mut TrHandle,
        blob_id: BlobId,
    ) -> Result<BlobHandle, FbError> {
        self.socket.write_all(&open_blob(tr_handle.0, blob_id.0))?;
        self.socket.flush()?;

        let resp = self.read_response()?;

        Ok(BlobHandle(resp.handle))
    }

    /// Get a blob segment, returns the bytes and true if there is more data
    pub fn get_segment(&mut self, blob_handle: BlobHandle) -> Result<(Bytes, bool), FbError> {
        self.socket.write_all(&get_segment(blob_handle.0))?;
        self.socket.flush()?;

        let mut blob_data = BytesMut::with_capacity(256);

        let resp = self.read_response()?;
        let mut data = resp.data;

        loop {
            if data.remaining() < 2 {
                break;
            }
            let len = data.get_u16_le()? as usize;
            if data.remaining() < len {
                return err_invalid_response();
            }
            blob_data.put_slice(&data[..len]);
            data.advance(len)?;
        }

        Ok((blob_data.freeze(), resp.handle == 2))
    }

    /// Closes a blob handle
    pub fn close_blob(&mut self, blob_handle: BlobHandle) -> Result<(), FbError> {
        self.socket.write_all(&close_blob(blob_handle.0))?;
        self.socket.flush()?;

        self.read_response()?;

        Ok(())
    }

    /// Read a server response
    fn read_response(&mut self) -> Result<Response, FbError> {
        self.read_reply(op_response)?
    }

    /// Parse one reply out of the connection buffer. See `read_reply`.
    fn read_reply<T>(
        &mut self,
        parse: impl FnMut(&mut Bytes, &mut u32) -> Result<T, FbError>,
    ) -> Result<T, FbError> {
        read_reply(
            &mut self.socket,
            &mut self.buff,
            &mut self.pending,
            &mut self.lazy_count,
            parse,
        )
    }
}

/// Parses one reply out of `pending`, reading from the socket until `parse`
/// has every byte it needs.
///
/// Firebird replies carry no length prefix, and with lazy_send the server
/// coalesces several of them into one TCP segment, so a socket read can stop
/// mid-reply or run past the end of one. The only reliable frame is the parser
/// itself: `parse` consumes exactly one reply (alignment padding included)
/// from the front of the buffer, and what it leaves is the start of the next.
///
/// A parse that runs out of bytes fails with "missing bytes"; the buffer is
/// then topped up from the socket and `parse` reruns from the start, so it
/// must not have side effects beyond its arguments. Any other outcome — a
/// value, or an error the server sent — commits the consumed bytes, since a
/// server error is itself a complete reply.
///
/// `lazy_count` is the number of replies still owed for lazily-sent ops; see
/// `next_op`.
fn read_reply<T>(
    socket: &mut impl Read,
    buff: &mut [u8],
    pending: &mut Bytes,
    lazy_count: &mut u32,
    mut parse: impl FnMut(&mut Bytes, &mut u32) -> Result<T, FbError>,
) -> Result<T, FbError> {
    loop {
        let mut view = pending.clone();
        let mut lazy = *lazy_count;

        match parse(&mut view, &mut lazy) {
            Err(e) if is_missing_bytes(&e) => {
                let n = socket.read(buff)?;
                if n == 0 {
                    return Err("Connection closed by the server mid-reply".into());
                }
                trace("read <-", &buff[..n]);

                let mut grown = BytesMut::with_capacity(pending.len() + n);
                grown.put_slice(pending);
                grown.put_slice(&buff[..n]);
                *pending = grown.freeze();
            }
            other => {
                *pending = view;
                *lazy_count = lazy;
                return other;
            }
        }
    }
}

/// True for the error a parser raises when the reply is not fully buffered.
fn is_missing_bytes(e: &FbError) -> bool {
    matches!(e, FbError::Other(m) if m == MISSING_BYTES)
}

/// Reads the next opcode, skipping op_dummy keepalives and the replies still
/// owed for lazily-sent ops (op_free_statement), which the server delivers
/// ahead of the reply to whatever was sent next.
fn next_op(r: &mut Bytes, lazy: &mut u32) -> Result<u32, FbError> {
    loop {
        let op_code = r.get_u32()?;

        if op_code == WireOp::Dummy as u32 {
            continue;
        }

        if *lazy > 0 {
            if op_code != WireOp::Response as u32 {
                return err_conn_rejected(op_code);
            }
            *lazy -= 1;
            // A failed free belongs to no caller: reporting it here would
            // misattribute it to the op being read, and returning early
            // would leave that op's reply in the buffer.
            response_body(r)?.ok();
            continue;
        }

        return Ok(op_code);
    }
}

/// Reads one op_response. The outer error means the reply could not be read
/// (incomplete, or another op); the inner one is the server's own error,
/// carried by a reply that was consumed in full.
fn op_response(r: &mut Bytes, lazy: &mut u32) -> Result<Result<Response, FbError>, FbError> {
    let op_code = next_op(r, lazy)?;
    if op_code != WireOp::Response as u32 {
        return err_conn_rejected(op_code);
    }
    response_body(r)
}

/// The body of an op_response whose opcode was already read. See `op_response`.
fn response_body(r: &mut Bytes) -> Result<Result<Response, FbError>, FbError> {
    match parse_response(r) {
        Err(e) if is_missing_bytes(&e) => Err(e),
        resp => Ok(resp),
    }
}

/// Parses one reply to op_fetch: a row, the end of the batch, or the end of
/// the cursor. Blob columns come back unresolved.
fn parse_fetch_reply(
    r: &mut Bytes,
    lazy: &mut u32,
    xsqlda: &[XSqlVar],
    version: ProtocolVersion,
    charset: &Charset,
) -> Result<FetchOne, FbError> {
    let op_code = next_op(r, lazy)?;

    if op_code == WireOp::Response as u32 {
        return Err(match response_body(r)? {
            Err(e) => e,
            Ok(_) => "fetch: server replied op_response without an error".into(),
        });
    }

    if op_code != WireOp::FetchResponse as u32 {
        return err_conn_rejected(op_code);
    }

    // [status][messages], then the row when messages is 1. Both header words
    // are always sent, end of cursor included.
    let status = r.get_u32()?;
    if status == 100 {
        r.get_u32()?;
        return Ok(FetchOne::End);
    }

    if r.clone().get_u32()? == 0 {
        r.advance(4)?;
        return Ok(FetchOne::BatchEnd);
    }

    Ok(FetchOne::Row(parse_sql_response(r, xsqlda, version, charset)?))
}

/// Computes the SRP client proof (hex-encoded, as the server expects it) and
/// the session key the wire encryption is keyed with.
fn srp_proof<D>(
    srp: SrpClient<D>,
    user: &str,
    pass: &str,
    data: &SrpAuthData,
) -> Result<(Vec<u8>, Vec<u8>), FbError>
where
    D: digest::Digest,
{
    // Generate a private key with the salt received from the server
    let private_key = srp_private_key::<sha1::Sha1>(user.as_bytes(), pass.as_bytes(), &data.salt);

    // Generate a verified with the private key above and the server public key received
    let verifier = srp
        .process_reply(user.as_bytes(), &data.salt, &private_key, &data.pub_key)
        .map_err(|e| FbError::from(format!("Srp error: {}", e)))?;

    // Generate a proof to send to the server so it can verify the password
    let proof = hex::encode(verifier.get_proof()).into_bytes();

    Ok((proof, verifier.get_key().to_vec()))
}

/// Performs the srp authentication with the server, returning the encrypted
/// stream. A proof the server turns down is not an error yet: it answers
/// op_cont_auth naming the next plugin on its list, returned here for another
/// round, and refuses the login only once it has run out of them.
fn srp_auth<D>(
    mut socket: FbStream,
    buff: &mut [u8],
    pending: &mut Bytes,
    srp: SrpClient<D>,
    plugin: AuthPluginType,
    user: &str,
    pass: &str,
    data: SrpAuthData,
) -> Result<(FbStream, Option<crate::wire::AuthPlugin>), FbError>
where
    D: digest::Digest,
{
    let (proof, key) = srp_proof(srp, user, pass, &data)?;

    // Send proof data
    let req = cont_auth(&proof, plugin, AuthPluginType::plugin_list(), &[]);
    trace("op_cont_auth(proof) ->", &req);
    socket.write_all(&req)?;
    socket.flush()?;

    let next = read_reply(&mut socket, buff, pending, &mut 0, |r, _| {
        let mut peek = r.clone();
        if peek.get_u32()? == WireOp::ContAuth as u32 {
            return Ok(Ok(Some(parse_cont_auth(r)?)));
        }
        Ok(op_response(r, &mut 0)?.map(|_| None))
    })??;
    if next.is_some() {
        return Ok((socket, next));
    }

    // Enable wire encryption
    socket.write_all(&crypt("Arc4", "Symmetric"))?;
    socket.flush()?;

    socket = FbStream::Arc4(Arc4Stream::new(
        match socket {
            FbStream::Plain(s) => s,
            _ => unreachable!("Stream was already encrypted!"),
        },
        &key,
        buff.len(),
    ));

    read_reply(&mut socket, buff, pending, &mut 0, op_response)??;

    Ok((socket, None))
}

/// What the server answers an op_attach with
enum AttachReply {
    Attached(Response),
    ContAuth(crate::wire::AuthPlugin),
}

#[derive(Debug, Clone, Copy)]
/// A database handle
pub struct DbHandle(u32);

#[derive(Debug, Clone, Copy)]
/// A transaction handle
pub struct TrHandle(u32);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
/// A statement handle
pub struct StmtHandle(u32);

#[derive(Debug, Clone, Copy)]
/// A blob handle
pub struct BlobHandle(u32);

#[derive(Debug, Clone, Copy)]
/// A blob Identificator
pub struct BlobId(pub(crate) u64);

/// Firebird tcp stream, may be encrypted
enum FbStream {
    /// Plaintext stream
    Plain(TcpStream),

    /// Arc4 ecrypted stream
    Arc4(Arc4Stream<TcpStream>),
}

impl Read for FbStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            FbStream::Plain(s) => s.read(buf),
            FbStream::Arc4(s) => s.read(buf),
        }
    }
}

impl Write for FbStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            FbStream::Plain(s) => s.write(buf),
            FbStream::Arc4(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            FbStream::Plain(s) => s.flush(),
            FbStream::Arc4(s) => s.flush(),
        }
    }
}

#[test]
#[ignore]
fn connection_test() {
    use rsfbclient_core::charset::UTF_8;

    let db_name = "test.fdb";
    let user = "SYSDBA";
    let pass = "masterkey";

    let mut conn = FirebirdWireConnection::connect(
        "127.0.0.1",
        3050,
        db_name,
        user,
        pass,
        UTF_8,
        AuthPlugin::Srp,
    )
    .unwrap();

    let mut db_handle = conn
        .attach_database(db_name, user, pass, None, Dialect::D3, false)
        .unwrap();

    let mut tr_handle = conn
        .begin_transaction(&mut db_handle, TransactionConfiguration::default())
        .unwrap();

    let (stmt_type, mut stmt_handle) = conn
        .prepare_statement(
            &mut db_handle,
            &mut tr_handle,
            Dialect::D3,
            "
            SELECT
                1, 'abcdefghij' as tst, rand(), CURRENT_DATE, CURRENT_TIME, CURRENT_TIMESTAMP, -1, -2, -3, -4, -5, 1, 2, 3, 4, 5, 0 as last
            FROM RDB$DATABASE where 1 = ?
            ",
            // "
            // SELECT cast(1 as bigint), cast('abcdefghij' as varchar(10)) as tst FROM RDB$DATABASE UNION ALL
            // SELECT cast(2 as bigint), cast('abcdefgh' as varchar(10)) as tst FROM RDB$DATABASE UNION ALL
            // SELECT cast(3 as bigint), cast('abcdef' as varchar(10)) as tst FROM RDB$DATABASE UNION ALL
            // SELECT cast(4 as bigint), cast(null as varchar(10)) as tst FROM RDB$DATABASE UNION ALL
            // SELECT cast(null as bigint), cast('abcd' as varchar(10)) as tst FROM RDB$DATABASE
            // ",
        )
        .unwrap();

    println!("Statement type: {:?}", stmt_type);

    let params = match rsfbclient_core::IntoParams::to_params((1,)) {
        rsfbclient_core::ParamsType::Positional(params) => params,
        _ => unreachable!(),
    };

    conn.execute(&mut tr_handle, &mut stmt_handle, &params)
        .unwrap();

    loop {
        let resp = conn.fetch(&mut tr_handle, &mut stmt_handle).unwrap();

        if resp.is_none() {
            break;
        }
        println!("Fetch Resp: {:#?}", resp);
    }

    std::thread::sleep(std::time::Duration::from_millis(100));
}
