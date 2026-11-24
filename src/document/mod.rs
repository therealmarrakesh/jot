mod decode;
mod encode;
mod path;
mod save;

pub(crate) use encode::is_lossy;
pub(crate) use path::{DocPath, PathKey, default_directory, display_name, file_name};
pub(crate) use save::write_rope;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::platform::AnsiCodePage;

use decode::load_reader_with_page;

pub(super) const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
pub(super) const UTF16_LE_BOM: &[u8] = b"\xff\xfe";
pub(super) const UTF16_BE_BOM: &[u8] = b"\xfe\xff";
pub(super) const IO_CHUNK: usize = 64 * 1024;
pub(super) const ANSI_WIDE_CHUNK: usize = 32 * 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) const UNTITLED: &str = "Untitled";

#[derive(Debug)]
pub(crate) enum SaveError {
    ReadOnly,
    Kept {
        source: io::Error,
        backup: PathBuf,
        temp: PathBuf,
    },
    Io(io::Error),
}

impl From<io::Error> for SaveError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum LineEnding {
    #[default]
    Crlf,
    Lf,
    Cr,
}

impl LineEnding {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Crlf => "Windows (CRLF)",
            Self::Lf => "Unix (LF)",
            Self::Cr => "Macintosh (CR)",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Ansi,
}

impl Encoding {
    pub(crate) const SAVE_ORDER: [Self; 5] = [
        Self::Ansi,
        Self::Utf16Le,
        Self::Utf16Be,
        Self::Utf8,
        Self::Utf8Bom,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8 with BOM",
            Self::Utf16Le => "UTF-16 LE",
            Self::Utf16Be => "UTF-16 BE",
            Self::Ansi => "ANSI",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) line_ending: LineEnding,
    pub(crate) encoding: Encoding,
    pub(crate) mixed_line_endings: bool,
    pub(crate) lossy_decode: bool,
}

impl Format {
    pub(crate) fn line_ending_label(self) -> &'static str {
        if self.mixed_line_endings {
            match self.line_ending {
                LineEnding::Crlf => "Mixed (CRLF)",
                LineEnding::Lf => "Mixed (LF)",
                LineEnding::Cr => "Mixed (CR)",
            }
        } else {
            self.line_ending.label()
        }
    }
}

pub(crate) struct Loaded {
    pub(crate) doc_path: DocPath,
    pub(crate) text: Rope,
    pub(crate) format: Format,
}

#[derive(Debug)]
pub(crate) enum LoadError {
    Io(io::Error),
    Undecodable,
    TooLarge(u64),
}

pub(crate) fn load(path: &Path) -> Result<Loaded, LoadError> {
    let mut file = fs::File::open(path).map_err(LoadError::Io)?;
    if let Ok(meta) = file.metadata()
        && meta.len() > MAX_FILE_BYTES
    {
        return Err(LoadError::TooLarge(meta.len()));
    }
    let decoded = load_reader_with_page(&mut file, IO_CHUNK, AnsiCodePage::current())?;
    Ok(Loaded {
        doc_path: DocPath::new(path.to_path_buf()),
        text: decoded.text,
        format: decoded.format,
    })
}

#[cfg(test)]
mod tests {
    use ropey::RopeBuilder;

    use super::decode::Decoded;
    use super::encode::{is_lossy_with_page, stream_encode, stream_encode_with_page};
    use super::path::normalize_path_key;
    use super::save::{BACKUP_SUFFIX, sibling};
    use super::*;

    fn load_bytes(bytes: &[u8], chunk_size: usize) -> Result<Decoded, LoadError> {
        let mut cursor = io::Cursor::new(bytes);
        load_reader_with_page(&mut cursor, chunk_size, AnsiCodePage::current())
    }

    fn load_bytes_with_page(
        bytes: &[u8],
        chunk_size: usize,
        page: AnsiCodePage,
    ) -> Result<Decoded, LoadError> {
        let mut cursor = io::Cursor::new(bytes);
        load_reader_with_page(&mut cursor, chunk_size, page)
    }

    struct CleanupDir(PathBuf);
    impl Drop for CleanupDir {
        fn drop(&mut self) {
            if let Ok(entries) = fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    if let Ok(meta) = entry.metadata() {
                        let mut perms = meta.permissions();
                        if perms.readonly() {
                            #[allow(clippy::permissions_set_readonly_false)]
                            perms.set_readonly(false);
                            let _ = fs::set_permissions(entry.path(), perms);
                        }
                    }
                }
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn detects_boms_and_utf8_vs_ansi() {
        assert_eq!(load_bytes(b"hello", 4).unwrap().format.encoding, Encoding::Utf8);
        assert_eq!(
            load_bytes(b"\xef\xbb\xbfhello", 4).unwrap().format.encoding,
            Encoding::Utf8Bom
        );
        assert_eq!(
            load_bytes(b"\xff\xfeh\0", 4).unwrap().format.encoding,
            Encoding::Utf16Le
        );
        assert_eq!(
            load_bytes(b"\xfe\xff\0h", 4).unwrap().format.encoding,
            Encoding::Utf16Be
        );
        assert!(matches!(
            load_bytes(b"\xff\xfe\x00\x00a\0\0\0", 4),
            Err(LoadError::Undecodable)
        ));
        let bomless_le = load_bytes(b"a\0b\0c\0", 4).unwrap();
        assert_eq!(bomless_le.format.encoding, Encoding::Utf16Le);
        assert_eq!(bomless_le.text.to_string(), "abc");
        let bomless_be = load_bytes(b"\0a\0b\0c", 4).unwrap();
        assert_eq!(bomless_be.format.encoding, Encoding::Utf16Be);
        assert_eq!(bomless_be.text.to_string(), "abc");

        let mut ansi = vec![b'a'; 20];
        ansi.extend_from_slice(&[0x80, 0xFF]);
        let loaded = load_bytes_with_page(&ansi, 5, AnsiCodePage::from_raw(1252)).unwrap();
        assert_eq!(loaded.format.encoding, Encoding::Ansi);
    }

    #[test]
    fn injected_ansi_code_pages_round_trip_and_flag_lossy() {
        let cp1252 = AnsiCodePage::from_raw(1252);
        let all_bytes: Vec<u8> = (0u8..=255u8)
            .filter(|&b| b != b'\r' && b != b'\n')
            .collect();
        let loaded = load_bytes_with_page(&all_bytes, 16, cp1252).unwrap();
        assert_eq!(loaded.format.encoding, Encoding::Ansi);
        assert!(!loaded.format.lossy_decode);
        assert!(!is_lossy_with_page(&loaded.text, loaded.format, cp1252).unwrap());
        let mut reencoded = Vec::new();
        stream_encode_with_page(&loaded.text, loaded.format, cp1252, &mut reencoded).unwrap();
        assert_eq!(reencoded, all_bytes);

        let ansi_fmt = Format {
            encoding: Encoding::Ansi,
            ..Default::default()
        };
        assert!(!is_lossy_with_page(&Rope::from("café"), ansi_fmt, cp1252).unwrap());
        assert!(is_lossy_with_page(&Rope::from("hello 😀"), ansi_fmt, cp1252).unwrap());

        let cp65001 = AnsiCodePage::from_raw(65001);
        let latin1 = b"caf\xe9";
        let loaded_utf8_acp = load_bytes_with_page(latin1, 8, cp65001).unwrap();
        assert!(loaded_utf8_acp.format.lossy_decode);
        assert!(
            is_lossy_with_page(&loaded_utf8_acp.text, loaded_utf8_acp.format, cp65001).unwrap()
        );

        let cp932 = AnsiCodePage::from_raw(932);
        let invalid_sjis = [0x81u8, 0x00u8, 0xFFu8];
        assert!(matches!(
            load_bytes_with_page(&invalid_sjis, 8, cp932),
            Err(LoadError::Undecodable)
        ));
    }

    #[test]
    fn splits_and_normalizes_line_endings_across_chunk_boundaries() {
        for chunk in [4, 5, 7, 64] {
            let plain = load_bytes(b"plain", chunk).unwrap();
            assert_eq!(plain.text.to_string(), "plain");
            assert_eq!(plain.format.line_ending, LineEnding::Crlf);
            assert!(!plain.format.mixed_line_endings);

            let crlf = load_bytes(b"a\r\nb\r\n", chunk).unwrap();
            assert_eq!(crlf.text.to_string(), "a\nb\n");
            assert_eq!(crlf.format.line_ending, LineEnding::Crlf);
            assert!(!crlf.format.mixed_line_endings);

            let lf = load_bytes(b"a\nb\n", chunk).unwrap();
            assert_eq!(lf.text.to_string(), "a\nb\n");
            assert_eq!(lf.format.line_ending, LineEnding::Lf);
            assert!(!lf.format.mixed_line_endings);

            let cr = load_bytes(b"a\rb\r", chunk).unwrap();
            assert_eq!(cr.text.to_string(), "a\nb\n");
            assert_eq!(cr.format.line_ending, LineEnding::Cr);
            assert!(!cr.format.mixed_line_endings);

            let mixed = load_bytes(b"a\r\r\nb\r", chunk).unwrap();
            assert_eq!(mixed.text.to_string(), "a\n\nb\n");
            assert_eq!(mixed.format.line_ending, LineEnding::Cr);
            assert!(mixed.format.mixed_line_endings);
        }
    }

    #[test]
    fn streaming_encode_and_decode_round_trip_including_surrogates() {
        let sample = "hello\nworld — 日本語 😀\nend";
        let rope = Rope::from(sample);
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            for line_ending in [LineEnding::Crlf, LineEnding::Lf, LineEnding::Cr] {
                let format = Format {
                    line_ending,
                    encoding,
                    ..Default::default()
                };
                assert!(!is_lossy(&rope, format).unwrap());
                let mut bytes = Vec::new();
                stream_encode(&rope, format, &mut bytes).unwrap();
                for chunk in [4, 5, 7, 64] {
                    let loaded = load_bytes(&bytes, chunk).unwrap();
                    assert_eq!(loaded.text.to_string(), sample);
                    assert_eq!(loaded.format, format);
                    let mut reencoded = Vec::new();
                    stream_encode(&loaded.text, loaded.format, &mut reencoded).unwrap();
                    assert_eq!(reencoded, bytes, "byte-exact round-trip mismatch");
                }
            }
        }
        assert!(load_bytes(&[0xff, 0xfe, b'a'], 4).is_err());
        assert!(load_bytes(&[0xef, 0xbb, 0xbf, 0xff], 4).is_err());
    }

    #[test]
    fn same_path_ignores_windows_casing_and_separators() {
        assert_eq!(
            PathKey::new(Path::new(r"C:\Notes\Todo.txt")),
            PathKey::new(Path::new(r"c:/notes/todo.txt")),
        );
        assert_eq!(
            normalize_path_key(Path::new(r"\\?\C:\Notes\Todo.txt")),
            normalize_path_key(Path::new(r"c:\notes\todo.txt")),
        );
        assert_ne!(
            PathKey::new(Path::new(r"C:\Notes\Todo.txt")),
            PathKey::new(Path::new(r"C:\Notes\Other.txt")),
        );
    }

    #[test]
    fn load_and_atomic_write_round_trip_and_respect_readonly() {
        let dir = std::env::temp_dir().join(format!("jot-doc-test-{}", std::process::id()));
        let _cleanup = CleanupDir(dir.clone());
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.txt");

        let format = Format {
            line_ending: LineEnding::Lf,
            encoding: Encoding::Utf8Bom,
            ..Default::default()
        };
        write_rope(&file, &Rope::from("first\nline"), format).unwrap();
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.text.to_string(), "first\nline");
        assert_eq!(loaded.format.encoding, Encoding::Utf8Bom);
        assert_eq!(loaded.format.line_ending, LineEnding::Lf);

        write_rope(&file, &Rope::from("second\nline"), format).unwrap();
        let loaded = load(&file).unwrap();
        assert_eq!(loaded.text.to_string(), "second\nline");

        let entries: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries.len(), 1);

        let long_name = format!("{}.txt", "a".repeat(245));
        let long_sibling = sibling(&dir.join(&long_name), 3, BACKUP_SUFFIX);
        let sib_len = long_sibling.file_name().unwrap().to_string_lossy().len();
        assert!(sib_len < 240, "sibling filename too long: {sib_len}");

        {
            use std::os::windows::fs::OpenOptionsExt as _;
            let _holder = fs::OpenOptions::new()
                .read(true)
                .share_mode(1 | 2)
                .open(&file)
                .unwrap();
            write_rope(&file, &Rope::from("in-place fallback"), format).unwrap();
        }
        assert_eq!(load(&file).unwrap().text.to_string(), "in-place fallback");

        let original = fs::metadata(&file).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&file, readonly).unwrap();
        let err = write_rope(&file, &Rope::from("blocked"), format).unwrap_err();
        assert!(matches!(err, SaveError::ReadOnly));
    }

    #[test]
    fn multi_chunk_streaming_round_trip() {
        let dir = std::env::temp_dir().join(format!("jot-doc-large-{}", std::process::id()));
        let _cleanup = CleanupDir(dir.clone());
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("large.txt");

        let mut builder = RopeBuilder::new();
        for i in 0..4000 {
            builder.append(&format!("line {i:04}: alpha beta — 日本語 😀\n"));
        }
        let original = builder.finish();
        assert!(original.len() > IO_CHUNK * 2);

        for format in [
            Format {
                line_ending: LineEnding::Crlf,
                encoding: Encoding::Utf8,
                ..Default::default()
            },
            Format {
                line_ending: LineEnding::Lf,
                encoding: Encoding::Utf8Bom,
                ..Default::default()
            },
            Format {
                line_ending: LineEnding::Crlf,
                encoding: Encoding::Utf16Le,
                ..Default::default()
            },
            Format {
                line_ending: LineEnding::Cr,
                encoding: Encoding::Utf16Be,
                ..Default::default()
            },
        ] {
            write_rope(&file, &original, format).unwrap();
            let loaded = load(&file).unwrap();
            assert_eq!(loaded.format, format);
            assert_eq!(loaded.text, original);
        }
    }
}
