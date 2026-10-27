//! Перетаскивание в окно (событие Tauri drag-drop): пути файлов приходят в Rust, интерфейс
//! получает итог. `.ipa` — как «Выбрать файл .ipa…» (`Library::import_ipa`); текстовый файл со
//! ссылками App Store или строками «Имя: N» — номера для «Добавить по ссылке»
//! (`app_link::ids_in_text`). Прочее — `NotIpa`.
use crate::app_link::{MAX_TEXT_IDS, ids_in_text};
use crate::errors::AppError;
use crate::library::{CopyInfo, Library};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Больше текстовый файл не читается: список ссылок столько не весит.
pub const MAX_TEXT_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ipa,
    /// Текст: `.txt`, `.text`, `.md`, `.csv`, ссылки `.webloc` (macOS) и `.url` (Windows).
    Text,
    Other,
}

/// Что за файл — по расширению, без учёта регистра.
pub fn kind(path: &Path) -> Kind {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("ipa") => Kind::Ipa,
        Some("txt" | "text" | "md" | "csv" | "webloc" | "url") => Kind::Text,
        _ => Kind::Other,
    }
}

/// Номера из текстового файла (первый мегабайт). Только обычный файл: папка или FIFO с именем
/// `*.txt` — `NotIpa` (FIFO иначе повесил бы чтение). `.webloc` в двоичном plist — по ключу `URL`;
/// UTF-16 с BOM (Блокнот Windows) раскодируется; прочий не UTF-8 — с заменой символов.
pub fn text_ids(path: &Path) -> Result<Vec<i64>, AppError> {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file()) {
        return Err(AppError::NotIpa);
    }
    let f = std::fs::File::open(path).map_err(|e| AppError::Unknown { detail: format!("dropped text: {e}") })?;
    let mut buf = vec![];
    f.take(MAX_TEXT_BYTES).read_to_end(&mut buf).map_err(|e| AppError::Unknown { detail: format!("dropped text: {e}") })?;
    if let Some(url) = plist_url(&buf) {
        let ids = ids_in_text(&url);
        if !ids.is_empty() {
            return Ok(ids);
        }
    }
    Ok(ids_in_text(&decode(&buf)))
}

/// Ключ `URL` из plist (`.webloc`, в том числе двоичный `bplist00`).
fn plist_url(buf: &[u8]) -> Option<String> {
    let v = plist::Value::from_reader(std::io::Cursor::new(buf)).ok()?;
    v.as_dictionary()?.get("URL")?.as_string().map(str::to_string)
}

/// Текст файла: UTF-16 LE/BE по BOM, иначе UTF-8 с заменой битых байтов.
fn decode(buf: &[u8]) -> String {
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match buf {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        _ => String::from_utf8_lossy(buf).into_owned(),
    }
}

/// Итог одного перетаскивания: принятые копии, номера (по порядку, без повторов, не больше
/// `MAX_TEXT_IDS`) и ошибки без повторов.
#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub copies: Vec<CopyInfo>,
    pub ids: Vec<i64>,
    pub errors: Vec<AppError>,
}

pub fn handle(library: &Library, paths: &[PathBuf]) -> Outcome {
    let mut out = Outcome::default();
    let fail = |out: &mut Outcome, e: AppError| {
        if !out.errors.contains(&e) {
            out.errors.push(e);
        }
    };
    for p in paths {
        match kind(p) {
            Kind::Ipa => match library.import_ipa(p) {
                Ok(c) => out.copies.push(c),
                Err(e) => fail(&mut out, e),
            },
            Kind::Text => match text_ids(p) {
                Ok(ids) if ids.is_empty() => fail(&mut out, AppError::BadLink),
                Ok(ids) => {
                    for id in ids {
                        if out.ids.len() < MAX_TEXT_IDS && !out.ids.contains(&id) {
                            out.ids.push(id);
                        }
                    }
                }
                Err(e) => fail(&mut out, e),
            },
            Kind::Other => fail(&mut out, AppError::NotIpa),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::testutil::make_ipa;

    #[test]
    fn kind_by_extension() {
        assert_eq!(kind(Path::new("/x/Банк.IPA")), Kind::Ipa);
        assert_eq!(kind(Path::new("AppsList.txt")), Kind::Text);
        assert_eq!(kind(Path::new("Сбер.webloc")), Kind::Text);
        assert_eq!(kind(Path::new("link.URL")), Kind::Text);
        assert_eq!(kind(Path::new("photo.png")), Kind::Other);
        assert_eq!(kind(Path::new("folder")), Kind::Other);
        assert_eq!(kind(Path::new("x.ipa.zip")), Kind::Other);
    }

    #[test]
    fn handle_mixes_copies_ids_and_errors() {
        let t = tempfile::tempdir().unwrap();
        let lib = Library::new(t.path().join("lib"));
        let ipa = t.path().join("Банк.ipa");
        make_ipa(&ipa, "com.example.bank", "1.0");
        let list = t.path().join("AppsList.txt");
        std::fs::write(&list, "Сбер: 492224193\nhttps://apps.apple.com/ru/app/x/id1631810724\n").unwrap();
        let webloc = t.path().join("ВК.webloc");
        std::fs::write(&webloc, "<plist><dict><key>URL</key><string>https://apps.apple.com/ru/app/vk/id564177498</string></dict></plist>").unwrap();
        let winurl = t.path().join("ВК.url");
        std::fs::write(&winurl, "[InternetShortcut]\r\nURL=https://apps.apple.com/ru/app/vk/id564177498\r\n").unwrap();
        let junk = t.path().join("junk.ipa");
        std::fs::write(&junk, b"not a zip").unwrap();
        let png = t.path().join("a.png");
        let png2 = t.path().join("b.png");
        let empty = t.path().join("notes.txt");
        std::fs::write(&empty, "ничего полезного 492224193").unwrap();
        let folder = t.path().join("папка.txt");
        std::fs::create_dir(&folder).unwrap();
        let out = handle(&lib, &[ipa, list, webloc, winurl, junk, png, png2, empty, t.path().join("missing.txt"), folder]);
        assert_eq!(out.copies.len(), 1);
        assert_eq!(out.copies[0].bundle_id, "com.example.bank");
        assert_eq!(out.ids, vec![492224193, 1631810724, 564177498]);
        assert_eq!(out.errors, vec![AppError::NotIpa, AppError::BadLink], "нет файла и папка — NotIpa, не Unknown");
    }

    #[test]
    fn text_file_must_be_a_regular_file() {
        let t = tempfile::tempdir().unwrap();
        let folder = t.path().join("x.txt");
        std::fs::create_dir(&folder).unwrap();
        assert_eq!(text_ids(&folder).unwrap_err(), AppError::NotIpa);
        assert_eq!(text_ids(&t.path().join("нет.txt")).unwrap_err(), AppError::NotIpa);
    }

    #[test]
    fn binary_webloc_is_read_as_plist() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("Сбер.webloc");
        let mut d = plist::Dictionary::new();
        d.insert("URL".into(), "https://apps.apple.com/ru/app/sber/id492224193".into());
        plist::Value::Dictionary(d).to_file_binary(&p).unwrap();
        assert!(std::fs::read(&p).unwrap().starts_with(b"bplist00"));
        assert_eq!(text_ids(&p).unwrap(), vec![492224193]);
    }

    #[test]
    fn utf16_text_with_bom_is_decoded() {
        let t = tempfile::tempdir().unwrap();
        let text = "[InternetShortcut]\r\nURL=https://apps.apple.com/ru/app/vk/id564177498\r\n";
        let le = t.path().join("ВК.url");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(&le, bytes).unwrap();
        assert_eq!(text_ids(&le).unwrap(), vec![564177498]);
        let be = t.path().join("Список.txt");
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend("Сбер: 492224193\nВК: 564177498".encode_utf16().flat_map(u16::to_be_bytes));
        std::fs::write(&be, bytes).unwrap();
        assert_eq!(text_ids(&be).unwrap(), vec![492224193, 564177498]);
    }

    #[test]
    fn text_file_reads_only_first_megabyte() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("big.txt");
        let mut s = "A: 1111111\n".to_string();
        s.push_str(&" ".repeat(MAX_TEXT_BYTES as usize));
        s.push_str("\nB: 2222222\n");
        std::fs::write(&p, s).unwrap();
        assert_eq!(text_ids(&p).unwrap(), vec![1111111]);
    }

    #[test]
    fn ids_across_files_are_capped() {
        let t = tempfile::tempdir().unwrap();
        let lib = Library::new(t.path().join("lib"));
        let mut paths = vec![];
        for f in 0..3 {
            let p = t.path().join(format!("{f}.txt"));
            let text: String = (0..80).map(|i| format!("A: {}\n", 1_000_000 + f * 1000 + i)).collect();
            std::fs::write(&p, text).unwrap();
            paths.push(p);
        }
        assert_eq!(handle(&lib, &paths).ids.len(), MAX_TEXT_IDS);
    }
}
