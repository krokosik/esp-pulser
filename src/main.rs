use std::io::Read;
use std::net::TcpListener;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Result};
use display_interface_spi::SPIInterface;
use drv2605::{Drv2605, Effect};
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};
use embedded_svc::http::client::Client;
use embedded_svc::http::Headers;
use esp_idf_svc::eth;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::reset::restart;
use esp_idf_svc::hal::{
    adc, delay, gpio::*, i2c, prelude::*, spi, task::*, timer::*, units::FromValueType,
};
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::http::Method;
use esp_idf_svc::ota::{EspFirmwareInfoLoader, EspOta, FirmwareInfo};
use esp_idf_svc::sys::{EspError, ESP_ERR_IMAGE_INVALID, ESP_ERR_INVALID_RESPONSE};
use esp_idf_svc::timer::EspTaskTimerService;
use http::header::ACCEPT;
use http::Uri;
use log::warn;
use mipidsi::{models::ST7789, options::*, Builder};

use log::info;

mod pulse_sensor;

const SAMPLING_RATE_HZ: u64 = 500;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const UPDATE_BIN_URL: &str =
    "https://github.com/krokosik/esp-pulser/releases/download/vTAG/esp-pulser";

#[derive(Debug, serde::Serialize)]
struct Status {
    version: [u8; 3],
    connected: bool,
    display_ok: bool,
    haptic_ok: bool,
    heart_ok: bool,
}

impl Status {
    fn new() -> Self {
        let mut version = [0; 3];
        for (i, v) in VERSION.split('.').take(3).enumerate() {
            version[i] = v.parse().unwrap_or(0);
        }
        Self {
            version,
            connected: false,
            display_ok: false,
            haptic_ok: false,
            heart_ok: false,
        }
    }
}

fn main() -> Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let mut status = Status::new();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let timer_service = EspTaskTimerService::new()?;

    let pins = peripherals.pins;

    let spi = peripherals.spi2;
    let dc = PinDriver::output(pins.gpio40)?;
    let sclk = pins.gpio36;
    let sdo = pins.gpio35; // mosi
    let sdi = pins.gpio37; // miso
    let spi_rst = PinDriver::output(pins.gpio41)?;
    let tft_cs = pins.gpio42;
    let eth_cs = pins.gpio10;
    let eth_int = pins.gpio13;
    let eth_rst = pins.gpio12;

    info!("Starting ADC");
    let adc_config = adc::oneshot::config::AdcChannelConfig {
        attenuation: adc::attenuation::DB_11,
        ..Default::default()
    };
    let adc_driver = adc::oneshot::AdcDriver::new(peripherals.adc2)?;
    let mut adc = adc::oneshot::AdcChannelDriver::new(adc_driver, pins.gpio18, &adc_config)?;

    status.heart_ok = true;

    info!("ADC started");

    let mut samples = [0u8; 2 * 100 + 4];

    let i2c = peripherals.i2c0;
    let sda = pins.gpio3;
    let scl = pins.gpio4;

    let i2c_driver = i2c::I2cDriver::new(
        i2c,
        sda,
        scl,
        &i2c::config::Config::new().baudrate(400.kHz().into()),
    )?;

    let mut tft_power = PinDriver::output(pins.gpio7)?;
    let mut backlight = PinDriver::output(pins.gpio45)?;

    info!("Pins initialized");

    tft_power.set_high()?;
    backlight.set_high()?;

    info!("Display power on");

    let mut haptic = Drv2605::new(i2c_driver);

    info!("Haptic driver says: {:?}", haptic.init_open_loop_erm());

    info!(
        "Haptic driver effect set to: {:?}",
        haptic.set_single_effect(Effect::PulsingStrongOne100)
    );

    status.haptic_ok = true;

    let config = spi::config::Config::new()
        .baudrate(26.MHz().into())
        .data_mode(spi::config::MODE_3);

    let spi_driver = spi::SpiDriver::new(
        spi,
        sclk,
        sdo,
        Some(sdi),
        &spi::SpiDriverConfig::new().dma(spi::Dma::Auto(4096)),
    )?;

    let tft_spi_device = spi::SpiDeviceDriver::new(&spi_driver, Some(tft_cs), &config)?;

    info!("SPI initialized");

    let mut delay = delay::Ets;

    let di = SPIInterface::new(tft_spi_device, dc);
    let mut display = Builder::new(ST7789, di)
        .display_size(135, 240)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .display_offset(52, 40)
        .invert_colors(ColorInversion::Inverted)
        .reset_pin(spi_rst)
        .init(&mut delay)
        .map_err(|_| anyhow!("display init"))?;

    info!("Display initialized");

    status.display_ok = true;

    display
        .clear(Rgb565::RED)
        .map_err(|_| anyhow!("clear display"))?;

    info!("Display cleared");

    get_styled_text("Unconnected", 100, 50)
        .draw(&mut display)
        .map_err(|_| anyhow!("draw text"))?;

    info!("Text drawn");

    let mut eth = eth::EspEth::wrap(eth::EthDriver::new_spi(
        &spi_driver,
        eth_int,
        Some(eth_cs),
        Some(eth_rst),
        eth::SpiEthChipset::W5500,
        20_u32.MHz().into(),
        Some(&[0x98, 0x76, 0xB6, 0x12, 0xF9, 0x93]),
        None,
        sys_loop.clone(),
    )?)?;

    let ip_info = esp_idf_svc::hal::task::block_on(async {
        let mut eth = eth::AsyncEth::wrap(&mut eth, sys_loop.clone(), timer_service)?;

        info!("Starting eth...");

        eth.start().await?;

        info!("Waiting for DHCP lease...");

        eth.wait_netif_up().await?;

        let ip_info = eth.eth().netif().get_ip_info()?;

        info!("Eth DHCP info: {:?}", ip_info);

        Result::<_, EspError>::Ok(ip_info)
    })?;

    status.connected = true;

    display
        .clear(Rgb565::GREEN)
        .map_err(|_| anyhow!("clear display"))?;

    get_styled_text(&["Connected", &ip_info.ip.to_string()].join("\n"), 100, 50)
        .draw(&mut display)
        .map_err(|_| anyhow!("draw text"))?;

    // ping(ip_info.subnet.gateway)?;

    let mut timer = TimerDriver::new(peripherals.timer00, &TimerConfig::new())?;

    let udp_socket = Arc::new(Mutex::new(UdpSocket::bind(SocketAddrV4::new(
        Ipv4Addr::new(0, 0, 0, 0),
        3333,
    ))?));

    {
        let udp_socket = udp_socket.lock().unwrap();
        info!("Socket bound to {:?}", udp_socket.local_addr()?);
    }

    let mut i = 4;
    {
        let udp_socket = udp_socket.clone();
        thread::Builder::new().stack_size(8 * 1024).spawn(move || {
            let tcp_socket =
                TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 12345)).unwrap();

            loop {
                match tcp_socket.accept() {
                    Ok((mut stream, addr)) => {
                        info!("Connection from: {:?}", addr);

                        let mut buf = [0; 10];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) => {
                                    info!("Connection closed");
                                    break;
                                }
                                Ok(2) => {
                                    let port = u16::from_be_bytes([buf[0], buf[1]]);
                                    let udp_target = SocketAddr::new(addr.ip(), port);
                                    info!("Connecting to UDP socket at: {}", udp_target);
                                    udp_socket.lock().unwrap().connect(udp_target).unwrap();
                                }
                                Ok(n) => {
                                    info!("Received TCP command: {:?}", buf[0]);
                                    match buf[0] {
                                        0 => {
                                            info!("Restarting...");
                                            restart();
                                        }
                                        1 => {
                                            info!("Attempting update...");
                                            let data =
                                                String::from_utf8(buf[1..n].to_vec()).unwrap();
                                            let update_url = UPDATE_BIN_URL.replace("TAG", &data);
                                            if let Ok(u) = Uri::try_from(update_url) {
                                                simple_download_and_update_firmware(u).unwrap();
                                            } else {
                                                log::warn!("Invalid URL to download firmware");
                                            }
                                            restart();
                                        }
                                        _ => {
                                            info!("Unknown command");
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!("Error receiving data: {:?}", e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Error accepting connection: {:?}", e);
                    }
                }
            }
        })?;
    }

    let status = Arc::new(Mutex::new(status));

    {
        let udp_socket = udp_socket.clone();
        let status = status.clone();
        thread::spawn(move || loop {
            thread::sleep(std::time::Duration::from_secs(1));
            let status = status.lock().unwrap();
            let status_bytes = bincode::serialize(&*status).unwrap();
            match udp_socket.lock().unwrap().send(&status_bytes) {
                Ok(_) => {}
                Err(e) => warn!("Error sending status: {:?}", e),
            }
            // let status_text = format!(
            //     "Version: {}.{}.{}\nConnected: {}\nIP: {}\nDisplay OK: {}\nHaptic OK: {}\nHeart OK: {}",
            //     status.version[0],
            //     status.version[1],
            //     status.version[2],
            //     status.connected,
            //     ip_info.ip,
            //     status.display_ok,
            //     status.haptic_ok,
            //     status.heart_ok
            // );

            // display
            //     .clear(Rgb565::BLACK)
            //     .map_err(|_| anyhow!("clear display"))
            //     .unwrap();

            // get_styled_text(&status_text, 100, 0)
            //     .draw(&mut display)
            //     .map_err(|_| anyhow!("draw text"))
            //     .unwrap();
        });
    }

    let mut pulse_sensor = pulse_sensor::PulseSensor::new();

    block_on(async {
        loop {
            timer.delay(timer.tick_hz() / SAMPLING_RATE_HZ).await?;

            let signal = adc.read_raw()?;
            pulse_sensor.read_next_sample(signal);
            pulse_sensor.process_latest_sample();

            if pulse_sensor.saw_start_of_beat() {
                let bpm = pulse_sensor.get_beats_per_minute();
                let ibi = pulse_sensor.get_inter_beat_interval_ms();
                let last_beat_time = pulse_sensor.get_last_beat_time();
                haptic.set_go(true)?;
                info!(
                    "BPM: {}, IBI: {}, Last Beat Time: {}",
                    bpm, ibi, last_beat_time
                );
            }

            samples[i..i + 2].copy_from_slice(&signal.to_be_bytes());
            i += 2;

            if i >= samples.len() {
                i = 4;
                samples[0..2].copy_from_slice(&pulse_sensor.get_beats_per_minute().to_be_bytes());
                samples[2..4]
                    .copy_from_slice(&pulse_sensor.get_inter_beat_interval_ms().to_be_bytes());
                match udp_socket.lock().unwrap().send(&samples) {
                    Ok(_) => (),
                    Err(e) => {
                        warn!("Error sending data: {:?}", e);
                        continue;
                    }
                }
            }
        }
    })
}

fn get_styled_text(text: &str, x: i32, y: i32) -> Text<'_, MonoTextStyle<Rgb565>> {
    let character_style = MonoTextStyle::new(&ascii::FONT_10X20, Rgb565::WHITE);

    // Create a new text style.
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .line_height(LineHeight::Percent(100))
        .build();

    // Create a text at position (20, 30) and draw it using the previously defined style.
    Text::with_text_style(text, Point::new(x, y), character_style, text_style)
}

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
