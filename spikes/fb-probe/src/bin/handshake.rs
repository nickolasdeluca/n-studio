//! Raw Firebird `op_connect` dumper.
//!
//! The driver collapses every handshake failure into one opaque message, so
//! this speaks the wire protocol directly and prints the server's actual
//! status vector. Auth is never attempted — this stops at the connect reply.
//!
//!   FB_HOST=h FB_PORT=p FB_DB=path cargo run -p fb-probe --bin handshake

use std::env;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

const OP_CONNECT: u32 = 1;
const OP_ATTACH: u32 = 19;
const OP_ACCEPT: u32 = 3;
const OP_RESPONSE: u32 = 9;
const OP_ACCEPT_DATA: u32 = 65;
const OP_COND_ACCEPT: u32 = 66;
const CONNECT_VERSION: u32 = 3;
const ARCH_GENERIC: u32 = 1;

const CNCT_USER: u8 = 1;
const CNCT_HOST: u8 = 4;
const CNCT_USER_VERIFICATION: u8 = 6;
const CNCT_SPECIFIC_DATA: u8 = 7;
const CNCT_PLUGIN_NAME: u8 = 8;
const CNCT_LOGIN: u8 = 9;
const CNCT_PLUGIN_LIST: u8 = 10;
const CNCT_CLIENT_CRYPT: u8 = 11;

fn main() {
    let host = env::var("FB_HOST").unwrap_or_else(|_| "localhost".into());
    let port: u16 = env::var("FB_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(3050);
    let db = env::var("FB_DB").unwrap_or_else(|_| "fixture.fdb".into());
    let user = env::var("FB_USER").unwrap_or_else(|_| "SYSDBA".into());
    // Wire-encryption level advertised to the server: 0 disabled, 1 enabled, 2 required.
    let crypt_level: u32 = env::var("FB_CRYPT").ok().and_then(|c| c.parse().ok()).unwrap_or(1);
    // Highest protocol version to offer.
    let max_proto: u32 = env::var("FB_MAXPROTO").ok().and_then(|p| p.parse().ok()).unwrap_or(13);

    println!("connecting to {host}:{port}");
    println!("  db          {db}");
    println!("  crypt level {crypt_level}");
    println!("  protocols   10..={max_proto}");

    let mut sock = match TcpStream::connect((host.as_str(), port)) {
        Ok(s) => s,
        Err(e) => return println!("TCP connect failed: {e}"),
    };
    sock.set_read_timeout(Some(Duration::from_secs(10))).ok();
    sock.set_nodelay(true).ok();

    let packet = build_connect(&db, &user, crypt_level, max_proto);
    println!("\nsending op_connect ({} bytes)", packet.len());
    if let Err(e) = sock.write_all(&packet) {
        return println!("write failed: {e}");
    }

    let mut buf = vec![0u8; 4096];
    let n = match sock.read(&mut buf) {
        Ok(0) => return println!("server closed the connection without replying"),
        Ok(n) => n,
        Err(e) => return println!("read failed: {e}"),
    };
    buf.truncate(n);
    println!("received {n} bytes\n");
    hexdump(&buf);

    let mut c = Cursor { b: &buf, i: 0 };
    let Some(op) = c.u32() else { return println!("reply too short") };
    println!("\nopcode {op} = {}", op_name(op));

    match op {
        OP_ACCEPT | OP_ACCEPT_DATA | OP_COND_ACCEPT => {
            let ver = c.u32().unwrap_or(0);
            let arch = c.u32().unwrap_or(0);
            let typ = c.u32().unwrap_or(0);
            println!("  protocol version {}", ver as i32);
            println!("  arch {arch}, type {typ}");
            if op != OP_ACCEPT {
                let data = c.wire_bytes().unwrap_or_default();
                let plugin = c.wire_bytes().unwrap_or_default();
                println!("  auth plugin      {}", String::from_utf8_lossy(&plugin));
                println!("  auth data        {} bytes", data.len());
            }
            println!("\nHANDSHAKE OK — the server accepted the connect.");

            // Protocol 10 has no SRP exchange: credentials ride in the DPB of
            // op_attach. Sending it here shows the server's real attach error.
            if env::var("FB_ATTACH").is_ok() && ver == 10 {
                let pass = env::var("FB_PASS").unwrap_or_else(|_| "masterkey".into());
                let packet = build_attach(&db, &user, &pass);
                println!("\nsending op_attach ({} bytes)", packet.len());
                if sock.write_all(&packet).is_err() {
                    return println!("attach write failed");
                }
                let mut rbuf = vec![0u8; 8192];
                let n = match sock.read(&mut rbuf) {
                    Ok(0) => return println!("server closed the connection"),
                    Ok(n) => n,
                    Err(e) => return println!("attach read failed: {e}"),
                };
                rbuf.truncate(n);
                println!("received {n} bytes\n");
                hexdump(&rbuf);
                let mut rc = Cursor { b: &rbuf, i: 0 };
                let rop = rc.u32().unwrap_or(0);
                println!("\nopcode {rop} = {}", op_name(rop));
                if rop == OP_RESPONSE {
                    let handle = rc.u32().unwrap_or(0);
                    let _blob_hi = rc.u32();
                    let _blob_lo = rc.u32();
                    let _data = rc.wire_bytes();
                    println!("  db handle {handle}");
                    println!("\nstatus vector:");
                    print_status_vector(&mut rc);
                }
            }
        }
        OP_RESPONSE => {
            let _handle = c.u32();
            let _blob_hi = c.u32();
            let _blob_lo = c.u32();
            let _data = c.wire_bytes();
            println!("\nserver returned an error status vector:");
            print_status_vector(&mut c);
        }
        _ => println!("  unexpected opcode — server rejected the connect"),
    }
}

fn build_connect(db: &str, user: &str, crypt_level: u32, max_proto: u32) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(OP_CONNECT.to_be_bytes());
    p.extend(OP_ATTACH.to_be_bytes());
    p.extend(CONNECT_VERSION.to_be_bytes());
    p.extend(ARCH_GENERIC.to_be_bytes());
    put_wire_bytes(&mut p, db.as_bytes());

    let protocols: Vec<[u32; 5]> = (10..=max_proto.min(13))
        .map(|v| [v, 1, 0, 5, (v - 8) * 2])
        .collect();
    p.extend((protocols.len() as u32).to_be_bytes());

    let mut uid = Vec::new();
    tag(&mut uid, CNCT_LOGIN, user.as_bytes());
    tag(&mut uid, CNCT_PLUGIN_NAME, b"Srp256");
    tag(&mut uid, CNCT_PLUGIN_LIST, b"Srp256,Srp");
    // Throwaway public key: auth is never completed, the server only needs a
    // well-formed tag at this stage.
    let pubkey = "00".repeat(128);
    for (i, chunk) in pubkey.as_bytes().chunks(254).enumerate() {
        let mut v = vec![i as u8];
        v.extend_from_slice(chunk);
        tag(&mut uid, CNCT_SPECIFIC_DATA, &v);
    }
    tag(&mut uid, CNCT_CLIENT_CRYPT, &crypt_level.to_le_bytes());
    tag(&mut uid, CNCT_USER, b"fbexpert");
    tag(&mut uid, CNCT_HOST, b"probe");
    tag(&mut uid, CNCT_USER_VERIFICATION, b"");
    put_wire_bytes(&mut p, &uid);

    for proto in &protocols {
        for field in proto {
            p.extend(field.to_be_bytes());
        }
    }
    p
}

/// op_attach with a protocol-10 DPB: plaintext credentials, no SRP.
fn build_attach(db: &str, user: &str, pass: &str) -> Vec<u8> {
    const ISC_DPB_VERSION1: u8 = 1;
    const ISC_DPB_USER_NAME: u8 = 28;
    const ISC_DPB_PASSWORD: u8 = 29;
    const ISC_DPB_LC_CTYPE: u8 = 48;

    const ISC_DPB_SQL_DIALECT: u8 = 63;

    let charset = env::var("FB_CHARSET").unwrap_or_else(|_| "WIN1252".into());
    let mut dpb = vec![ISC_DPB_VERSION1];
    for (t, v) in [
        (ISC_DPB_LC_CTYPE, charset.as_str()),
        (ISC_DPB_USER_NAME, user),
    ] {
        dpb.push(t);
        dpb.push(v.len() as u8);
        dpb.extend_from_slice(v.as_bytes());
    }

    // The driver always sends a dialect tag here; ours is opt-in so the two
    // DPBs can be compared tag by tag.
    if let Ok(d) = env::var("FB_DPB_DIALECT") {
        if let Ok(d) = d.parse::<u8>() {
            dpb.extend_from_slice(&[ISC_DPB_SQL_DIALECT, 1, d]);
        }
    }

    dpb.push(ISC_DPB_PASSWORD);
    dpb.push(pass.len() as u8);
    dpb.extend_from_slice(pass.as_bytes());

    let mut p = Vec::new();
    p.extend(OP_ATTACH.to_be_bytes());
    p.extend(0u32.to_be_bytes());
    put_wire_bytes(&mut p, db.as_bytes());
    put_wire_bytes(&mut p, &dpb);
    p
}

fn tag(buf: &mut Vec<u8>, tag: u8, data: &[u8]) {
    buf.push(tag);
    buf.push(data.len() as u8);
    buf.extend_from_slice(data);
}

fn put_wire_bytes(buf: &mut Vec<u8>, data: &[u8]) {
    buf.extend((data.len() as u32).to_be_bytes());
    buf.extend_from_slice(data);
    let pad = (4 - data.len() % 4) % 4;
    buf.extend(std::iter::repeat(0u8).take(pad));
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> Option<u32> {
        let s = self.b.get(self.i..self.i + 4)?;
        self.i += 4;
        Some(u32::from_be_bytes(s.try_into().ok()?))
    }
    fn wire_bytes(&mut self) -> Option<Vec<u8>> {
        let len = self.u32()? as usize;
        let s = self.b.get(self.i..self.i + len)?.to_vec();
        self.i += len + (4 - len % 4) % 4;
        Some(s)
    }
}

/// Firebird status vector: clumplets until isc_arg_end.
fn print_status_vector(c: &mut Cursor) {
    loop {
        let Some(arg) = c.u32() else { return };
        match arg {
            0 => return,
            1 => {
                let Some(code) = c.u32() else { return };
                println!("  gds code   {code}  {}", gds_hint(code));
            }
            2 | 5 => {
                let s = c.wire_bytes().unwrap_or_default();
                println!("  text       {}", String::from_utf8_lossy(&s));
            }
            4 => {
                let Some(n) = c.u32() else { return };
                println!("  number     {n}");
            }
            19 => {
                let s = c.wire_bytes().unwrap_or_default();
                println!("  sqlstate   {}", String::from_utf8_lossy(&s));
            }
            other => {
                println!("  arg type   {other} (stopping)");
                return;
            }
        }
    }
}

fn gds_hint(code: u32) -> &'static str {
    match code {
        335544344 => "isc_io_error — file open failed (wrong path?)",
        335544375 => "isc_unavailable — server refused / shutting down",
        335544421 => "isc_conn_lost",
        335544472 => "isc_login — bad user name or password",
        335544734 => "isc_wrong_ods — unsupported on-disk structure",
        335545106 => "wire encryption required by the server",
        _ => "",
    }
}

fn op_name(op: u32) -> &'static str {
    match op {
        3 => "op_accept",
        9 => "op_response",
        18 => "op_reject",
        65 => "op_accept_data",
        66 => "op_cond_accept",
        _ => "unknown",
    }
}

fn hexdump(b: &[u8]) {
    for (i, chunk) in b.chunks(16).take(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|x| format!("{x:02x}")).collect();
        let ascii: String = chunk
            .iter()
            .map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '.' })
            .collect();
        println!("  {:04x}  {:<48}  {}", i * 16, hex.join(" "), ascii);
    }
}
