//! Scripts as files: read into an editor and written back the way they were
//! found. A script that came from IBExpert on Windows is as likely WIN1252
//! with CRLF as UTF-8, and saving it must not turn every line of it into a
//! difference.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::commands::blocking;
use crate::error::{AppError, AppResult};

/// Past this a file is not something to type in: the editor would hold it
/// several times over
const LARGEST: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Utf8,
    /// UTF-8 behind a byte order mark, as Windows tools write it
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Win1252,
}

/// How a file's text is laid down in bytes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Format {
    pub encoding: Encoding,
    /// Lines end in CR LF. The editor's text has LF alone.
    pub crlf: bool,
}

#[derive(Debug, Serialize)]
pub struct ScriptFile {
    text: String,
    format: Format,
}

/// What WIN1252 has at 0x80 to 0x9F; elsewhere it is Latin-1. The five
/// bytes it leaves undefined read as the control characters of the same
/// number, which is what brings them back unchanged.
const WIN1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{81}', '\u{201A}', '\u{192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{2C6}',
    '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{8D}', '\u{17D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}',
    '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}', '\u{2DC}', '\u{2122}', '\u{161}', '\u{203A}',
    '\u{153}', '\u{9D}', '\u{17E}', '\u{178}',
];

fn from_win1252(byte: u8) -> char {
    match byte {
        0x80..=0x9F => WIN1252_HIGH[(byte - 0x80) as usize],
        _ => byte as char,
    }
}

fn to_win1252(c: char) -> Option<u8> {
    match c as u32 {
        0..=0x7F | 0xA0..=0xFF => Some(c as u8),
        _ => WIN1252_HIGH.iter().position(|h| *h == c).map(|i| 0x80 + i as u8),
    }
}

/// The text of a file's bytes and the encoding they were taken to be in: by
/// the byte order mark when there is one, UTF-8 when they are valid UTF-8,
/// and WIN1252 otherwise, which every sequence of bytes is.
fn decode(bytes: &[u8]) -> (String, Encoding) {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units = rest.chunks_exact(2).map(|pair| unit([pair[0], pair[1]]));
        char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect::<String>()
    };
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if let Ok(text) = std::str::from_utf8(rest) {
            return (text.to_owned(), Encoding::Utf8Bom);
        }
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (utf16(rest, u16::from_le_bytes), Encoding::Utf16Le);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (utf16(rest, u16::from_be_bytes), Encoding::Utf16Be);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), Encoding::Utf8),
        Err(_) => (bytes.iter().map(|b| from_win1252(*b)).collect(), Encoding::Win1252),
    }
}

/// The bytes of `text` in `encoding`, or the first character it cannot hold
fn encode(text: &str, encoding: Encoding) -> Result<Vec<u8>, char> {
    let utf16 = |bom: [u8; 2], bytes: fn(u16) -> [u8; 2]| {
        let mut out = bom.to_vec();
        out.extend(text.encode_utf16().flat_map(bytes));
        out
    };
    Ok(match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [&[0xEF, 0xBB, 0xBF], text.as_bytes()].concat(),
        Encoding::Utf16Le => utf16([0xFF, 0xFE], u16::to_le_bytes),
        Encoding::Utf16Be => utf16([0xFE, 0xFF], u16::to_be_bytes),
        Encoding::Win1252 => text.chars().map(|c| to_win1252(c).ok_or(c)).collect::<Result<_, _>>()?,
    })
}

fn read(path: &Path) -> AppResult<ScriptFile> {
    let size = fs::metadata(path).map_err(|e| AppError::storage(path, e))?.len();
    if size > LARGEST {
        return Err(AppError::Invalid {
            message: format!(
                "{} has {} MB; an editor takes files up to {} MB.",
                path.display(),
                size / (1024 * 1024),
                LARGEST / (1024 * 1024)
            ),
        });
    }
    let bytes = fs::read(path).map_err(|e| AppError::storage(path, e))?;
    let (text, encoding) = decode(&bytes);
    let crlf = text.contains("\r\n");
    let text = if crlf { text.replace("\r\n", "\n") } else { text };
    Ok(ScriptFile { text, format: Format { encoding, crlf } })
}

/// Writes `text` as `format` says, or as UTF-8 where the format's encoding
/// lacks one of its characters: a file in another encoding than it had is
/// a lesser harm than one without what was typed. Answers the format
/// written.
fn write(path: &Path, text: &str, format: Format) -> AppResult<Format> {
    let text = if format.crlf { text.replace('\n', "\r\n") } else { text.to_owned() };
    let (bytes, encoding) = match encode(&text, format.encoding) {
        Ok(bytes) => (bytes, format.encoding),
        Err(_) => (text.into_bytes(), Encoding::Utf8),
    };
    fs::write(path, bytes).map_err(|e| AppError::storage(path, e))?;
    Ok(Format { encoding, crlf: format.crlf })
}

/// Replaces one of the app's own files through a temporary one and a rename,
/// so a crash mid-write cannot leave half a file behind
pub fn replace(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let dir = path.parent().expect("the file lives in a directory");
    fs::create_dir_all(dir).map_err(|e| AppError::storage(dir, e))?;
    let mut name = path.file_name().expect("the file has a name").to_owned();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    let mut file = fs::File::create(&tmp).map_err(|e| AppError::storage(&tmp, e))?;
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|e| AppError::storage(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| AppError::storage(path, e))
}

/// A script's text, and how the file holds it
#[tauri::command]
pub async fn script_read(app: AppHandle, path: PathBuf) -> AppResult<ScriptFile> {
    blocking(app, move |_| read(&path)).await
}

/// Writes an editor's text to a file; the format it was written in comes
/// back, which is not the one asked for when that could not hold the text
#[tauri::command]
pub async fn script_write(app: AppHandle, path: PathBuf, text: String, format: Format) -> AppResult<Format> {
    blocking(app, move |_| write(&path, &text, format)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTF8: Format = Format { encoding: Encoding::Utf8, crlf: false };

    fn round_trip(bytes: &[u8]) -> Format {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sql");
        fs::write(&path, bytes).unwrap();
        let file = read(&path).unwrap();
        assert!(!file.text.contains('\r'), "the editor's text has LF alone");
        assert_eq!(write(&path, &file.text, file.format).unwrap(), file.format);
        assert_eq!(fs::read(&path).unwrap(), bytes, "a file saved untouched is the file it was");
        file.format
    }

    #[test]
    fn a_file_comes_back_as_it_was_found() {
        let text = "select 'ação' from rdb$database;\r\n-- fim\r\n";
        assert_eq!(round_trip(text.as_bytes()), Format { encoding: Encoding::Utf8, crlf: true });
        assert_eq!(round_trip("select 1;\n".as_bytes()), UTF8);
        assert_eq!(round_trip(b"").encoding, Encoding::Utf8);

        let bom = [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat();
        assert_eq!(round_trip(&bom).encoding, Encoding::Utf8Bom);

        let le: Vec<u8> = [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(round_trip(&le).encoding, Encoding::Utf16Le);
        let be: Vec<u8> = [0xFE, 0xFF].into_iter().chain(text.encode_utf16().flat_map(u16::to_be_bytes)).collect();
        assert_eq!(round_trip(&be).encoding, Encoding::Utf16Be);
    }

    #[test]
    fn what_is_not_utf8_is_win1252() {
        // 'ação — fim' as Windows wrote it
        let bytes = [b'a', 0xE7, 0xE3, b'o', b' ', 0x97, b' ', b'f', b'i', b'm', b'\n'];
        assert_eq!(decode(&bytes), ("ação \u{2014} fim\n".to_string(), Encoding::Win1252));
        assert_eq!(round_trip(&bytes).encoding, Encoding::Win1252);
        // Every byte reads, and reads back: the ones WIN1252 leaves undefined too
        let all: Vec<u8> = (0x80..=0xFF).collect();
        assert_eq!(round_trip(&all).encoding, Encoding::Win1252);
    }

    #[test]
    fn text_its_encoding_cannot_hold_is_written_as_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sql");
        let format = Format { encoding: Encoding::Win1252, crlf: true };
        assert_eq!(write(&path, "ação\n", format).unwrap(), format);
        assert_eq!(fs::read(&path).unwrap(), [b'a', 0xE7, 0xE3, b'o', b'\r', b'\n']);

        let written = write(&path, "ação ✓\n", format).unwrap();
        assert_eq!(written, Format { encoding: Encoding::Utf8, crlf: true });
        assert_eq!(fs::read(&path).unwrap(), "ação ✓\r\n".as_bytes());
    }

    #[test]
    fn a_file_too_large_to_edit_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql");
        fs::File::create(&path).unwrap().set_len(LARGEST + 1).unwrap();
        assert!(matches!(read(&path), Err(AppError::Invalid { .. })));
    }

    #[test]
    fn replacing_leaves_no_temporary_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("state.json");
        replace(&path, b"one").unwrap();
        replace(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
}
