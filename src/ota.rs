use anyhow::{anyhow, Result};
use embedded_svc::http::client::Client;
use embedded_svc::http::Headers;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::http::Method;
use esp_idf_svc::ota::{EspFirmwareInfoLoader, EspOta, FirmwareInfo};
use esp_idf_svc::sys::{ESP_ERR_IMAGE_INVALID, ESP_ERR_INVALID_RESPONSE};
use http::{header::ACCEPT, Uri};

pub const UPDATE_BIN_URL: &str =
    "https://github.com/krokosik/esp-pulser/releases/download/vTAG/esp-pulser";

const FIRMWARE_DOWNLOAD_CHUNK_SIZE: usize = 1024 * 20;
// Not expect firmware bigger than partition size
const FIRMWARE_MAX_SIZE: usize = 1_310_720;
const FIRMWARE_MIN_SIZE: usize = size_of::<FirmwareInfo>() + 1024;

pub fn simple_download_and_update_firmware(url: Uri) -> Result<()> {
    let mut client = Client::wrap(EspHttpConnection::new(&Configuration {
        buffer_size: Some(1024 * 6),
        buffer_size_tx: Some(1024),
        use_global_ca_store: true,
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    })?);
    let headers = [(ACCEPT.as_str(), mime::APPLICATION_OCTET_STREAM.as_ref())];
    let surl = url.to_string();
    let request = client
        .request(Method::Get, &surl, &headers)
        .map_err(|e| e.0)?;
    let mut response = request.submit().map_err(|e| e.0)?;
    if response.status() != 200 {
        log::info!("Bad HTTP response: {}", response.status());
        return Err(anyhow!(ESP_ERR_INVALID_RESPONSE));
    }
    let file_size = response.content_len().unwrap_or(0) as usize;
    if file_size <= FIRMWARE_MIN_SIZE {
        log::info!(
            "File size is {file_size}, too small to be a firmware! No need to proceed further."
        );
        return Err(anyhow!(ESP_ERR_IMAGE_INVALID));
    }
    if file_size > FIRMWARE_MAX_SIZE {
        log::info!("File is too big ({file_size} bytes).");
        return Err(anyhow!(ESP_ERR_IMAGE_INVALID));
    }
    let mut ota = EspOta::new()?;
    let mut work = ota.initiate_update()?;
    let mut buff = vec![0; FIRMWARE_DOWNLOAD_CHUNK_SIZE];
    let mut total_read_len: usize = 0;
    let mut got_info = false;
    let dl_result = loop {
        let n = response.read(&mut buff).unwrap_or_default();
        total_read_len += n;
        if !got_info {
            match get_firmware_info(&buff[..n]) {
                Ok(info) => log::info!("Firmware to be downloaded: {info:?}"),
                Err(e) => {
                    log::error!("Failed to get firmware info from downloaded bytes!");
                    break Err(e);
                }
            };
            got_info = true;
        }
        if n > 0 {
            if let Err(e) = work.write(&buff[..n]) {
                log::error!("Failed to write to OTA. {e}");
                break Err(anyhow!(e));
            }
        }
        if total_read_len >= file_size {
            break Ok(());
        }
    };
    if dl_result.is_err() {
        return work.abort().map_err(|e| anyhow!(e));
    }
    if total_read_len < file_size {
        log::error!("Supposed to download {file_size} bytes, but we could only get {total_read_len}. May be network error?");
        return work.abort().map_err(|e| anyhow!(e));
    }
    work.complete().map_err(|e| anyhow!(e))
}

fn get_firmware_info(buff: &[u8]) -> Result<FirmwareInfo> {
    let mut loader = EspFirmwareInfoLoader::new();
    loader.load(buff)?;
    loader.get_info().map_err(|e| anyhow!(e))
}
