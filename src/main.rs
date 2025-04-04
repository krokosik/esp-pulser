use std::io::Read;
use std::net::TcpListener;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread::{self};
use std::time::Duration;

use anyhow::{anyhow, Result};
use circ::Circ;
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};
use esp_idf_svc::hal::i2c::I2cDriver;
use esp_idf_svc::ipv4::IpInfo;
use esp_idf_svc::nvs::*;
use http::Uri;
use max3010x::Max3010x;

use embedded_hal_bus::i2c::MutexDevice;
use esp_idf_svc::hal::{prelude::*, reset::restart, task::block_on};

use esp_pulser::*;
use pulse_sensor::{MAX30102_NUM_SAMPLES, MAX30102_SAMPLE_RATE};
mod circ;
mod linreg;
mod ota;
mod pulse_sensor;
mod signal;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, serde::Serialize)]
struct Status {
    version: [u8; 3],
    connected: bool,
    display_ok: bool,
    haptic_ok: bool,
    heart_ok: bool,
    led_amplitude: u8,
    haptic_amplitude: u8,
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
            led_amplitude: 0,
            haptic_amplitude: 0,
        }
    }
}

#[derive(Debug, serde::Serialize)]
enum Packet {
    Status(Status),
    RawHeartRate(f32),
    Bpm(f32),
    HeartRate(f32),
    // Debug((f32, f32, f32)),
}

fn main() -> Result<()> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let nvs_default_partition: EspNvsPartition<NvsDefault> = EspDefaultNvsPartition::take()?;
    let nvs = EspNvs::new(nvs_default_partition, "heart_sensor", true)?;

    let mut status = Status::new();

    let led_amplitude = nvs.get_u8("led_amplitude")?.unwrap_or(35);
    let mut haptic_amplitude = nvs.get_u8("haptic_amp")?.unwrap_or(255);
    status.led_amplitude = led_amplitude;

    let peripherals = Peripherals::take()?;
    let sys_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
    let timer_service = esp_idf_svc::timer::EspTaskTimerService::new()?;

    let mut board = Board::new(peripherals, sys_loop.clone(), timer_service.clone());

    status.display_ok = board.display_driver.is_some();

    let status: Arc<Mutex<Status>> = Arc::new(Mutex::new(status));
    let ip_info = Arc::new(Mutex::new(None));
    let eth = Arc::new(Mutex::new(board.eth_driver));
    let i2c_device = Arc::new(board.i2c_driver);

    {
        let eth = eth.clone();
        let ip_info = ip_info.clone();
        let status = status.clone();
        thread::Builder::new()
            .stack_size(4 * 1024)
            .spawn(move || eth_reconnect_task(eth, ip_info, status))?;
    }

    let i2c_device_clone = i2c_device.clone();
    let mut haptic = (|| {
        let mut haptic = drv2605::Drv2605::new(MutexDevice::new(&i2c_device_clone));
        haptic.calibrate(drv2605::CalibrationParams {
            brake_factor: 2,
            loop_gain: 2,
            auto_cal_time: 4,
            overdrive_clamp_voltage: haptic_amplitude,
            rated_voltage: 234,
        })?;
        haptic.init_open_loop_erm()?;
        haptic.set_single_effect(drv2605::Effect::PulsingStrongOne100)?;
        anyhow::Ok(haptic)
    })()
    .ok();

    {
        let mut status = status.lock().unwrap();
        status.haptic_ok = haptic.is_some();
        status.haptic_amplitude = haptic_amplitude;
    }

    let udp_socket = Arc::new(Mutex::new(UdpSocket::bind(SocketAddrV4::new(
        Ipv4Addr::new(0, 0, 0, 0),
        3333,
    ))?));

    {
        let udp_socket = udp_socket.clone();
        let status = status.clone();
        thread::Builder::new()
            .stack_size(8 * 1024)
            .spawn(move || tcp_receiver_task(udp_socket, status, nvs))?;
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

    {
        let udp_socket = udp_socket.clone();
        let status = status.clone();
        let ip_info = ip_info.clone();

        thread::Builder::new()
            .stack_size(8 * 1024)
            .spawn(move || status_log_thread(udp_socket, board.display_driver, status, ip_info))?;
    }

    let samples = Arc::new(Mutex::new(Circ::<f32, MAX30102_NUM_SAMPLES>::new(0.0)));
    let mut heart_data_channel = pulse_sensor::Max3012SampleData::new();
    let data_to_send = Arc::new(Mutex::new(0));

    {
        let samples = samples.clone();
        let udp_socket = udp_socket.clone();
        let i2c_device = i2c_device.clone();
        let status = status.clone();
        let data_to_send = data_to_send.clone();
        thread::Builder::new().stack_size(4 * 1024).spawn(move || {
            match heart_sensing_task(
                samples,
                udp_socket,
                i2c_device,
                status.clone(),
                data_to_send,
            ) {
                Ok(_) => (),
                Err(e) => log::error!("Error in heart sensing task: {:?}", e),
            }
            status.lock().unwrap().heart_ok = false;
        })?;
    }

    std::thread::sleep(Duration::from_millis(400));

    let mut beat_triggered = false;

    loop {
        std::thread::sleep(Duration::from_millis(10));
        {
            let samples = samples.lock().unwrap();

            heart_data_channel.update_from_samples(samples.iter());
        }

        heart_data_channel.process_signal();

        {
            let mut data_to_send = data_to_send.lock().unwrap();
            if *data_to_send > 0 {
                *data_to_send = 0;
                send_via_udp(
                    udp_socket.clone(),
                    status.clone(),
                    &Packet::HeartRate(heart_data_channel.ac[MAX30102_NUM_SAMPLES - 1]),
                );
                // let samples = samples.lock().unwrap();
                // send_via_udp(
                //     udp_socket.clone(),
                //     status.clone(),
                //     &Packet::Debug((
                //         samples.iter().last().unwrap_or_default(),
                //         heart_data_channel.ac[MAX30102_NUM_SAMPLES - 1],
                //         heart_data_channel.heart_rate_bpm.unwrap_or_default(),
                //     )),
                // );
            }
        }

        if let Some(last_heartbeat) = heart_data_channel.heartbeats.last() {
            let last_heartbeat_idx = last_heartbeat.low_idx;

            if last_heartbeat_idx > MAX30102_NUM_SAMPLES - (MAX30102_SAMPLE_RATE.0 / 2) as usize // wait at least half a second (BPM 120)
                && heart_data_channel.heart_rate_bpm.is_some()
            {
                if !beat_triggered {
                    beat_triggered = true;
                    if let Some(haptic) = haptic.as_mut() {
                        haptic.set_go(true)?;
                    }
                }
            } else {
                beat_triggered = false;
            }
        } else {
            beat_triggered = false;
        }

        {
            let status = status.lock().unwrap();
            if status.haptic_amplitude != haptic_amplitude {
                haptic_amplitude = status.haptic_amplitude;
                if let Some(haptic) = haptic.as_mut() {
                    haptic.calibrate(drv2605::CalibrationParams {
                        brake_factor: 2,
                        loop_gain: 2,
                        auto_cal_time: 4,
                        overdrive_clamp_voltage: haptic_amplitude,
                        rated_voltage: 234,
                    })?;
                    haptic.init_open_loop_erm()?;
                    haptic.set_single_effect(drv2605::Effect::PulsingStrongOne100)?;
                }
            }
        }

        send_via_udp(
            udp_socket.clone(),
            status.clone(),
            &Packet::Bpm(heart_data_channel.heart_rate_bpm.unwrap_or_default()),
        );
    }
}

fn heart_sensing_task(
    samples: Arc<Mutex<Circ<f32, MAX30102_NUM_SAMPLES>>>,
    udp_socket: Arc<Mutex<UdpSocket>>,
    i2c_device: Arc<Mutex<I2cDriver>>,
    status: Arc<Mutex<Status>>,
    data_to_send: Arc<Mutex<u8>>,
) -> anyhow::Result<()> {
    let heart = Max3010x::new_max30101(MutexDevice::new(&*i2c_device));
    let mut led_amplitude;

    {
        led_amplitude = status.lock().unwrap().led_amplitude;
    }

    // Fs = 25 Hz
    let mut heart = heart.into_multi_led()?;
    heart.set_led_time_slots([
        max3010x::TimeSlot::Led3,
        max3010x::TimeSlot::Disabled,
        max3010x::TimeSlot::Disabled,
        max3010x::TimeSlot::Disabled,
    ])?;
    heart.set_sample_averaging(max3010x::SampleAveraging::Sa16)?;
    heart.set_sampling_rate(max3010x::SamplingRate::Sps800)?;
    heart.set_pulse_amplitude(max3010x::Led::Led3, led_amplitude)?;
    heart.set_pulse_width(max3010x::LedPulseWidth::Pw411)?;
    heart.enable_fifo_rollover()?;
    heart.clear_fifo()?;
    let mut data = [0; 10];
    let interval = Duration::from_micros(1_000_000 / MAX30102_SAMPLE_RATE.0 as u64);

    {
        status.lock().unwrap().heart_ok = true;
    }

    log::info!("Starting heart rate sensing...");

    loop {
        let now = std::time::Instant::now();

        match heart.read_fifo(&mut data) {
            Ok(samples_read) if samples_read > 0 => {
                let sample = data[0] as f32;
                {
                    *data_to_send.lock().unwrap() = 1;
                }
                {
                    samples.lock().unwrap().add(data[0] as f32);
                }
                send_via_udp(
                    udp_socket.clone(),
                    status.clone(),
                    &Packet::RawHeartRate(sample),
                );
            }
            Ok(_) => (),
            Err(e) => log::error!("Error reading FIFO: {:?}", e),
        }

        {
            let status = status.lock().unwrap();
            if status.led_amplitude != led_amplitude {
                led_amplitude = status.led_amplitude;
                heart
                    .set_pulse_amplitude(max3010x::Led::Led3, led_amplitude)
                    .map_err(|_| anyhow!("Heartbeat I2C disconnected"))?;
            }
        }

        std::thread::sleep(interval.checked_sub(now.elapsed()).unwrap_or_default());
    }
}

fn status_log_thread(
    udp_socket: Arc<Mutex<UdpSocket>>,
    mut display_driver: Option<TftDisplay<'_>>,
    status: Arc<Mutex<Status>>,
    ip_info: Arc<Mutex<Option<IpInfo>>>,
) {
    let mut displayed_ip_info = None::<IpInfo>;
    let mut first = true;

    loop {
        thread::sleep(std::time::Duration::from_secs(1));
        let status_clone = status.lock().unwrap().clone();
        send_via_udp(
            udp_socket.clone(),
            status.clone(),
            &Packet::Status(status_clone),
        );

        if display_driver.as_ref().is_some() {
            let ip_info = ip_info.lock().unwrap();
            if first || *ip_info != displayed_ip_info {
                displayed_ip_info = *ip_info;
            } else {
                continue;
            }

            let display = display_driver.as_mut().unwrap();

            if let Some(displayed_ip_info) = displayed_ip_info {
                first = false;
                display.clear(Rgb565::GREEN).unwrap();

                get_styled_text(
                    &["Connected", &displayed_ip_info.ip.to_string()].join("\n"),
                    100,
                    50,
                )
                .draw(display)
                .unwrap();
            } else {
                display.clear(Rgb565::RED).unwrap();

                get_styled_text("Unconnected", 100, 50)
                    .draw(display)
                    .unwrap();
            }
        } else {
            log::warn!("Display driver not initialized");
            status.lock().unwrap().display_ok = false;
        }
    }
}

fn eth_reconnect_task(
    eth: Arc<Mutex<Option<EthPeripheral>>>,
    ip_info: Arc<Mutex<Option<IpInfo>>>,
    status: Arc<Mutex<Status>>,
) {
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
                        status.lock().unwrap().connected = true;
                    }
                    Err(e) => {
                        if error_count < 3 {
                            error_count += 1;
                            log::warn!("Error connecting eth: {:?}", e);
                        }
                        let mut ip_info = ip_info.lock().unwrap();
                        *ip_info = None;
                        status.lock().unwrap().connected = false;
                    }
                }
            }
        }
    }
}

fn tcp_receiver_task(
    udp_socket: Arc<Mutex<UdpSocket>>,
    status: Arc<Mutex<Status>>,
    nvs: EspNvs<NvsDefault>,
) {
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
                                2 => {
                                    let led_amplitude = buf[1];
                                    log::info!("Setting LED amplitude to: {}", led_amplitude);
                                    status.lock().unwrap().led_amplitude = led_amplitude;
                                    nvs.set_u8("led_amplitude", led_amplitude).unwrap();
                                }
                                3 => {
                                    let port = u16::from_be_bytes([buf[1], buf[2]]);
                                    let udp_target = SocketAddr::new(addr.ip(), port);
                                    log::info!("Connecting to UDP socket at: {}", udp_target);
                                    udp_socket.lock().unwrap().connect(udp_target).unwrap();
                                }
                                4 => {
                                    let haptic_amplitude = buf[1];
                                    log::info!("Setting Motor amplitude to: {}", haptic_amplitude);
                                    status.lock().unwrap().haptic_amplitude = haptic_amplitude;
                                    nvs.set_u8("haptic_amp", haptic_amplitude).unwrap();
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

fn send_via_udp(udp_socket: Arc<Mutex<UdpSocket>>, status: Arc<Mutex<Status>>, packet: &Packet) {
    if status.lock().unwrap().connected {
        match udp_socket
            .lock()
            .unwrap()
            .send(&bincode::serialize(packet).unwrap())
        {
            Ok(_) => (),
            Err(e) => log::warn!("Error sending data: {:?}", e),
        };
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
