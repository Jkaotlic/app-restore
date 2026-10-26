
//! «Мои приложения»: папка с сохранёнными ipa, у каждого Apple ID своя подпапка
//! `<база>/<apple_id>/` (`account_dir_name`). Недокачанное лежит в `<папка аккаунта>/.partial/<track_id>/`.
//! Без входа копий нет: `list` пуст, скачивать некуда, «Открыть папку» открывает базу.
use crate::catalog::SavedIpa;
use crate::errors::AppError;
use plist::Value;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

pub const PARTIAL: &str = ".partial";

/// Метаданные копии и (время изменения, размер), при которых они прочитаны.
type MetaEntry = ((std::time::SystemTime, u64), Option<CopyMeta>);

pub struct Library {
    /// «Мои приложения».
    base: PathBuf,
    account: std::sync::Mutex<Acct>,
    /// Есть ли покупатель в ipa (путь, время изменения): `iTunesMetadata.plist` читается один раз.
    has_buyer: std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::SystemTime, bool)>>,
    /// UIDeviceFamily по файлу (путь, время изменения): Info.plist читается один раз.
    families: std::sync::Mutex<std::collections::HashMap<PathBuf, (std::time::SystemTime, Vec<u8>)>>,
    /// Метаданные копий для «Все копии» (путь, время изменения, размер).
    metas: std::sync::Mutex<std::collections::HashMap<PathBuf, MetaEntry>>,
    /// Миграция (при запуске и при входе), уборка и добавление файла идут строго по одной.
    moving: std::sync::Mutex<()>,
}

/// Чья папка сейчас.
#[derive(Debug, Clone, PartialEq, Default)]
enum Acct {
    #[default]
    None,
    /// Имя папки (`account_dir_name`).
    Dir(String),
    /// Вошли, но из Apple ID не вышло имя папки.
    Bad,
}

fn unknown(what: &str, e: impl std::fmt::Display) -> AppError {
    AppError::Unknown { detail: format!("{what}: {e}") }
}

/// Имя папки аккаунта: Apple ID без пробелов по краям, в нижнем регистре, всё вне
/// `[A-Za-z0-9@._+-]` заменено на `_`. Пустое и с точкой в начале или в конце (`.`, `..`,
/// `.partial`, `x.`) — ошибка `BadAccountFolder`.
pub fn account_dir_name(email: &str) -> Result<String, AppError> {
    let n: String = email
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-') { c } else { '_' })
        .collect();
    if n.is_empty() || n.starts_with('.') || n.ends_with('.') {
        return Err(AppError::BadAccountFolder);
    }
    Ok(n)
}

/// Apple ID покупателя из `iTunesMetadata.plist` в корне ipa (его кладёт ipatool). Ключи те же,
/// что у метаданных на телефоне (`device::itunes_meta`).
pub fn ipa_purchaser(ipa: &Path) -> Option<String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(ipa).ok()?).ok()?;
    let mut buf = Vec::new();
    zip.by_name("iTunesMetadata.plist").ok()?.take(MAX_PLIST + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_PLIST {
        return None;
    }
    let v = Value::from_reader(Cursor::new(buf)).ok()?;
    crate::device::itunes_meta(v.as_dictionary()?).purchaser
}

/// Имена для `name` в `dir` по очереди: `name`, `<stem> (2).ipa`, `(3)`…
fn candidates<'a>(dir: &'a Path, name: &'a str) -> impl Iterator<Item = PathBuf> + 'a {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, format!(".{e}")),
        None => (name, String::new()),
    };
    std::iter::once(dir.join(name)).chain((2..1000).map(move |i| dir.join(format!("{stem} ({i}){ext}"))))
}

/// Файловые операции миграции; в тестах подменяются сбоями.
pub(crate) trait FileOps {
    /// Перенос, который никогда не затирает: занято — `AlreadyExists`, оба файла целы.
    fn rename_noreplace(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    /// Копия в новый файл (`to` не должен существовать) с fsync.
    fn copy_synced(&self, from: &Path, to: &Path) -> std::io::Result<()>;
    fn remove(&self, p: &Path) -> std::io::Result<()>;
    /// Перенос с заменой (атомарно; Windows — `MoveFileExW` с `REPLACE_EXISTING`, так делает std).
    /// Только для своей же свежей копии в `commit`.
    fn rename_replace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::rename(from, to)
    }
}

pub(crate) struct RealOps;

impl FileOps for RealOps {
    /// Жёсткая ссылка и удаление исходника: ссылка не создаётся поверх существующего файла,
    /// поэтому параллельно появившийся файл не затрётся. Нет жёстких ссылок (FAT, exFAT) —
    /// системный перенос без замены.
    fn rename_noreplace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        match std::fs::hard_link(from, to) {
            Ok(()) => {
                if let Err(e) = std::fs::remove_file(from) {
                    let _ = std::fs::remove_file(to);
                    return Err(e);
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
            Err(_) => os_rename_noreplace(from, to),
        }
    }

    fn copy_synced(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        let mut out = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
        std::io::copy(&mut std::fs::File::open(from)?, &mut out)?;
        out.sync_all()
    }

    fn remove(&self, p: &Path) -> std::io::Result<()> {
        std::fs::remove_file(p)
    }
}

/// macOS: `renamex_np(RENAME_EXCL)`.
#[cfg(target_os = "macos")]
fn os_rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(std::io::Error::other);
    let (f, t) = (c(from)?, c(to)?);
    // SAFETY: обе строки живы и оканчиваются нулём на время вызова.
    if unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Windows: `MoveFileExW` без `MOVEFILE_REPLACE_EXISTING` (и без копирования между томами).
#[cfg(windows)]
fn os_rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let w = |p: &Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (f, t) = (w(from), w(to));
    // SAFETY: обе строки живы и оканчиваются нулём на время вызова.
    if unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(f.as_ptr(), t.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_rename_noreplace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "no-replace rename"))
}

/// Одинаковы ли файлы: размер, затем содержимое побайтно.
fn same_content(a: &Path, b: &Path) -> std::io::Result<bool> {
    if std::fs::metadata(a)?.len() != std::fs::metadata(b)?.len() {
        return Ok(false);
    }
    let (mut x, mut y) = (std::io::BufReader::new(std::fs::File::open(a)?), std::io::BufReader::new(std::fs::File::open(b)?));
    let (mut bx, mut by) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let n = x.read(&mut bx)?;
        if n == 0 {
            return Ok(true);
        }
        y.read_exact(&mut by[..n])?;
        if bx[..n] != by[..n] {
            return Ok(false);
        }
    }
}

/// Точно такая же ipa в `dir` (под любым именем), если есть.
fn identical_in(dir: &Path, src: &Path) -> Option<PathBuf> {
    let len = std::fs::metadata(src).ok()?.len();
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(".ipa") && !n.starts_with('.')))
        .filter(|e| e.metadata().is_ok_and(|m| m.len() == len))
        .map(|e| e.path())
        .find(|p| p != src && same_content(src, p).unwrap_or(false))
}

/// Временное имя для копии `name` в той же папке: `.{name}.part`; длиннее 255 байт — `.{hash}.part`.
/// Остатки после сбоя убирает `sweep_partials`.
fn part_name(name: &str) -> String {
    let p = format!(".{name}.part");
    if p.len() <= 255 {
        return p;
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    format!(".{:016x}.part", h.finish())
}

/// Остаток прерванной копии (`part_name`)?
fn is_part(name: &str) -> bool {
    name.len() >= ".x.part".len() && name.starts_with('.') && name.ends_with(".part")
}

/// Копия `src` в `dir` под именем `name` (занято — ` (2)`…): во временный `.part`, fsync, затем
/// перенос без замены. Под настоящим именем никогда нет недописанного файла; при ошибке `.part`
/// убран. Исходник не трогается.
fn copy_in(ops: &dyn FileOps, src: &Path, dir: &Path, name: &str) -> std::io::Result<PathBuf> {
    let part = dir.join(part_name(name));
    let _ = ops.remove(&part);
    if let Err(e) = ops.copy_synced(src, &part) {
        let _ = ops.remove(&part);
        return Err(e);
    }
    for dest in candidates(dir, name) {
        match ops.rename_noreplace(&part, &dest) {
            Ok(()) => return Ok(dest),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                let _ = ops.remove(&part);
                return Err(e);
            }
        }
    }
    let _ = ops.remove(&part);
    Err(std::io::Error::other("no free name"))
}

/// Кладёт `src` в `dir` под именем `name` (занято — ` (2)`…), ничего не затирая. Итог: где копия
/// теперь и убран ли исходник (`false` — такая же копия уже была, а исходник убрать не вышло).
/// При ошибке исходник на месте и лишних файлов в `dir` нет.
fn place(ops: &dyn FileOps, src: &Path, dir: &Path, name: &str) -> std::io::Result<(PathBuf, bool)> {
    if let Some(same) = identical_in(dir, src) {
        return match ops.remove(src) {
            Ok(()) => Ok((same, true)),
            Err(e) => {
                eprintln!("[app-restore] place {name}: same copy exists, source not removed: {e}");
                Ok((same, false))
            }
        };
    }
    for dest in candidates(dir, name) {
        match ops.rename_noreplace(src, &dest) {
            Ok(()) => return Ok((dest, true)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => break, // например, другой том: копируем
        }
    }
    let dest = copy_in(ops, src, dir, name)?;
    // Исходник не убрать — убираем копию: следующий запуск начнёт чисто, без дублей.
    if let Err(e) = ops.remove(src) {
        let _ = ops.remove(&dest);
        return Err(e);
    }
    Ok((dest, true))
}

/// Своё свежее скачивание на каноническое имя `name` с заменой: во временный `.part` (перенос, а
/// между томами — копия), fsync, затем атомарный перенос поверх. Под `name` всегда целый файл.
/// Итог: путь и заменили ли каноническое (`false` — оно занято, например открыто в Windows:
/// новое легло рядом как ` (n)`). Скачанное не удаляется никогда: не легло — возвращается
/// на место, а не вышло и это — остаётся как `<имя>.recovered.ipa`.
fn replace_own(ops: &dyn FileOps, src: &Path, dir: &Path, name: &str) -> std::io::Result<(PathBuf, bool)> {
    let part = dir.join(part_name(name));
    let dest = dir.join(name);
    let _ = ops.remove(&part);
    let moved = ops.rename_noreplace(src, &part).is_ok();
    if !moved
        && let Err(e) = ops.copy_synced(src, &part)
    {
        let _ = ops.remove(&part);
        return Err(e);
    }
    let finish = |p: PathBuf, replaced: bool| {
        if !moved {
            let _ = ops.remove(src);
        }
        Ok((p, replaced))
    };
    let synced = std::fs::File::open(&part).and_then(|f| f.sync_all());
    let err = match synced.and_then(|_| ops.rename_replace(&part, &dest)) {
        Ok(()) => return finish(dest, true),
        Err(e) => e,
    };
    // каноническое не заменить — рядом, без замены
    for cand in candidates(dir, name).skip(1) {
        match ops.rename_noreplace(&part, &cand) {
            Ok(()) => return finish(cand, false),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    if !moved {
        // исходник цел — копию можно убрать
        let _ = ops.remove(&part);
        return Err(err);
    }
    if ops.rename_noreplace(&part, src).is_ok() {
        return Err(err);
    }
    let stem = name.strip_suffix(".ipa").unwrap_or(name);
    for cand in candidates(dir, &format!("{stem}.recovered.ipa")) {
        match ops.rename_noreplace(&part, &cand) {
            Ok(()) => {
                eprintln!("[app-restore] commit {name}: kept the download as {}", cand.display());
                return Err(err);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => break,
        }
    }
    // последнее: `.part` остаётся как есть
    Err(err)
}

/// После `replace_own`: убрать дубли той же копии `<name без суффикса> (N).ipa` — тот же trackId
/// и та же версия. Другие версии и другие приложения не трогаются.
fn drop_numbered_siblings(dir: &Path, name: &str, track_id: i64, version: &str) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let Ok(f) = e.file_name().into_string() else { continue };
        if f == name || !e.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let numbered = f.strip_suffix(".ipa").and_then(|s| s.rsplit_once(" (")).is_some_and(|(_, n)| n.ends_with(')'));
        let same = parse_saved_name(&f).is_some_and(|(t, _, v)| t == Some(track_id) && v == version);
        if numbered && same {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Для имени файла: оставить `[A-Za-z0-9._-]`, остальное в `_`, ведущие точки убрать.
fn sanitize(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    t.trim_start_matches('.').replace("..", "_")
}

const MAX_PLIST: u64 = 1024 * 1024;

fn digits(s: &str) -> Option<i64> {
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

struct NameParts {
    track_id: Option<i64>,
    bundle: Option<String>,
    version: String,
}

/// Имя от ipatool: `<trackId>_<version>.ipa` (`download -i`) или старое `<bundleID>_<trackId>_<version>.ipa`.
fn parse_ipatool_name(file: &str) -> Option<NameParts> {
    let stem = file.strip_suffix(".ipa")?;
    if let Some((first, version)) = stem.split_once('_') {
        if let Some(t) = digits(first) {
            return Some(NameParts { track_id: Some(t), bundle: None, version: version.to_string() });
        }
    }
    let mut it = stem.rsplitn(3, '_');
    let version = it.next()?;
    let track = digits(it.next()?)?;
    let bundle = it.next()?;
    Some(NameParts { track_id: Some(track), bundle: Some(bundle.to_string()), version: version.to_string() })
}

/// Имя в библиотеке: `<trackId>_<bundle>_<version>.ipa`; старое `<bundle>_<version>.ipa` даёт `track_id: None`.
/// Суффикс ` (N)` от миграции (`1_a_1.0 (2).ipa`) в версию не входит.
fn parse_saved_name(file: &str) -> Option<(Option<i64>, String, String)> {
    let stem = file.strip_suffix(".ipa")?;
    let stem = match stem.rsplit_once(" (") {
        Some((s, n)) if n.strip_suffix(')').is_some_and(|d| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit())) => s,
        _ => stem,
    };
    if let Some((first, rest)) = stem.split_once('_') {
        if let (Some(t), Some((b, v))) = (digits(first), rest.rsplit_once('_')) {
            return Some((Some(t), b.to_string(), v.to_string()));
        }
    }
    let (b, v) = stem.rsplit_once('_')?;
    Some((None, b.to_string(), v.to_string()))
}

/// Файл из zip целиком, если он не больше `MAX_PLIST`.
fn read_small<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    zip.by_name(name).map_err(|e| e.to_string())?.take(MAX_PLIST + 1).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_PLIST {
        return Err(format!("{name} слишком большой"));
    }
    Ok(buf)
}

/// Словарь `Payload/*.app/Info.plist` из открытого архива.
fn info_from_zip<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<plist::Dictionary, String> {
    let name = zip
        .file_names()
        .find(|n| {
            let parts: Vec<_> = n.split('/').collect();
            parts.len() == 3 && parts[0] == "Payload" && parts[1].ends_with(".app") && parts[2] == "Info.plist"
        })
        .map(String::from)
        .ok_or("в ipa нет Info.plist")?;
    let buf = read_small(zip, &name)?;
    let v = Value::from_reader(Cursor::new(buf)).map_err(|e| e.to_string())?;
    v.into_dictionary().ok_or_else(|| "Info.plist не словарь".to_string())
}

/// Словарь `Payload/*.app/Info.plist` внутри ipa.
fn info_plist(ipa: &Path) -> Result<plist::Dictionary, String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(ipa).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    info_from_zip(&mut zip)
}

/// Приложение iPhone: zip, в котором есть что-то внутри `Payload/<имя>.app/`.
fn is_ipa_zip(path: &Path) -> bool {
    let Ok(f) = std::fs::File::open(path) else { return false };
    let Ok(zip) = zip::ZipArchive::new(f) else { return false };
    zip.file_names().any(|n| {
        let mut it = n.split('/');
        it.next() == Some("Payload")
            && it.next().is_some_and(|a| a.len() > ".app".len() && a.ends_with(".app"))
            && it.next().is_some_and(|x| !x.is_empty())
    })
}

/// Что известно о копии из её Info.plist и iTunesMetadata.plist.
#[derive(Debug, Clone, PartialEq)]
struct CopyMeta {
    bundle_id: Option<String>,
    version: Option<String>,
    name: Option<String>,
    family: Vec<u8>,
    purchaser: Option<String>,
    item_id: Option<i64>,
}

/// Метаданные копии; `None` — не приложение (нет Info.plist).
fn read_copy_meta(ipa: &Path) -> Option<CopyMeta> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(ipa).ok()?).ok()?;
    let d = info_from_zip(&mut zip).ok()?;
    let text = |d: &plist::Dictionary, k: &str| d.get(k).and_then(Value::as_string).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let itunes = read_small(&mut zip, "iTunesMetadata.plist")
        .ok()
        .and_then(|b| Value::from_reader(Cursor::new(b)).ok())
        .and_then(Value::into_dictionary);
    let meta = itunes.as_ref().map(crate::device::itunes_meta).unwrap_or_default();
    let name = itunes.as_ref().and_then(|m| text(m, "itemName")).or_else(|| text(&d, "CFBundleDisplayName")).or_else(|| text(&d, "CFBundleName"));
    Some(CopyMeta {
        bundle_id: text(&d, "CFBundleIdentifier"),
        version: text(&d, "CFBundleShortVersionString"),
        name,
        family: family_of(&d),
        purchaser: meta.purchaser,
        item_id: meta.item_id.filter(|i| *i > 0),
    })
}

/// Копия для «Все копии»: любая ipa из «Мои приложения» и папок аккаунтов.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyInfo {
    pub path: PathBuf,
    pub track_id: Option<i64>,
    pub bundle_id: String,
    pub name: String,
    pub version: String,
    pub device_family: Vec<u8>,
    /// Apple ID покупателя: личные данные, наружу только маской.
    pub purchaser: Option<String>,
    /// Имя папки аккаунта; `None` — лежит прямо в «Мои приложения».
    pub account_folder: Option<String>,
}

/// (CFBundleIdentifier, CFBundleShortVersionString) из Info.plist.
fn read_info_plist(ipa: &Path) -> Result<(Option<String>, Option<String>), String> {
    let d = info_plist(ipa)?;
    let get = |k: &str| d.get(k).and_then(Value::as_string).map(String::from);
    Ok((get("CFBundleIdentifier"), get("CFBundleShortVersionString")))
}

/// `UIDeviceFamily` из Info.plist внутри ipa: 1 — iPhone, 2 — iPad. Пусто, если не прочитать.
pub fn read_device_family(ipa: &Path) -> Vec<u8> {
    info_plist(ipa).map(|d| family_of(&d)).unwrap_or_default()
}

fn family_of(d: &plist::Dictionary) -> Vec<u8> {
    let num = |v: &Value| v.as_unsigned_integer().or_else(|| v.as_string().and_then(|s| s.trim().parse().ok()));
    match d.get("UIDeviceFamily") {
        Some(Value::Array(a)) => a.iter().filter_map(num).filter_map(|n| u8::try_from(n).ok()).collect(),
        Some(v) => num(v).and_then(|n| u8::try_from(n).ok()).into_iter().collect(),
        None => vec![],
    }
}

/// Имя для внешней копии: как у скачанных (`<trackId>_<bundle>_<version>.ipa`, без trackId —
/// `<bundle>_<version>.ipa`), чтобы её видел «Выбор»; без Info.plist — исходное имя.
fn import_name(meta: Option<&CopyMeta>, original: &str) -> String {
    let m = meta.and_then(|m| Some((m.item_id, sanitize(m.bundle_id.as_deref()?), sanitize(m.version.as_deref()?))));
    match m {
        Some((Some(t), b, v)) if !b.is_empty() && !v.is_empty() => format!("{t}_{b}_{v}.ipa"),
        Some((None, b, v)) if !b.is_empty() && !v.is_empty() => format!("{b}_{v}.ipa"),
        _ => {
            let stem = sanitize(original.strip_suffix(".ipa").unwrap_or(original));
            if stem.is_empty() { "copy.ipa".into() } else { format!("{stem}.ipa") }
        }
    }
}

impl Library {
    pub fn new(base: PathBuf) -> Self {
        Self { base, account: Default::default(), has_buyer: Default::default(), families: Default::default(), metas: Default::default(), moving: Default::default() }
    }

    /// Вход в Apple ID (`Some`) или выход (`None`). Плохое имя — ошибка, копии выключены, а
    /// скачивание и «Открыть папку» отвечают `BadAccountFolder`.
    pub fn set_account(&self, email: Option<&str>) -> Result<(), AppError> {
        let (acct, result) = match email.map(account_dir_name) {
            None => (Acct::None, Ok(())),
            Some(Ok(n)) => (Acct::Dir(n), Ok(())),
            Some(Err(e)) => (Acct::Bad, Err(e)),
        };
        *self.account.lock().unwrap() = acct;
        result
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Папка текущего аккаунта; `None` — не вошли.
    pub fn root(&self) -> Option<PathBuf> {
        match &*self.account.lock().unwrap() {
            Acct::Dir(a) => Some(self.base.join(a)),
            _ => None,
        }
    }

    /// Папка аккаунта или ошибка: не вошли / не вышло имя папки.
    fn account_root(&self) -> Result<PathBuf, AppError> {
        match &*self.account.lock().unwrap() {
            Acct::Dir(a) => Ok(self.base.join(a)),
            Acct::Bad => Err(AppError::BadAccountFolder),
            Acct::None => Err(AppError::NotLoggedIn),
        }
    }

    /// Для «Открыть папку с копиями»: папка аккаунта, без входа — «Мои приложения». Создаётся.
    pub fn ensure_root(&self) -> Result<PathBuf, AppError> {
        let dir = match self.account_root() {
            Err(AppError::NotLoggedIn) => self.base.clone(),
            other => other?,
        };
        std::fs::create_dir_all(&dir).map_err(|e| unknown("создание папки", e))?;
        Ok(dir)
    }

    /// Разложить ipa, лежащие прямо в «Мои приложения» (до папок по аккаунтам), по папкам их
    /// покупателей. Без покупателя — остаётся на месте. Повтор безопасен. Возвращает число перенесённых.
    pub fn migrate(&self) -> usize {
        self.migrate_with(&RealOps)
    }

    pub(crate) fn migrate_with(&self, ops: &dyn FileOps) -> usize {
        let _one = self.moving.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(rd) = std::fs::read_dir(&self.base) else { return 0 };
        let mut moved = 0;
        for e in rd.filter_map(Result::ok) {
            let path = e.path();
            let Some(name) = e.file_name().into_string().ok().filter(|n| n.ends_with(".ipa")) else { continue };
            if !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            // Адрес покупателя в лог не пишем: только имя файла.
            let Some(dir) = ipa_purchaser(&path).and_then(|p| account_dir_name(&p).ok()) else {
                eprintln!("[app-restore] migrate: no purchaser in {name}, left in place");
                continue;
            };
            let dir = self.base.join(dir);
            match std::fs::create_dir_all(&dir).and_then(|_| place(ops, &path, &dir, &name)) {
                Ok((_, true)) => moved += 1,
                Ok((_, false)) => {}
                Err(err) => eprintln!("[app-restore] migrate {name}: {err}"),
            }
        }
        moved
    }

    /// UIDeviceFamily с кэшем по времени изменения файла.
    fn family(&self, path: &Path) -> Vec<u8> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if let Some((t, f)) = self.families.lock().unwrap().get(path) {
            if *t == mtime {
                return f.clone();
            }
        }
        let f = read_device_family(path);
        self.families.lock().unwrap().insert(path.to_path_buf(), (mtime, f.clone()));
        f
    }

    /// `~/Documents/Мои приложения` (на Mac и Windows через `dirs::document_dir`).
    pub fn default_root() -> Option<PathBuf> {
        dirs::document_dir().map(|d| d.join("Мои приложения"))
    }

    /// Копии текущего аккаунта; без входа — пусто.
    pub fn list(&self) -> Vec<SavedIpa> {
        self.root().map(|r| self.list_dir(&r, |_| true)).unwrap_or_default()
    }

    /// ipa прямо в «Мои приложения», у которых не узнать покупателя (миграция их оставила).
    /// Их можно поставить, но чей это Apple ID — неизвестно. Без входа — пусто.
    pub fn list_unknown(&self) -> Vec<SavedIpa> {
        if self.root().is_none() {
            return vec![];
        }
        self.list_dir(&self.base, |p| !self.has_buyer(p))
    }

    /// Покупатель в ipa есть? С кэшем по времени изменения; адрес не хранится.
    fn has_buyer(&self, path: &Path) -> bool {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if let Some((t, b)) = self.has_buyer.lock().unwrap().get(path) {
            if *t == mtime {
                return *b;
            }
        }
        let b = ipa_purchaser(path).is_some();
        self.has_buyer.lock().unwrap().insert(path.to_path_buf(), (mtime, b));
        b
    }

    fn list_dir(&self, dir: &Path, keep: impl Fn(&Path) -> bool) -> Vec<SavedIpa> {
        let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
        let mut out: Vec<SavedIpa> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let (track_id, bundle_id, version) = parse_saved_name(&name)?;
                let path = e.path();
                if !keep(&path) {
                    return None;
                }
                Some(SavedIpa { track_id, bundle_id, version, device_family: self.family(&path), path })
            })
            .collect();
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Метаданные копии с кэшем по времени изменения и размеру.
    fn meta(&self, path: &Path) -> Option<CopyMeta> {
        let md = std::fs::metadata(path).ok()?;
        let key = (md.modified().unwrap_or(std::time::UNIX_EPOCH), md.len());
        if let Some((k, m)) = self.metas.lock().unwrap().get(path)
            && *k == key
        {
            return m.clone();
        }
        let m = read_copy_meta(path);
        self.metas.lock().unwrap().insert(path.to_path_buf(), (key, m.clone()));
        m
    }

    fn copy_info(&self, path: &Path, folder: Option<&str>) -> Option<CopyInfo> {
        let meta = self.meta(path)?;
        let parsed = path.file_name().and_then(|f| f.to_str()).and_then(parse_saved_name);
        let bundle_id = meta.bundle_id.clone().or_else(|| parsed.as_ref().map(|p| p.1.clone()))?;
        Some(CopyInfo {
            path: path.to_path_buf(),
            track_id: meta.item_id.or_else(|| parsed.as_ref().and_then(|p| p.0)),
            name: meta.name.clone().unwrap_or_else(|| bundle_id.clone()),
            version: meta.version.clone().or_else(|| parsed.map(|p| p.2)).unwrap_or_default(),
            bundle_id,
            device_family: meta.family,
            purchaser: meta.purchaser,
            account_folder: folder.map(String::from),
        })
    }

    /// ipa прямо в `dir`: обычные файлы, не скрытые и не ссылки.
    fn copies_in(&self, dir: &Path, folder: Option<&str>, out: &mut Vec<CopyInfo>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.filter_map(Result::ok) {
            let Ok(name) = e.file_name().into_string() else { continue };
            if name.starts_with('.') || !name.ends_with(".ipa") || !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            out.extend(self.copy_info(&e.path(), folder));
        }
    }

    /// «Все копии»: ipa в «Мои приложения» и в папке каждого аккаунта, без входа тоже. Недокачанное
    /// (`.partial`), скрытые файлы, ссылки и не-приложения пропускаются. По пути.
    pub fn all_copies(&self) -> Vec<CopyInfo> {
        let mut out = vec![];
        self.copies_in(&self.base, None, &mut out);
        if let Ok(rd) = std::fs::read_dir(&self.base) {
            for e in rd.filter_map(Result::ok) {
                let Ok(name) = e.file_name().into_string() else { continue };
                if !name.starts_with('.') && e.file_type().is_ok_and(|t| t.is_dir()) {
                    self.copies_in(&e.path(), Some(&name), &mut out);
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Сколько копий в папке каждого аккаунта (по именам файлов, без чтения архивов).
    pub fn copy_counts(&self) -> Vec<(String, usize)> {
        let count = |dir: &Path| {
            std::fs::read_dir(dir).map_or(0, |rd| {
                rd.filter_map(Result::ok)
                    .filter(|e| e.file_name().to_str().is_some_and(|n| !n.starts_with('.') && n.ends_with(".ipa")))
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                    .count()
            })
        };
        let Ok(rd) = std::fs::read_dir(&self.base) else { return vec![] };
        let mut out: Vec<(String, usize)> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| e.file_name().into_string().ok().filter(|n| !n.starts_with('.')).map(|n| (n, count(&e.path()))))
            .filter(|(_, n)| *n > 0)
            .collect();
        out.sort();
        out
    }

    /// Путь от интерфейса — копия внутри «Мои приложения» (после разворота ссылок): сама база или
    /// папка аккаунта, не скрытое и не `.partial`, обычный файл `.ipa`. Иначе ошибка.
    /// Возвращает `база/…` (как в `all_copies`) и имя папки аккаунта.
    fn inside(&self, path: &Path) -> Option<(PathBuf, Option<String>)> {
        use std::path::Component;
        let base = std::fs::canonicalize(&self.base).ok()?;
        let real = std::fs::canonicalize(path).ok()?;
        let rel = real.strip_prefix(&base).ok()?;
        let parts: Vec<&str> = rel
            .components()
            .map(|c| match c {
                Component::Normal(n) => n.to_str().filter(|s| !s.starts_with('.')),
                _ => None,
            })
            .collect::<Option<_>>()?;
        let ok = matches!(parts.len(), 1 | 2) && parts.last()?.ends_with(".ipa") && real.is_file();
        ok.then(|| (self.base.join(rel), (parts.len() == 2).then(|| parts[0].to_string())))
    }

    /// Для `install_copy`: настоящий путь копии, только если она лежит в «Мои приложения».
    pub fn resolve_copy(&self, path: &Path) -> Result<PathBuf, AppError> {
        let (p, _) = self.inside(path).ok_or_else(|| AppError::Unknown { detail: "copy outside library".into() })?;
        std::fs::canonicalize(p).map_err(|e| unknown("copy", e))
    }

    /// Копия по пути из «Мои приложения» (для проверки покупателя и имени задания).
    pub fn copy_at(&self, path: &Path) -> Option<CopyInfo> {
        let (p, folder) = self.inside(path)?;
        self.copy_info(&p, folder.as_deref())
    }

    /// «Выбрать файл .ipa…»: файл проверяется (zip с `Payload/*.app`) и копируется в папку своего
    /// покупателя, а без покупателя — в «Мои приложения». Ничего не затирает (занято — ` (2)`),
    /// такая же копия не дублируется, исходник не трогается. Уже лежит в «Мои приложения» — как есть.
    pub fn import_ipa(&self, src: &Path) -> Result<CopyInfo, AppError> {
        if !std::fs::metadata(src).is_ok_and(|m| m.is_file()) || !is_ipa_zip(src) {
            return Err(AppError::NotIpa);
        }
        if let Some((p, folder)) = self.inside(src) {
            return self.copy_info(&p, folder.as_deref()).ok_or(AppError::NotIpa);
        }
        let meta = read_copy_meta(src);
        let folder = meta.as_ref().and_then(|m| m.purchaser.as_deref()).and_then(|p| account_dir_name(p).ok());
        let dir = folder.as_ref().map_or_else(|| self.base.clone(), |f| self.base.join(f));
        std::fs::create_dir_all(&dir).map_err(|e| unknown("создание папки", e))?;
        let original = src.file_name().and_then(|f| f.to_str()).unwrap_or("copy.ipa");
        let name = import_name(meta.as_ref(), original);
        let dest = {
            let _one = self.moving.lock().unwrap_or_else(|e| e.into_inner());
            match identical_in(&dir, src) {
                Some(same) => same,
                None => copy_in(&RealOps, src, &dir, &name).map_err(|e| unknown("копия ipa", e))?,
            }
        };
        self.copy_info(&dest, folder.as_deref()).ok_or(AppError::NotIpa)
    }

    /// Числовой trackId безопасен как имя каталога. Без входа скачивать некуда.
    pub fn partial_dir(&self, track_id: i64) -> Result<PathBuf, AppError> {
        if track_id <= 0 {
            return Err(AppError::Unknown { detail: "invalid track id".into() });
        }
        Ok(self.account_root()?.join(PARTIAL).join(track_id.to_string()))
    }

    /// Переносит скачанный ipa в `<папка аккаунта>/<trackId>_<bundle_id>_<version>.ipa`. Папка —
    /// та, в чьей `.partial` шло скачивание: смена входа посреди скачивания не путает аккаунты. Версия и bundle id берутся из
    /// `Info.plist`; если прочитать не вышло, из имени файла ipatool (bundle там есть только в старом формате).
    /// trackId берётся из имени файла ipatool, иначе из имени каталога `.partial/<trackId>`.
    pub fn commit(&self, partial: &Path, ipa: &Path) -> Result<PathBuf, AppError> {
        self.commit_with(&RealOps, partial, ipa)
    }

    pub(crate) fn commit_with(&self, ops: &dyn FileOps, partial: &Path, ipa: &Path) -> Result<PathBuf, AppError> {
        let escapes = |p: &Path| p.components().any(|c| matches!(c, std::path::Component::ParentDir));
        let root = partial.parent().filter(|p| p.file_name().is_some_and(|n| n == PARTIAL)).and_then(Path::parent);
        let root = match root {
            Some(r) if !escapes(ipa) && !escapes(partial) && ipa.starts_with(partial) && r.parent() == Some(self.base.as_path()) => r,
            _ => return Err(AppError::Unknown { detail: "unexpected download path".into() }),
        };
        let file = ipa.file_name().and_then(|f| f.to_str()).unwrap_or_default();
        let from_name = parse_ipatool_name(file);
        let track_id = from_name
            .as_ref()
            .and_then(|n| n.track_id)
            .or_else(|| partial.file_name().and_then(|f| f.to_str()).and_then(digits));
        let (plist_bundle, plist_version) = read_info_plist(ipa).unwrap_or((None, None));
        let bundle = plist_bundle.or_else(|| from_name.as_ref().and_then(|n| n.bundle.clone()));
        let version = plist_version.or_else(|| from_name.map(|n| n.version));
        let (Some(track_id), Some(bundle), Some(version)) = (track_id, bundle, version) else {
            return Err(AppError::Unknown { detail: format!("не удалось определить версию {file}") });
        };
        std::fs::create_dir_all(root).map_err(|e| unknown("создание папки", e))?;
        let (bundle, version) = (sanitize(&bundle), sanitize(&version));
        if bundle.is_empty() || version.is_empty() {
            return Err(AppError::Unknown { detail: format!("не удалось определить версию {file}") });
        }
        let name = format!("{track_id}_{bundle}_{version}.ipa");
        let _one = self.moving.lock().unwrap_or_else(|e| e.into_inner());
        let (dest, replaced) = replace_own(ops, ipa, root, &name).map_err(|e| unknown("перенос ipa", e))?;
        if replaced {
            drop_numbered_siblings(root, &name, track_id, &version);
        }
        Ok(dest)
    }

    /// Удаляет недокачанное при старте: в базе (старое место) и в папке каждого аккаунта.
    /// И остатки прерванных копий `.<имя>.part` (миграция, скачивание, «Выбрать файл .ipa…»):
    /// только обычные файлы с таким именем, папки и ссылки не трогаются.
    pub fn sweep_partials(&self) {
        let _one = self.moving.lock().unwrap_or_else(|e| e.into_inner());
        let sweep_parts = |dir: &Path| {
            let Ok(rd) = std::fs::read_dir(dir) else { return };
            for e in rd.filter_map(Result::ok) {
                let part = e.file_name().to_str().is_some_and(is_part);
                if part && e.file_type().is_ok_and(|t| t.is_file()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        };
        let _ = std::fs::remove_dir_all(self.base.join(PARTIAL));
        sweep_parts(&self.base);
        let Ok(rd) = std::fs::read_dir(&self.base) else { return };
        for e in rd.filter_map(Result::ok).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
            let _ = std::fs::remove_dir_all(e.path().join(PARTIAL));
            sweep_parts(&e.path());
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::io::Write;
    use std::path::Path;

    /// Минимальный ipa: `Payload/X.app/Info.plist`.
    pub fn make_ipa(path: &Path, bundle: &str, version: &str) {
        make_ipa_family(path, bundle, version, &[]);
    }

    /// ipa с `iTunesMetadata.plist` в корне архива, как кладёт ipatool. Пары: `("apple-id", адрес)` —
    /// ключ в корне; `("downloadInfo", адрес)` — новый формат `com.apple.iTunesStore.downloadInfo`.
    pub fn make_ipa_meta(path: &Path, bundle: &str, version: &str, meta: &[(&str, &str)]) {
        make_ipa(path, bundle, version);
        let mut d = plist::Dictionary::new();
        for (k, v) in meta {
            if *k == "downloadInfo" {
                let mut ai = plist::Dictionary::new();
                ai.insert("AppleID".into(), (*v).into());
                let mut di = plist::Dictionary::new();
                di.insert("accountInfo".into(), plist::Value::Dictionary(ai));
                d.insert("com.apple.iTunesStore.downloadInfo".into(), plist::Value::Dictionary(di));
            } else {
                d.insert((*k).into(), (*v).into());
            }
        }
        let mut xml = Vec::new();
        plist::to_writer_xml(&mut xml, &plist::Value::Dictionary(d)).unwrap();
        let f = std::fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
        let mut z = zip::ZipWriter::new_append(f).unwrap();
        z.start_file("iTunesMetadata.plist", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(&xml).unwrap();
        z.finish().unwrap();
    }

    /// То же с `UIDeviceFamily` (пустой список — без ключа).
    pub fn make_ipa_family(path: &Path, bundle: &str, version: &str, family: &[u8]) {
        let mut d = plist::Dictionary::new();
        if !family.is_empty() {
            let f = family.iter().map(|x| plist::Value::Integer((*x as i64).into())).collect();
            d.insert("UIDeviceFamily".into(), plist::Value::Array(f));
        }
        d.insert("CFBundleIdentifier".into(), bundle.into());
        d.insert("CFBundleShortVersionString".into(), version.into());
        let mut xml = Vec::new();
        plist::to_writer_xml(&mut xml, &plist::Value::Dictionary(d)).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        z.start_file("Payload/X.app/Info.plist", zip::write::SimpleFileOptions::default()).unwrap();
        z.write_all(&xml).unwrap();
        z.finish().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::{make_ipa, make_ipa_meta};
    use super::*;

    const ME: &str = "me@example.com";

    /// Библиотека с входом в `ME`: (библиотека, папка аккаунта).
    fn acct(base: PathBuf) -> (Library, PathBuf) {
        let lib = Library::new(base.clone());
        lib.set_account(Some(ME)).unwrap();
        (lib, base.join(ME))
    }

    #[test]
    fn commit_names_file_by_bundle_and_plist_version() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().join("lib"));
        let part = lib.partial_dir(492224193).unwrap();
        // ipatool -i называет файл <trackId>_<версия сборки>; версия и bundle берутся из Info.plist
        let src = part.join("492224193_9999.ipa");
        make_ipa(&src, "com.example.app", "2.1.1");
        let dest = lib.commit(src.parent().unwrap(), &src).unwrap();
        assert_eq!(dest, root.join("492224193_com.example.app_2.1.1.ipa"));
        assert!(dest.is_file() && !src.exists());
    }

    #[test]
    fn commit_falls_back_to_file_name_when_plist_unreadable() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().join("lib"));
        let src = lib.partial_dir(492224193).unwrap().join("com.example.app_123_3.4.ipa");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"not a zip").unwrap();
        assert_eq!(lib.commit(src.parent().unwrap(), &src).unwrap(), root.join("123_com.example.app_3.4.ipa"));
    }

    #[test]
    fn commit_without_bundle_anywhere_is_error() {
        let t = tempfile::tempdir().unwrap();
        let (lib, _) = acct(t.path().join("lib"));
        let src = lib.partial_dir(5).unwrap().join("5_1.0.ipa");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"not a zip").unwrap();
        assert!(matches!(lib.commit(src.parent().unwrap(), &src), Err(AppError::Unknown { .. })));
        assert!(src.exists());
    }

    #[test]
    fn commit_unknown_name_and_bad_zip_is_error() {
        let t = tempfile::tempdir().unwrap();
        let (lib, _) = acct(t.path().join("lib"));
        let src = t.path().join("weird.ipa");
        std::fs::write(&src, b"x").unwrap();
        assert!(matches!(lib.commit(t.path(), &src), Err(AppError::Unknown { .. })));
        assert!(src.exists());
    }

    #[test]
    fn sweep_partials_removes_unfinished() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().to_path_buf());
        let p = lib.partial_dir(492224193).unwrap();
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("half.ipa"), b"123").unwrap();
        lib.sweep_partials();
        assert!(!root.join(PARTIAL).exists());
        lib.sweep_partials(); // повторный запуск безопасен
    }

    #[test]
    fn list_sees_saved_and_not_partial() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().to_path_buf());
        make_ipa(&root.join("com.example.a_1.0.ipa"), "com.example.a", "1.0");
        make_ipa(&root.join("492224193_ru.sberbankmobile_12.15.0.ipa"), "ru.sberbankmobile", "12.15.0");
        make_ipa(&lib.partial_dir(7).unwrap().join("7_1.0.ipa"), "com.example.b", "1.0");
        std::fs::write(root.join("notes.txt"), b"").unwrap();
        let l = lib.list();
        assert_eq!(l.len(), 2);
        // старое имя без trackId
        let old = l.iter().find(|x| x.bundle_id == "com.example.a").unwrap();
        assert_eq!((old.track_id, old.version.as_str()), (None, "1.0"));
        let new = l.iter().find(|x| x.bundle_id == "ru.sberbankmobile").unwrap();
        assert_eq!((new.track_id, new.version.as_str()), (Some(492224193), "12.15.0"));
    }

    #[test]
    fn list_reads_device_family() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().to_path_buf());
        super::testutil::make_ipa_family(&root.join("656485245_com.vk.vkhd_2.0.4.ipa"), "com.vk.vkhd", "2.0.4", &[2]);
        super::testutil::make_ipa_family(&root.join("1_com.example.u_1.0.ipa"), "com.example.u", "1.0", &[1, 2]);
        make_ipa(&root.join("2_com.example.n_1.0.ipa"), "com.example.n", "1.0");
        std::fs::write(root.join("3_com.example.bad_1.0.ipa"), b"not a zip").unwrap();
        let fam = |b: &str| lib.list().into_iter().find(|x| x.bundle_id == b).unwrap().device_family;
        assert_eq!(fam("com.vk.vkhd"), [2]);
        assert_eq!(fam("com.example.u"), [1, 2]);
        assert!(fam("com.example.n").is_empty());
        assert!(fam("com.example.bad").is_empty());
        // второй раз из кэша, тот же итог
        assert_eq!(fam("com.vk.vkhd"), [2]);
    }

    #[test]
    fn ensure_root_creates_folder_once() {
        let t = tempfile::tempdir().unwrap();
        // без входа — общая папка
        let lib = Library::new(t.path().join("a/Мои приложения"));
        assert_eq!(lib.ensure_root().unwrap(), t.path().join("a/Мои приложения"));
        assert!(t.path().join("a/Мои приложения").is_dir());
        assert!(lib.ensure_root().is_ok(), "повтор безопасен");
        // со входом — папка аккаунта
        lib.set_account(Some(ME)).unwrap();
        assert_eq!(lib.ensure_root().unwrap(), t.path().join("a/Мои приложения").join(ME));
        assert!(t.path().join("a/Мои приложения").join(ME).is_dir());
        // на месте папки файл — понятная ошибка, не паника
        let f = t.path().join("file");
        std::fs::write(&f, b"").unwrap();
        assert!(matches!(Library::new(f.join("x")).ensure_root(), Err(AppError::Unknown { .. })));
    }

    #[test]
    fn partial_dir_rejects_invalid_ids() {
        let (lib, _) = acct("/r".into());
        for bad in [0i64, -1, i64::MIN] {
            assert!(lib.partial_dir(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn commit_sanitizes_plist_names_and_stays_in_root() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().join("lib"));
        let src = lib.partial_dir(492224193).unwrap().join("1_1.0.ipa");
        make_ipa(&src, "../evil", "../../1.0");
        let dest = lib.commit(src.parent().unwrap(), &src).unwrap();
        assert_eq!(dest.parent().unwrap(), root);
        assert!(!dest.file_name().unwrap().to_str().unwrap().starts_with('.'));
        assert!(!t.path().join("evil_1.0.ipa").exists());
    }

    #[test]
    fn commit_rejects_path_outside_partial() {
        let t = tempfile::tempdir().unwrap();
        let (lib, _) = acct(t.path().join("lib"));
        let part = lib.partial_dir(492224193).unwrap();
        std::fs::create_dir_all(&part).unwrap();
        let outside = t.path().join("492224193_1.0.ipa");
        make_ipa(&outside, "com.example.app", "1.0");
        let e = lib.commit(&part, &outside);
        assert!(matches!(e, Err(AppError::Unknown { ref detail }) if detail == "unexpected download path"), "{e:?}");
        assert!(outside.exists());
        let dotdot = part.join("../../492224193_1.0.ipa");
        assert!(lib.commit(&part, &dotdot).is_err());
    }

    #[test]
    fn oversized_info_plist_is_ignored() {
        let t = tempfile::tempdir().unwrap();
        let ipa = t.path().join("com.example.big_1_7.0.ipa");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&ipa).unwrap());
        z.start_file("Payload/X.app/Info.plist", zip::write::SimpleFileOptions::default()).unwrap();
        std::io::Write::write_all(&mut z, &vec![b' '; 2 * 1024 * 1024]).unwrap();
        z.finish().unwrap();
        assert_eq!(read_info_plist(&ipa).unwrap_or((None, None)), (None, None));
    }

    #[test]
    fn list_of_missing_root_is_empty() {
        let t = tempfile::tempdir().unwrap();
        assert!(Library::new(t.path().join("nope")).list().is_empty());
    }

    #[test]
    fn partial_dir_path() {
        let (lib, _) = acct("/r".into());
        assert_eq!(lib.partial_dir(42).unwrap(), PathBuf::from("/r/me@example.com/.partial/42"));
        // без входа скачивать некуда
        assert_eq!(Library::new("/r".into()).partial_dir(42), Err(AppError::NotLoggedIn));
    }

    #[test]
    fn account_folder_name_is_sanitized() {
        assert_eq!(account_dir_name(" Me@Example.COM ").unwrap(), "me@example.com");
        assert_eq!(account_dir_name("a+b.c-d_e@example.com").unwrap(), "a+b.c-d_e@example.com");
        assert_eq!(account_dir_name("я@example.com").unwrap(), "_@example.com");
        assert_eq!(account_dir_name("a/b\\c:d@example.com").unwrap(), "a_b_c_d@example.com");
        assert_eq!(account_dir_name("a..b@example.com").unwrap(), "a..b@example.com");
        // точка в начале или в конце: скрытая папка, `.partial`, хвост, который Windows срезает
        for bad in ["", "   ", ".", "..", " .. ", ".partial", "x.", ".me@example.com", "../../etc", "../x@example.com"] {
            assert_eq!(account_dir_name(bad), Err(AppError::BadAccountFolder), "{bad:?}");
        }
        // разделители превращаются в `_`: имя всегда одно звено пути
        let n = account_dir_name("a/../x@example.com").unwrap();
        assert!(!n.contains('/') && !n.contains('\\'), "{n}");
        let lib = Library::new("/r".into());
        assert!(lib.set_account(Some("..")).is_err());
        assert_eq!(lib.root(), None, "после ошибки входа папки нет");
        // и понятная ошибка вместо «не вошли»
        assert_eq!(lib.partial_dir(1), Err(AppError::BadAccountFolder));
        assert_eq!(lib.ensure_root(), Err(AppError::BadAccountFolder));
        assert!(lib.list().is_empty());
        lib.set_account(None).unwrap();
        assert_eq!(lib.partial_dir(1), Err(AppError::NotLoggedIn));
    }

    #[test]
    fn saved_name_ignores_copy_suffix() {
        assert_eq!(parse_saved_name("1_com.example.a_1.0 (2).ipa"), Some((Some(1), "com.example.a".into(), "1.0".into())));
        assert_eq!(parse_saved_name("com.example.a_1.0 (13).ipa"), Some((None, "com.example.a".into(), "1.0".into())));
        // не наш суффикс — как есть
        assert_eq!(parse_saved_name("1_com.example.a_1.0 (x).ipa"), Some((Some(1), "com.example.a".into(), "1.0 (x)".into())));
    }

    #[test]
    fn unknown_purchaser_copies_in_base_are_listed_separately() {
        let t = tempfile::tempdir().unwrap();
        let (lib, mine) = acct(t.path().to_path_buf());
        make_ipa(&t.path().join("5_com.example.u_1.0.ipa"), "com.example.u", "1.0");
        make_ipa_meta(&t.path().join("6_com.example.k_1.0.ipa"), "com.example.k", "1.0", &[("apple-id", "other@example.com")]);
        make_ipa(&mine.join("1_com.example.a_1.0.ipa"), "com.example.a", "1.0");
        let u = lib.list_unknown();
        assert_eq!(u.iter().map(|x| x.track_id).collect::<Vec<_>>(), vec![Some(5)], "покупатель известен — не «неизвестный»");
        assert_eq!(lib.list().len(), 1, "в списке аккаунта их нет");
        lib.set_account(None).unwrap();
        assert!(lib.list_unknown().is_empty(), "без входа копии выключены");
    }

    /// Сбои по заказу поверх настоящих операций.
    #[derive(Default)]
    struct Faulty {
        /// Перенос без замены всегда падает (как между томами).
        no_rename: bool,
        /// Удаление файлов из этой папки падает.
        undeletable: Option<PathBuf>,
        /// Копия пишет половину и падает.
        copy_breaks: bool,
    }
    impl FileOps for Faulty {
        fn rename_noreplace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            if self.no_rename {
                return Err(std::io::Error::other("cross-device"));
            }
            RealOps.rename_noreplace(from, to)
        }
        fn copy_synced(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            if self.copy_breaks {
                let half = std::fs::read(from)?;
                std::fs::write(to, &half[..half.len() / 2])?;
                return Err(std::io::Error::other("disk gone"));
            }
            RealOps.copy_synced(from, to)
        }
        fn remove(&self, p: &Path) -> std::io::Result<()> {
            if self.undeletable.as_deref().is_some_and(|d| p.parent() == Some(d)) {
                return Err(std::io::Error::other("busy"));
            }
            RealOps.remove(p)
        }
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        v.sort();
        v
    }

    #[test]
    fn migrate_rename_and_remove_failures_leave_no_duplicates() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().to_path_buf();
        make_ipa_meta(&base.join("1_com.example.a_1.0.ipa"), "com.example.a", "1.0", &[("apple-id", ME)]);
        let lib = Library::new(base.clone());
        let ops = Faulty { no_rename: true, undeletable: Some(base.clone()), ..Default::default() };
        for _ in 0..3 {
            assert_eq!(lib.migrate_with(&ops), 0);
        }
        assert!(files(&base.join(ME)).is_empty(), "{:?}", files(&base.join(ME)));
        assert!(base.join("1_com.example.a_1.0.ipa").is_file(), "исходник на месте");
        // помеха ушла — следующий запуск переносит чисто
        assert_eq!(lib.migrate(), 1);
        assert_eq!(files(&base.join(ME)), vec!["1_com.example.a_1.0.ipa"]);
    }

    #[test]
    fn migrate_copy_fallback_when_rename_fails() {
        // первый перенос (исходник -> папка) падает, как между томами: идём через .part
        let t2 = tempfile::tempdir().unwrap();
        let b2 = t2.path().to_path_buf();
        make_ipa_meta(&b2.join("1_com.example.a_1.0.ipa"), "com.example.a", "1.0", &[("apple-id", ME)]);
        let want = std::fs::read(b2.join("1_com.example.a_1.0.ipa")).unwrap();
        let lib2 = Library::new(b2.clone());
        struct FirstRenameFails(std::sync::atomic::AtomicBool);
        impl FileOps for FirstRenameFails {
            fn rename_noreplace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                if !self.0.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    return Err(std::io::Error::other("cross-device"));
                }
                RealOps.rename_noreplace(from, to)
            }
            fn copy_synced(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                RealOps.copy_synced(from, to)
            }
            fn remove(&self, p: &Path) -> std::io::Result<()> {
                RealOps.remove(p)
            }
        }
        assert_eq!(lib2.migrate_with(&FirstRenameFails(Default::default())), 1);
        assert_eq!(files(&b2.join(ME)), vec!["1_com.example.a_1.0.ipa"], "без .part");
        assert_eq!(std::fs::read(b2.join(ME).join("1_com.example.a_1.0.ipa")).unwrap(), want);
        assert!(!b2.join("1_com.example.a_1.0.ipa").exists());
    }

    #[test]
    fn migrate_broken_copy_leaves_no_truncated_file() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().to_path_buf();
        make_ipa_meta(&base.join("1_com.example.a_1.0.ipa"), "com.example.a", "1.0", &[("apple-id", ME)]);
        let lib = Library::new(base.clone());
        let ops = Faulty { no_rename: true, copy_breaks: true, ..Default::default() };
        assert_eq!(lib.migrate_with(&ops), 0);
        assert!(files(&base.join(ME)).is_empty(), "{:?}", files(&base.join(ME)));
        assert!(base.join("1_com.example.a_1.0.ipa").is_file());
    }

    #[test]
    fn migrate_identical_copy_is_not_duplicated() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().to_path_buf();
        let src = base.join("1_com.example.a_1.0.ipa");
        make_ipa_meta(&src, "com.example.a", "1.0", &[("apple-id", ME)]);
        std::fs::create_dir_all(base.join(ME)).unwrap();
        // та же копия под другим именем
        std::fs::copy(&src, base.join(ME).join("1_com.example.a_1.0 (2).ipa")).unwrap();
        let lib = Library::new(base.clone());
        assert_eq!(lib.migrate(), 1);
        assert_eq!(files(&base.join(ME)), vec!["1_com.example.a_1.0 (2).ipa"]);
        assert!(!src.exists());
        // убрать исходник не вышло — оба остаются, копий не прибавляется
        make_ipa_meta(&src, "com.example.a", "1.0", &[("apple-id", ME)]);
        std::fs::copy(&src, base.join(ME).join("1_com.example.a_1.0 (2).ipa")).unwrap();
        let ops = Faulty { undeletable: Some(base.clone()), ..Default::default() };
        assert_eq!(lib.migrate_with(&ops), 0);
        assert_eq!(files(&base.join(ME)), vec!["1_com.example.a_1.0 (2).ipa"]);
        assert!(src.exists());
    }

    /// Системный перенос без замены (для томов без жёстких ссылок).
    #[cfg(target_os = "macos")]
    #[test]
    fn os_rename_noreplace_never_overwrites() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, c) = (t.path().join("a"), t.path().join("b"), t.path().join("c"));
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        assert_eq!(os_rename_noreplace(&a, &b).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&b).unwrap(), b"old");
        os_rename_noreplace(&a, &c).unwrap();
        assert!(!a.exists() && std::fs::read(&c).unwrap() == b"new");
    }

    #[test]
    fn rename_noreplace_never_overwrites() {
        let t = tempfile::tempdir().unwrap();
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        std::fs::write(&a, b"new").unwrap();
        std::fs::write(&b, b"old").unwrap();
        let e = RealOps.rename_noreplace(&a, &b).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!((std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap()), (b"new".to_vec(), b"old".to_vec()));
        let c = t.path().join("c");
        RealOps.rename_noreplace(&a, &c).unwrap();
        assert!(!a.exists() && std::fs::read(&c).unwrap() == b"new");
    }

    #[test]
    fn list_and_commit_are_per_account() {
        let t = tempfile::tempdir().unwrap();
        let (lib, mine) = acct(t.path().to_path_buf());
        make_ipa(&mine.join("1_com.example.a_1.0.ipa"), "com.example.a", "1.0");
        make_ipa(&t.path().join("other@example.com/2_com.example.b_1.0.ipa"), "com.example.b", "1.0");
        make_ipa(&t.path().join("3_com.example.c_1.0.ipa"), "com.example.c", "1.0");
        let ids = |l: &Library| l.list().iter().map(|x| x.track_id).collect::<Vec<_>>();
        assert_eq!(ids(&lib), vec![Some(1)]);
        lib.set_account(Some("OTHER@example.com")).unwrap();
        assert_eq!(ids(&lib), vec![Some(2)]);
        lib.set_account(None).unwrap();
        assert!(lib.list().is_empty(), "без входа копий нет");
        // скачанное в папку аккаунта, с которым начали, даже если вход сменился посреди
        lib.set_account(Some(ME)).unwrap();
        let src = lib.partial_dir(9).unwrap().join("9_1.0.ipa");
        make_ipa(&src, "com.example.n", "1.0");
        lib.set_account(Some("other@example.com")).unwrap();
        assert_eq!(lib.commit(src.parent().unwrap(), &src).unwrap(), mine.join("9_com.example.n_1.0.ipa"));
    }

    #[test]
    fn sweep_partials_covers_every_account() {
        let t = tempfile::tempdir().unwrap();
        for d in [".partial/1", "me@example.com/.partial/2", "other@example.com/.partial/3"] {
            std::fs::create_dir_all(t.path().join(d)).unwrap();
        }
        std::fs::write(t.path().join("me@example.com/1_com.example.a_1.0.ipa"), b"x").unwrap();
        Library::new(t.path().to_path_buf()).sweep_partials();
        for d in [".partial", "me@example.com/.partial", "other@example.com/.partial"] {
            assert!(!t.path().join(d).exists(), "{d}");
        }
        assert!(t.path().join("me@example.com/1_com.example.a_1.0.ipa").exists());
    }

    #[test]
    fn migrate_moves_by_purchaser_and_is_idempotent() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path();
        // старый формат ipatool: apple-id в корне iTunesMetadata.plist
        make_ipa_meta(&base.join("1_com.example.a_1.0.ipa"), "com.example.a", "1.0", &[("apple-id", "Me@Example.com")]);
        // новый формат
        make_ipa_meta(&base.join("2_com.example.b_1.0.ipa"), "com.example.b", "1.0", &[("downloadInfo", "other@example.com")]);
        // без покупателя и не zip — остаются на месте
        make_ipa(&base.join("3_com.example.c_1.0.ipa"), "com.example.c", "1.0");
        std::fs::write(base.join("4_com.example.d_1.0.ipa"), b"not a zip").unwrap();
        // в папке аккаунта уже есть такой файл: не затираем
        std::fs::create_dir_all(base.join(ME)).unwrap();
        std::fs::write(base.join(ME).join("1_com.example.a_1.0.ipa"), b"old").unwrap();
        let lib = Library::new(base.to_path_buf());
        let moved = lib.migrate();
        assert_eq!(moved, 2);
        assert_eq!(std::fs::read(base.join(ME).join("1_com.example.a_1.0.ipa")).unwrap(), b"old");
        assert!(base.join(ME).join("1_com.example.a_1.0 (2).ipa").is_file());
        assert!(base.join("other@example.com/2_com.example.b_1.0.ipa").is_file());
        assert!(base.join("3_com.example.c_1.0.ipa").is_file() && base.join("4_com.example.d_1.0.ipa").is_file());
        assert!(!base.join("1_com.example.a_1.0.ipa").exists() && !base.join("2_com.example.b_1.0.ipa").exists());
        // повтор ничего не меняет
        assert_eq!(lib.migrate(), 0);
        assert!(!base.join(ME).join("1_com.example.a_1.0 (3).ipa").exists());
        // перенесённое видно в списке аккаунта
        lib.set_account(Some(ME)).unwrap();
        assert_eq!(lib.list().len(), 2);
    }

    /// Счётчик одновременных операций: перенос долгий, чтобы два прохода наверняка встретились.
    #[derive(Default)]
    struct Slow {
        now: std::sync::atomic::AtomicUsize,
        max: std::sync::atomic::AtomicUsize,
    }
    impl FileOps for Slow {
        fn rename_noreplace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            use std::sync::atomic::Ordering::SeqCst;
            let n = self.now.fetch_add(1, SeqCst) + 1;
            self.max.fetch_max(n, SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(60));
            self.now.fetch_sub(1, SeqCst);
            RealOps.rename_noreplace(from, to)
        }
        fn copy_synced(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            RealOps.copy_synced(from, to)
        }
        fn remove(&self, p: &Path) -> std::io::Result<()> {
            RealOps.remove(p)
        }
    }

    #[test]
    fn concurrent_migrations_never_overlap() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().to_path_buf();
        for i in 1..=3 {
            make_ipa_meta(&base.join(format!("{i}_com.example.a{i}_1.0.ipa")), &format!("com.example.a{i}"), "1.0", &[("apple-id", ME)]);
        }
        let lib = std::sync::Arc::new(Library::new(base.clone()));
        let ops = std::sync::Arc::new(Slow::default());
        // как миграция при запуске и миграция при входе
        let run = || {
            let (l, o) = (lib.clone(), ops.clone());
            std::thread::spawn(move || l.migrate_with(o.as_ref()))
        };
        let (a, b) = (run(), run());
        let moved = a.join().unwrap() + b.join().unwrap();
        assert_eq!(ops.max.load(std::sync::atomic::Ordering::SeqCst), 1, "миграции шли одновременно");
        assert_eq!(moved, 3);
        assert_eq!(files(&base.join(ME)), vec!["1_com.example.a1_1.0.ipa", "2_com.example.a2_1.0.ipa", "3_com.example.a3_1.0.ipa"]);
    }

    #[test]
    fn sweep_partials_removes_stale_part_files() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path();
        std::fs::create_dir_all(base.join(ME)).unwrap();
        for p in [".1_com.example.a_1.0.ipa.part", ".0123456789abcdef.part"] {
            std::fs::write(base.join(ME).join(p), b"half").unwrap();
        }
        std::fs::write(base.join(".x.ipa.part"), b"half").unwrap();
        // настоящие файлы владельца не трогаем
        std::fs::write(base.join(ME).join("1_com.example.a_1.0.ipa"), b"x").unwrap();
        std::fs::write(base.join(ME).join("notes.part"), b"x").unwrap();
        std::fs::create_dir_all(base.join(ME).join(".dir.part")).unwrap();
        Library::new(base.to_path_buf()).sweep_partials();
        assert_eq!(files(&base.join(ME)), vec![".dir.part", "1_com.example.a_1.0.ipa", "notes.part"]);
        assert!(!base.join(".x.ipa.part").exists());
    }

    #[test]
    fn commit_replaces_own_download_and_drops_numbered_siblings() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().join("lib"));
        let canon = root.join("7_com.example.app_1.0.ipa");
        let down = |bytes: &[u8]| {
            let src = lib.partial_dir(7).unwrap().join("7_1.0.ipa");
            make_ipa_meta(&src, "com.example.app", "1.0", &[("note", std::str::from_utf8(bytes).unwrap())]);
            let want = std::fs::read(&src).unwrap();
            let dest = lib.commit(src.parent().unwrap(), &src).unwrap();
            assert!(!src.exists());
            (dest, want)
        };
        let (d1, _) = down(b"first");
        assert_eq!(d1, canon);
        // старые дубли той же версии от прошлых сборок, и другая версия
        std::fs::write(root.join("7_com.example.app_1.0 (2).ipa"), b"dup").unwrap();
        std::fs::write(root.join("7_com.example.app_1.0 (3).ipa"), b"dup").unwrap();
        std::fs::write(root.join("7_com.example.app_0.9.ipa"), b"older").unwrap();
        std::fs::write(root.join("8_com.example.other_1.0 (2).ipa"), b"other").unwrap();
        let (d2, want2) = down(b"second, different bytes");
        assert_eq!(d2, canon);
        assert_eq!(std::fs::read(&canon).unwrap(), want2, "второе скачивание заменило первое");
        assert_eq!(
            files(&root).into_iter().filter(|f| f.ends_with(".ipa")).collect::<Vec<_>>(),
            vec!["7_com.example.app_0.9.ipa", "7_com.example.app_1.0.ipa", "8_com.example.other_1.0 (2).ipa"]
        );
        assert!(files(&root).iter().all(|f| !f.ends_with(".part")));
    }

    /// Перенос с заменой падает (Windows: файл занят); `bad` — куда перенос без замены тоже падает.
    struct Locked {
        bad: fn(&Path) -> bool,
    }
    impl FileOps for Locked {
        fn rename_noreplace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            if (self.bad)(to) {
                return Err(std::io::Error::other("denied"));
            }
            RealOps.rename_noreplace(from, to)
        }
        fn copy_synced(&self, from: &Path, to: &Path) -> std::io::Result<()> {
            RealOps.copy_synced(from, to)
        }
        fn remove(&self, p: &Path) -> std::io::Result<()> {
            RealOps.remove(p)
        }
        fn rename_replace(&self, _: &Path, _: &Path) -> std::io::Result<()> {
            Err(std::io::Error::other("file in use"))
        }
    }

    #[test]
    fn commit_keeps_new_download_numbered_when_canonical_is_locked() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().join("lib"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("7_com.example.app_1.0.ipa"), b"old, in use").unwrap();
        let src = lib.partial_dir(7).unwrap().join("7_1.0.ipa");
        make_ipa(&src, "com.example.app", "1.0");
        let want = std::fs::read(&src).unwrap();
        let dest = lib.commit_with(&Locked { bad: |_| false }, src.parent().unwrap(), &src).unwrap();
        assert_eq!(dest, root.join("7_com.example.app_1.0 (2).ipa"), "новое скачивание не теряется и не удаляется как дубль");
        assert_eq!(std::fs::read(&dest).unwrap(), want);
        assert_eq!(std::fs::read(root.join("7_com.example.app_1.0.ipa")).unwrap(), b"old, in use");
        assert!(files(&root).iter().all(|f| !f.ends_with(".part")));
    }

    #[test]
    fn commit_never_deletes_the_download_when_it_cannot_be_put_back() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().join("lib"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("7_com.example.app_1.0.ipa"), b"old").unwrap();
        let src = lib.partial_dir(7).unwrap().join("7_1.0.ipa");
        make_ipa(&src, "com.example.app", "1.0");
        let want = std::fs::read(&src).unwrap();
        // ни « (n)», ни обратно в .partial — только под именем .recovered
        let bad = |to: &Path| {
            let n = to.file_name().unwrap().to_string_lossy().into_owned();
            n.contains(" (") || to.parent().is_some_and(|p| p.ends_with("7"))
        };
        assert!(lib.commit_with(&Locked { bad }, src.parent().unwrap(), &src).is_err());
        let rec = root.join("7_com.example.app_1.0.recovered.ipa");
        assert_eq!(std::fs::read(&rec).unwrap(), want, "скачанное сохранено");
        assert_eq!(std::fs::read(root.join("7_com.example.app_1.0.ipa")).unwrap(), b"old");
    }

    #[test]
    fn is_ipa_zip_needs_something_inside_app() {
        let t = tempfile::tempdir().unwrap();
        let only_dir = t.path().join("d.ipa");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&only_dir).unwrap());
        z.add_directory("Payload/X.app/", zip::write::SimpleFileOptions::default()).unwrap();
        z.finish().unwrap();
        assert!(!is_ipa_zip(&only_dir));
        let lib = Library::new(t.path().join("lib"));
        assert_eq!(lib.import_ipa(&only_dir).unwrap_err(), AppError::NotIpa);
        make_ipa(&t.path().join("ok.ipa"), "com.example.a", "1.0");
        assert!(is_ipa_zip(&t.path().join("ok.ipa")));
    }

    #[test]
    fn commit_copy_fallback_goes_through_part() {
        let t = tempfile::tempdir().unwrap();
        let (lib, root) = acct(t.path().join("lib"));
        let src = lib.partial_dir(7).unwrap().join("7_1.0.ipa");
        make_ipa(&src, "com.example.app", "1.0");
        let want = std::fs::read(&src).unwrap();
        // перенос не работает (как между томами), копия ломается: под настоящим именем ничего
        let broken = Faulty { no_rename: true, copy_breaks: true, ..Default::default() };
        assert!(lib.commit_with(&broken, src.parent().unwrap(), &src).is_err());
        assert!(files(&root).iter().all(|f| f == PARTIAL), "{:?}", files(&root));
        assert!(src.exists());
        // копия целая: через .part, затем перенос без замены
        struct CopyOnly;
        impl FileOps for CopyOnly {
            fn rename_noreplace(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                if from.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".part")) {
                    RealOps.rename_noreplace(from, to)
                } else {
                    Err(std::io::Error::other("cross-device"))
                }
            }
            fn copy_synced(&self, from: &Path, to: &Path) -> std::io::Result<()> {
                assert!(to.file_name().unwrap().to_string_lossy().ends_with(".part"), "копия только во временное имя");
                RealOps.copy_synced(from, to)
            }
            fn remove(&self, p: &Path) -> std::io::Result<()> {
                RealOps.remove(p)
            }
        }
        let dest = lib.commit_with(&CopyOnly, src.parent().unwrap(), &src).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), want);
        assert!(!src.exists());
    }

    #[test]
    fn part_name_stays_within_255_bytes() {
        assert_eq!(part_name("1_a_1.0.ipa"), ".1_a_1.0.ipa.part");
        let long = format!("{}.ipa", "я".repeat(125)); // 254 байта
        let p = part_name(&long);
        assert!(p.len() <= 255 && p.starts_with('.') && p.ends_with(".part"), "{p}");
        assert_eq!(p, part_name(&long), "одно имя на один файл");
        assert_ne!(p, part_name(&format!("{}.ipa", "ю".repeat(125))));
    }

    fn copy<'a>(all: &'a [CopyInfo], bundle: &str) -> &'a CopyInfo {
        all.iter().find(|c| c.bundle_id == bundle).unwrap_or_else(|| panic!("нет {bundle}"))
    }

    #[cfg(unix)]
    #[test]
    fn all_copies_walks_base_and_every_account() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("lib");
        let outside = t.path().join("outside");
        // в базе, покупатель неизвестен; имя по версии из Info.plist
        make_ipa(&base.join("5_com.example.u_1.0.ipa"), "com.example.u", "1.0");
        // в папке аккаунта, новый формат покупателя, trackId и название из iTunesMetadata
        make_ipa_meta(&base.join(ME).join("any name.ipa"), "com.example.m", "2.0", &[("downloadInfo", ME), ("itemId", "42"), ("itemName", "Мой банк")]);
        // старый формат, iPad-only
        super::testutil::make_ipa_family(&base.join("other@example.com/7_com.example.o_3.0.ipa"), "com.example.o", "3.0", &[2]);
        make_ipa_meta(&base.join("other@example.com/8_com.example.p_1.0.ipa"), "com.example.p", "1.0", &[("apple-id", "Other@Example.com")]);
        // пропускаются: недокачанное, остаток копии, не zip, ссылки наружу
        make_ipa(&base.join(".partial/9/9_1.0.ipa"), "com.example.x1", "1.0");
        make_ipa(&base.join(ME).join(".partial/9/9_1.0.ipa"), "com.example.x2", "1.0");
        make_ipa(&base.join(ME).join(".1_com.example.x3_1.0.ipa.part"), "com.example.x3", "1.0");
        std::fs::write(base.join(ME).join("broken.ipa"), b"not a zip").unwrap();
        make_ipa(&outside.join("1_com.example.x4_1.0.ipa"), "com.example.x4", "1.0");
        std::os::unix::fs::symlink(outside.join("1_com.example.x4_1.0.ipa"), base.join("1_com.example.x4_1.0.ipa")).unwrap();
        std::os::unix::fs::symlink(&outside, base.join("link@example.com")).unwrap();
        let lib = Library::new(base.clone());
        let all = lib.all_copies();
        let mut b: Vec<&str> = all.iter().map(|c| c.bundle_id.as_str()).collect();
        b.sort();
        assert_eq!(b, ["com.example.m", "com.example.o", "com.example.p", "com.example.u"]);
        let u = copy(&all, "com.example.u");
        assert_eq!((u.track_id, u.version.as_str(), u.purchaser.as_deref(), u.account_folder.as_deref()), (Some(5), "1.0", None, None));
        let m = copy(&all, "com.example.m");
        assert_eq!((m.track_id, m.name.as_str(), m.purchaser.as_deref(), m.account_folder.as_deref()), (Some(42), "Мой банк", Some(ME), Some(ME)));
        let o = copy(&all, "com.example.o");
        assert_eq!((o.device_family.clone(), o.account_folder.as_deref()), (vec![2], Some("other@example.com")));
        assert_eq!(copy(&all, "com.example.p").purchaser.as_deref(), Some("Other@Example.com"));
        // без входа то же самое: «Все копии» не зависят от входа
        lib.set_account(Some(ME)).unwrap();
        assert_eq!(lib.all_copies().len(), 4);
        // второй раз из кэша
        assert_eq!(lib.all_copies(), all_sorted(lib.all_copies()));
    }

    fn all_sorted(mut v: Vec<CopyInfo>) -> Vec<CopyInfo> {
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }

    #[cfg(unix)]
    #[test]
    fn resolve_copy_accepts_only_files_inside_base() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("lib");
        make_ipa(&base.join(ME).join("1_com.example.a_1.0.ipa"), "com.example.a", "1.0");
        make_ipa(&t.path().join("out.ipa"), "com.example.o", "1.0");
        std::os::unix::fs::symlink(t.path().join("out.ipa"), base.join(ME).join("2_com.example.o_1.0.ipa")).unwrap();
        let lib = Library::new(base.clone());
        let ok = lib.resolve_copy(&base.join(ME).join("1_com.example.a_1.0.ipa")).unwrap();
        assert!(ok.ends_with("1_com.example.a_1.0.ipa"));
        for bad in [
            t.path().join("out.ipa"),
            base.join(ME).join("../../out.ipa"),
            base.join(ME).join("2_com.example.o_1.0.ipa"), // ссылка наружу
            base.join(ME).join("nope.ipa"),
            base.join(ME), // папка
        ] {
            assert!(lib.resolve_copy(&bad).is_err(), "{bad:?}");
        }
        make_ipa(&base.join(".partial/3/3_1.0.ipa"), "com.example.p", "1.0");
        assert!(lib.resolve_copy(&base.join(".partial/3/3_1.0.ipa")).is_err(), "недокачанное не ставим");
    }

    #[test]
    fn import_rejects_non_ipa() {
        let t = tempfile::tempdir().unwrap();
        let lib = Library::new(t.path().join("lib"));
        let junk = t.path().join("junk.ipa");
        std::fs::write(&junk, b"not a zip").unwrap();
        assert_eq!(lib.import_ipa(&junk).unwrap_err(), AppError::NotIpa);
        let zip_no_app = t.path().join("doc.ipa");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_no_app).unwrap());
        z.start_file("Payload/readme.txt", zip::write::SimpleFileOptions::default()).unwrap();
        z.finish().unwrap();
        assert_eq!(lib.import_ipa(&zip_no_app).unwrap_err(), AppError::NotIpa);
        assert!(!t.path().join("lib").exists() || lib.all_copies().is_empty());
    }

    #[test]
    fn import_copies_into_purchaser_folder_without_overwriting() {
        let t = tempfile::tempdir().unwrap();
        let base = t.path().join("lib");
        std::fs::create_dir_all(base.join(ME)).unwrap();
        std::fs::write(base.join(ME).join("5_com.example.k_1.0.ipa"), b"old").unwrap();
        let ext = t.path().join("Downloads/Банк.ipa");
        make_ipa_meta(&ext, "com.example.k", "1.0", &[("apple-id", ME), ("itemId", "5")]);
        let want = std::fs::read(&ext).unwrap();
        let lib = Library::new(base.clone());
        let c = lib.import_ipa(&ext).unwrap();
        assert_eq!(c.path, base.join(ME).join("5_com.example.k_1.0 (2).ipa"));
        assert_eq!(std::fs::read(base.join(ME).join("5_com.example.k_1.0.ipa")).unwrap(), b"old");
        assert_eq!(std::fs::read(&c.path).unwrap(), want);
        assert!(ext.exists(), "исходник не трогаем");
        assert_eq!(c.account_folder.as_deref(), Some(ME));
        // ещё раз тот же файл — без дубля
        assert_eq!(lib.import_ipa(&ext).unwrap().path, c.path);
        // уже лежит в «Мои приложения» — не копируем
        assert_eq!(lib.import_ipa(&c.path).unwrap().path, c.path);
        assert_eq!(files(&base.join(ME)).len(), 2);
        // покупатель неизвестен — в базу, имя по Info.plist
        let anon = t.path().join("x.ipa");
        make_ipa(&anon, "com.example.anon", "4.1");
        assert_eq!(lib.import_ipa(&anon).unwrap().path, base.join("com.example.anon_4.1.ipa"));
    }
}
