use std::io::Read;
use std::net::TcpListener;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{anyhow, Result};
use circ::Circ;
use drv2605::CalibrationParams;
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};

use esp_idf_svc::ipv4::IpInfo;
use http::Uri;
use max3010x::Max3010x;

use embedded_hal_bus::i2c as i2c_bus;
use esp_idf_svc::hal::{prelude::*, reset::restart, task::block_on};

use esp_pulser::*;
use pulse_sensor::{MAX30102_NUM_SAMPLES, MAX30102_SAMPLE_RATE};
mod circ;
mod linreg;
mod ota;
mod pulse_sensor;
mod signal;

const VERSION: &str = env!("CARGO_PKG_VERSION");

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
    let sys_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let timer_service = esp_idf_svc::timer::EspTaskTimerService::new()?;

    let mut board = Board::new(peripherals, sys_loop.clone(), timer_service.clone());

    status.display_ok = board.display_driver.is_some();

    let ip_info = Arc::new(Mutex::new(None));
    let eth = Arc::new(Mutex::new(board.eth_driver));

    {
        let eth = eth.clone();
        let ip_info = ip_info.clone();
        thread::spawn(move || eth_reconnect_task(eth, ip_info));
    }

    let mut haptic = drv2605::Drv2605::new(i2c_bus::RefCellDevice::new(&board.i2c_driver));
    haptic.set_overdrive_time_offset(20)?;
    haptic.calibrate(CalibrationParams {
        brake_factor: 2,
        loop_gain: 2,
        auto_cal_time: 4,
        overdrive_clamp_voltage: 255,
        rated_voltage: 234,
    })?;
    haptic.init_open_loop_erm()?;
    haptic.set_single_effect(drv2605::Effect::SharpTickOne100)?;

    let heart = Max3010x::new_max30102(i2c_bus::RefCellDevice::new(&board.i2c_driver));

    // Fs = 25 Hz
    let mut heart = heart.into_heart_rate().unwrap();
    heart
        .set_sample_averaging(max3010x::SampleAveraging::Sa16)
        .unwrap();
    heart
        .set_sampling_rate(max3010x::SamplingRate::Sps400)
        .unwrap();
    heart.set_pulse_amplitude(max3010x::Led::Led1, 35).unwrap();
    heart
        .set_pulse_width(max3010x::LedPulseWidth::Pw411)
        .unwrap();
    heart.enable_fifo_rollover().unwrap();

    let udp_socket = Arc::new(Mutex::new(UdpSocket::bind(SocketAddrV4::new(
        Ipv4Addr::new(0, 0, 0, 0),
        3333,
    ))?));

    {
        let udp_socket = udp_socket.lock().unwrap();
        log::info!("Socket bound to {:?}", udp_socket.local_addr()?);
    }
    {
        let udp_socket = udp_socket.clone();
        thread::Builder::new()
            .stack_size(8 * 1024)
            .spawn(move || tcp_receiver_task(udp_socket))?;
    }

    thread::spawn(move || {
        block_on(async {
            loop {
                board.d1_btn.wait_for_high().await.unwrap();
                board.backlight.set_high().unwrap();
                board.d1_btn.wait_for_low().await.unwrap();
                board.backlight.set_low().unwrap();
            }
        })
    });

    if board.display_driver.is_some() {
        let mut display = board.display_driver.unwrap();
        let ip_info = ip_info.lock().unwrap();
        if ip_info.is_none() {
            display
                .clear(Rgb565::RED)
                .map_err(|_| anyhow!("clear display"))?;

            get_styled_text("Unconnected", 100, 50)
                .draw(&mut display)
                .map_err(|_| anyhow!("draw text"))?;
        } else {
            display
                .clear(Rgb565::GREEN)
                .map_err(|_| anyhow!("clear display"))?;

            get_styled_text(
                &["Connected", &ip_info.unwrap().ip.to_string()].join("\n"),
                100,
                50,
            )
            .draw(&mut display)
            .map_err(|_| anyhow!("draw text"))?;
        }
    }

    let status: Arc<Mutex<Status>> = Arc::new(Mutex::new(status));

    {
        let udp_socket = udp_socket.clone();
        let status = status.clone();

        thread::spawn(move || loop {
            thread::sleep(std::time::Duration::from_secs(1));
            let status = status.lock().unwrap();
            let status_bytes = bincode::serialize(&*status).unwrap();
            match udp_socket.lock().unwrap().send(&status_bytes) {
                Ok(_) => {}
                Err(e) => log::warn!("Error sending status: {:?}", e),
            }
        });
    }

    let mut i = 0;

    let mut data = [0; 1];
    let mut samples = Circ::<f32, MAX30102_NUM_SAMPLES>::new(0.0);
    let mut heart_data_bytes = [0_u8; (MAX30102_NUM_SAMPLES + 1) * 4];
    let mut heart_data = pulse_sensor::Max3012SampleData::new();

    heart.clear_fifo().unwrap();

    block_on(async {
        loop {
            board
                .timer
                .delay(board.timer.tick_hz() / MAX30102_SAMPLE_RATE.0 as u64)
                .await?;

            let samples_read = heart.read_fifo(&mut data).unwrap() as usize;

            if samples_read > 0 {
                samples.add(data[0] as f32);
            }

            if i >= MAX30102_NUM_SAMPLES {
                i = 0;
                heart_data.update_from_samples(&samples.data);
                log::info!("Heart rate: {:?}", heart_data.heart_rate_bpm);

                heart_data_bytes[0..4]
                    .copy_from_slice(&heart_data.heart_rate_bpm.unwrap_or(0.0).to_le_bytes());
                for (i, x) in heart_data.ac.iter().enumerate() {
                    heart_data_bytes[(i + 1) * 4..(i + 2) * 4].copy_from_slice(&x.to_le_bytes());
                }

                match udp_socket.lock().unwrap().send(&heart_data_bytes) {
                    Ok(_) => (),
                    Err(e) => {
                        log::warn!("Error sending data: {:?}", e);
                        continue;
                    }
                }
            }
            i += 1;
        }
    })
}

fn eth_reconnect_task(eth: Arc<Mutex<Option<EthPeripheral>>>, ip_info: Arc<Mutex<Option<IpInfo>>>) {
    let mut error_count = 0;
    loop {
        thread::sleep(std::time::Duration::from_secs(5));
        let mut eth = eth.lock().unwrap();

        if let Some(ref mut eth) = *eth {
            if let Ok(false) = eth.is_connected() {
                match connect_eth(eth) {
                    Ok(ip) => {
                        let mut ip_info = ip_info.lock().unwrap();
                        *ip_info = Some(ip);
                        error_count = 0;
                    }
                    Err(e) => {
                        if error_count < 3 {
                            error_count += 1;
                            log::warn!("Error connecting eth: {:?}", e);
                        }
                    }
                }
            }
        }
    }
}

fn tcp_receiver_task(udp_socket: Arc<Mutex<UdpSocket>>) {
    let tcp_socket =
        TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 12345)).unwrap();

    log::info!("Listening for GUI client...");

    loop {
        match tcp_socket.accept() {
            Ok((mut stream, addr)) => {
                log::info!("Connection from: {:?}", addr);

                let mut buf = [0; 10];
                loop {
                    match stream.read(&mut buf) {
                        Ok(0) => {
                            log::info!("Connection closed");
                            break;
                        }
                        Ok(2) => {
                            let port = u16::from_be_bytes([buf[0], buf[1]]);
                            let udp_target = SocketAddr::new(addr.ip(), port);
                            log::info!("Connecting to UDP socket at: {}", udp_target);
                            udp_socket.lock().unwrap().connect(udp_target).unwrap();
                        }
                        Ok(n) => {
                            log::info!("Received TCP command: {:?}", buf[0]);
                            match buf[0] {
                                0 => {
                                    log::info!("Restarting...");
                                    restart();
                                }
                                1 => {
                                    log::info!("Attempting update...");
                                    let data = String::from_utf8(buf[1..n].to_vec()).unwrap();
                                    let update_url = ota::UPDATE_BIN_URL.replace("TAG", &data);
                                    if let Ok(u) = Uri::try_from(update_url) {
                                        ota::simple_download_and_update_firmware(u).unwrap();
                                    } else {
                                        log::warn!("Invalid URL to download firmware");
                                    }
                                    restart();
                                }
                                _ => {
                                    log::info!("Unknown command");
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("Error receiving data: {:?}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                log::warn!("Error accepting connection: {:?}", e);
            }
        }
    }
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
