use std::io::Read;
use std::net::TcpListener;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::anyhow;
use display_interface_spi::SPIInterface;
use drv2605::{Drv2605, Effect};
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};
use esp_idf_svc::eth;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::sys::EspError;
use esp_idf_svc::timer::EspTaskTimerService;
use log::warn;
use mipidsi::{models::ST7789, options::*, Builder};

use esp_idf_svc::hal::{
    adc, delay, gpio::*, i2c, prelude::*, spi, task::*, timer::*, units::FromValueType,
};

use log::info;

mod pulse_sensor;

const SAMPLING_RATE_HZ: u64 = 500;

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

fn main() -> Result<(), anyhow::Error> {
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
        thread::spawn(move || {
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
                                    info!("Received {} bytes", n);
                                    info!("Data: {:?}", &buf[..n]);
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
        });
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
                Ok(_) => info!("Status sent"),
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
