//! Тонкий адаптер к USB: usbmuxd -> lockdown -> AFC (`PublicStaging`) -> installation_proxy.
//! Логики здесь нет, всё проверяемое лежит в `device/mod.rs`. Проверено вживую (iOS 27.0.1;
//! iOS 15.8.8 — неактивированный iPad отказывает в installation_proxy, см. `explain`).
use super::{CONNECT_LIMIT, BROWSE_LIMIT, INSTALL_LIMIT, deadline, 
    DeviceHub, InstalledApp, PhoneInfo, installed_from_browse, install_percent, map_install_error, map_service_error, merge_probes, needs_activation_check, open_local, staging_path, upload_percent,
};
use crate::errors::AppError;
use async_trait::async_trait;
use idevice::{
    IdeviceError, IdeviceService,
    afc::{AfcClient, opcode::AfcFopenMode},
    installation_proxy::InstallationProxyClient,
    lockdown::LockdownClient,
    provider::IdeviceProvider,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc::Sender;

const LABEL: &str = "app-restore";
const CHUNK: usize = 4 * 1024 * 1024;

pub struct IdeviceHub;

/// Сырой текст для `map_install_error`: Display теряет вид ошибки сокета, Debug его сохраняет.
fn raw(e: &IdeviceError) -> String {
    format!("{e} / {e:?}")
}

fn app_err(e: IdeviceError) -> AppError {
    map_install_error(&raw(&e))
}

/// Ошибка службы с пояснением: на "InvalidService" спрашиваем у устройства `ActivationState`
/// (читается только внутри сессии lockdown) — неактивированный iPad отказывает именно так.
async fn explain(provider: &dyn IdeviceProvider, e: IdeviceError) -> AppError {
    let raw = raw(&e);
    let state = if needs_activation_check(&raw) {
        tokio::time::timeout(CONNECT_LIMIT, activation_state(provider)).await.ok().flatten()
    } else {
        None
    };
    crate::log::write("device", &format!("service error: {raw}; ActivationState={state:?}"));
    map_service_error(&raw, state.as_deref())
}

async fn activation_state(provider: &dyn IdeviceProvider) -> Option<String> {
    let mut lockdown = LockdownClient::connect(provider).await.ok()?;
    let pairing = provider.get_pairing_file().await.ok()?;
    lockdown.start_session(&pairing).await.ok()?;
    Some(text(lockdown.get_value(Some("ActivationState"), None).await)).filter(|s| !s.is_empty())
}

/// usbmuxd не отвечает: на Windows нет службы «Apple Mobile Device» (Apple Devices), на Mac его
/// сокет есть всегда, поэтому там это просто «телефона нет».
fn mux_err(_e: IdeviceError) -> AppError {
    AppError::DeviceNotFound { drivers_missing: cfg!(windows) }
}

async fn connect_mux() -> Result<UsbmuxdConnection, AppError> {
    UsbmuxdConnection::default().await.map_err(mux_err)
}

fn is_usb(d: &UsbmuxdDevice) -> bool {
    matches!(d.connection_type, Connection::Usb)
}

fn text(v: Result<plist::Value, IdeviceError>) -> String {
    v.ok().and_then(|p| p.as_string().map(str::to_string)).unwrap_or_default()
}

/// Один телефон: lockdown и четыре поля. Ошибка относится только к этому устройству.
async fn probe(dev: UsbmuxdDevice) -> Result<PhoneInfo, AppError> {
    let provider = dev.to_provider(UsbmuxdAddr::default(), LABEL);
    let mut lockdown = LockdownClient::connect(&provider).await.map_err(app_err)?;
    let mut get = async |k: &str| text(lockdown.get_value(Some(k), None).await);
    Ok(PhoneInfo {
        name: get("DeviceName").await,
        ios_version: get("ProductVersion").await,
        device_class: get("DeviceClass").await,
        product_type: get("ProductType").await,
        udid: dev.udid,
    })
}

#[async_trait]
impl DeviceHub for IdeviceHub {
    async fn list(&self) -> Result<Vec<PhoneInfo>, AppError> {
        let mut mux = deadline(CONNECT_LIMIT, "usbmuxd", connect_mux()).await?;
        let devices = deadline(CONNECT_LIMIT, "devices", async { mux.get_devices().await.map_err(app_err) }).await?;
        let mut probes = vec![];
        for dev in devices.into_iter().filter(is_usb) {
            // зависший телефон — сбой только его пробы, остальные в списке
            probes.push(deadline(CONNECT_LIMIT, "lockdown", probe(dev)).await);
        }
        merge_probes(probes)
    }

    async fn install(&self, udid: &str, ipa: &Path, progress: Sender<u8>) -> Result<(), AppError> {
        // Локальный диск проверяем до обращения к телефону.
        let (file, total) = open_local(ipa).await?;
        let mut mux = deadline(CONNECT_LIMIT, "usbmuxd", connect_mux()).await?;
        let dev = deadline(CONNECT_LIMIT, "device", async { mux.get_device(udid).await.map_err(app_err) }).await?;
        let provider = dev.to_provider(UsbmuxdAddr::default(), LABEL);
        let remote = staging_path(ipa);
        let mut afc = match deadline(CONNECT_LIMIT, "afc", async { Ok(AfcClient::connect(&provider).await) }).await? {
            Ok(afc) => afc,
            Err(e) => return Err(explain(&provider, e).await),
        };
        let result = deadline(INSTALL_LIMIT, "install", async {
            match push_and_install(&mut afc, &provider, file, total, &remote, &progress).await {
                Ok(()) => Ok(()),
                Err(e) => Err(explain(&provider, e).await),
            }
        })
        .await;
        // Лучшее, что можно сделать: убрать за собой; ошибки уборки не важны.
        let _ = tokio::time::timeout(CONNECT_LIMIT, afc.remove(remote)).await;
        result
    }

    async fn installed(&self, udid: &str) -> Result<Vec<InstalledApp>, AppError> {
        deadline(BROWSE_LIMIT, "browse", self.browse(udid)).await
    }
}

impl IdeviceHub {
    async fn browse(&self, udid: &str) -> Result<Vec<InstalledApp>, AppError> {
        let mut mux = connect_mux().await?;
        let dev = mux.get_device(udid).await.map_err(app_err)?;
        let provider = dev.to_provider(UsbmuxdAddr::default(), LABEL);
        let mut proxy = match InstallationProxyClient::connect(&provider).await {
            Ok(p) => p,
            Err(e) => return Err(explain(&provider, e).await),
        };
        let mut opts = plist::Dictionary::new();
        opts.insert("ApplicationType".into(), plist::Value::String("User".into()));
        let attrs = BROWSE_ATTRS.iter().map(|a| plist::Value::String(a.to_string())).collect();
        opts.insert("ReturnAttributes".into(), plist::Value::Array(attrs));
        let list = proxy.browse(Some(plist::Value::Dictionary(opts))).await.map_err(app_err)?;
        Ok(installed_from_browse(&list))
    }
}

/// Поля Browse: проверено вживую 26.10 (282 приложения за 225 мс).
const BROWSE_ATTRS: [&str; 6] = [
    "CFBundleIdentifier",
    "CFBundleDisplayName",
    "CFBundleName",
    "CFBundleShortVersionString",
    "CFBundleVersion",
    "iTunesMetadata",
];

async fn push_and_install(
    afc: &mut AfcClient,
    provider: &dyn IdeviceProvider,
    mut file: tokio::fs::File,
    total: u64,
    remote: &str,
    progress: &Sender<u8>,
) -> Result<(), IdeviceError> {
    let _ = afc.mk_dir("PublicStaging").await; // уже есть — не ошибка
    let mut dst = afc.open(remote.to_string(), AfcFopenMode::WrOnly).await?;
    let uploaded = upload(&mut dst, &mut file, total, progress).await;
    // Незакрытый дескриптор AFC в debug-сборке роняет программу: закрываем при любом исходе.
    let closed = dst.close().await;
    uploaded?;
    closed?;
    let mut proxy = InstallationProxyClient::connect(provider).await?;
    let tx = progress.clone();
    proxy
        .install_with_callback(
            remote.to_string(),
            None, // как у ideviceinstaller: PackageType для обычной ipa не нужен
            move |(pct, _)| {
                let tx = tx.clone();
                async move {
                    let _ = tx.try_send(install_percent(pct));
                }
            },
            (),
        )
        .await
}

async fn upload(
    dst: &mut idevice::afc::file::FileDescriptor<'_>,
    file: &mut tokio::fs::File,
    total: u64,
    progress: &Sender<u8>,
) -> Result<(), IdeviceError> {
    let mut buf = vec![0u8; CHUNK];
    let mut sent = 0u64;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            return Ok(());
        }
        dst.write_entire(&buf[..n]).await?;
        sent += n as u64;
        let _ = progress.try_send(upload_percent(sent, total));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Без установки: только опрос usbmuxd. Запуск: `cargo test --lib -- --ignored list_smoke --nocapture`.
    #[tokio::test]
    #[ignore = "нужен usbmuxd; телефон по желанию"]
    async fn list_smoke() {
        let r = IdeviceHub.list().await;
        println!("list -> {r:?}");
        assert!(matches!(r, Ok(_) | Err(AppError::DeviceNotFound { .. } | AppError::DeviceNotTrusted | AppError::DeviceLocked)));
    }

    /// Только чтение: Browse. Печатает лишь количество (список — личные данные владельца).
    /// Запуск: `APP_RESTORE_UDID=<udid> cargo test --lib -- --ignored installed_smoke --nocapture`.
    #[tokio::test]
    #[ignore = "нужен подключённый iPhone"]
    async fn installed_smoke() {
        let udid = std::env::var("APP_RESTORE_UDID").expect("APP_RESTORE_UDID");
        let t = std::time::Instant::now();
        let apps = IdeviceHub.installed(&udid).await.expect("installed");
        let with_id = apps.iter().filter(|a| a.item_id.is_some()).count();
        println!("installed: {} apps, {with_id} with itemId, {:?}", apps.len(), t.elapsed());
        assert!(!apps.is_empty());
    }

    /// Только чтение: Browse и большинство покупателей. Адресов не печатает — только вид
    /// сравнения с выдуманным адресом программы и равны ли аккаунты телефонов между собой.
    /// Запуск: `APP_RESTORE_UDIDS=<udid>,<udid> cargo test --lib -- --ignored phone_account_smoke --nocapture`.
    #[tokio::test]
    #[ignore = "нужны подключённые iPhone"]
    async fn phone_account_smoke() {
        use crate::device::{AccountMatch, account_match, phone_account};
        let udids = std::env::var("APP_RESTORE_UDIDS").expect("APP_RESTORE_UDIDS");
        let mut accounts = vec![];
        for udid in udids.split(',').map(str::trim).filter(|u| !u.is_empty()) {
            let apps = match IdeviceHub.installed(udid).await {
                Ok(a) => a,
                Err(e) => {
                    println!("{udid}: installed failed: {e:?}");
                    continue;
                }
            };
            let with_buyer = apps.iter().filter(|a| a.purchaser.is_some()).count();
            let acc = phone_account(&apps);
            let kind = match account_match(acc.as_deref(), "program@example.com") {
                AccountMatch::Same => "Same",
                AccountMatch::Different { .. } => "Different",
                AccountMatch::Unknown => "Unknown",
            };
            println!("{udid}: {} apps, {with_buyer} with purchaser, vs dummy program -> {kind}", apps.len());
            accounts.push((udid.to_string(), acc));
        }
        for i in 0..accounts.len() {
            for j in i + 1..accounts.len() {
                let (a, b) = (&accounts[i], &accounts[j]);
                let same = matches!(account_match(a.1.as_deref(), b.1.as_deref().unwrap_or("")), AccountMatch::Same);
                println!("{} vs {}: phone accounts {}", a.0, b.0, if same { "equal" } else { "differ or unknown" });
            }
        }
    }
}
