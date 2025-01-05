use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use anyhow::anyhow;
use display_interface_spi::SPIInterface;
use drv2605::{Drv2605, Effect};
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};
use esp_idf_svc::eth;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::ipv4;
use esp_idf_svc::ping;
use esp_idf_svc::sys::EspError;
use esp_idf_svc::timer::EspTaskTimerService;
use log::warn;
use mipidsi::{models::ST7789, options::*, Builder};

use esp_idf_svc::hal::{
    adc, delay, gpio::*, i2c, prelude::*, spi, task::*, timer::*, units::FromValueType,
};

use log::info;

fn main() -> Result<(), anyhow::Error> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

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

    info!("ADC started");

    let mut samples = [0u8; 2 * 500];

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

    display
        .clear(Rgb565::GREEN)
        .map_err(|_| anyhow!("clear display"))?;

    get_styled_text("Connected", 100, 50)
        .draw(&mut display)
        .map_err(|_| anyhow!("draw text"))?;

    ping(ip_info.subnet.gateway)?;

    let mut timer = TimerDriver::new(peripherals.timer00, &TimerConfig::new())?;

    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 3333))?;
    socket.connect(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 45), 34254))?;
    info!("Socket bound to {:?}", socket.local_addr()?);

    let mut i = 0;

    block_on(async {
        loop {
            timer.delay(timer.tick_hz() / 500).await?;

            samples[i..i + 2].copy_from_slice(&adc.read()?.to_be_bytes());
            i += 2;

            if i >= samples.len() {
                i = 0;
                match socket.send(&samples) {
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

fn ping(ip: ipv4::Ipv4Addr) -> Result<(), anyhow::Error> {
    info!("About to do some pings for {:?}", ip);

    let ping_summary = ping::EspPing::default().ping(ip, &Default::default())?;
    if ping_summary.transmitted != ping_summary.received {
        warn!("Pinging IP {} resulted in timeouts", ip);
    }

    info!("Pinging done");

    Ok(())
}

fn get_styled_text(text: &str, x: i32, y: i32) -> Text<'_, MonoTextStyle<Rgb565>> {
    let character_style = MonoTextStyle::new(&ascii::FONT_10X20, Rgb565::WHITE);

    // Create a new text style.
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .line_height(LineHeight::Percent(150))
        .build();

    // Create a text at position (20, 30) and draw it using the previously defined style.
    Text::with_text_style(text, Point::new(x, y), character_style, text_style)
}

// use esp_idf_svc::eth;
// use esp_idf_svc::eventloop::EspSystemEventLoop;
// use esp_idf_svc::hal::spi;
// use esp_idf_svc::hal::{prelude::Peripherals, units::FromValueType};
// use esp_idf_svc::log::EspLogger;
// use esp_idf_svc::sys::EspError;
// use esp_idf_svc::timer::EspTaskTimerService;
// use esp_idf_svc::{ipv4, ping};

// use log::{info, warn};

// fn main() -> anyhow::Result<()> {
//     esp_idf_svc::sys::link_patches();
//     EspLogger::initialize_default();

//     let peripherals = Peripherals::take()?;
//     let pins = peripherals.pins;
//     let sysloop = EspSystemEventLoop::take()?;
//     let timer_service = EspTaskTimerService::new()?;

//     let mut eth = eth::EspEth::wrap(eth::EthDriver::new_spi(
//         spi::SpiDriver::new(
//             peripherals.spi2,
//             pins.gpio36,
//             pins.gpio35,
//             Some(pins.gpio37),
//             &spi::SpiDriverConfig::new().dma(spi::Dma::Auto(4096)),
//         )?,
//         pins.gpio13,
//         Some(pins.gpio10),
//         Some(pins.gpio12),
//         // Replace with DM9051 or KSZ8851SNL if you have some of these variants
//         eth::SpiEthChipset::W5500,
//         20_u32.MHz().into(),
//         Some(&[0x98, 0x76, 0xB6, 0x12, 0xF9, 0x93]),
//         None,
//         sysloop.clone(),
//     )?)?;

//     // Wait for the Eth peripheral and network layer 3 to come up - in an async way because we can
//     let ip_info = esp_idf_svc::hal::task::block_on(async {
//         let mut eth = eth::AsyncEth::wrap(&mut eth, sysloop.clone(), timer_service)?;

//         info!("Starting eth...");

//         eth.start().await?;

//         info!("Waiting for DHCP lease...");

//         eth.wait_netif_up().await?;

//         let ip_info = eth.eth().netif().get_ip_info()?;

//         info!("Eth DHCP info: {:?}", ip_info);

//         Result::<_, EspError>::Ok(ip_info)
//     })?;

//     ping(ip_info.subnet.gateway)?;

//     Ok(())
// }

// fn ping(ip: ipv4::Ipv4Addr) -> Result<(), EspError> {
//     info!("About to do some pings for {:?}", ip);

//     let ping_summary = ping::EspPing::default().ping(ip, &Default::default())?;
//     if ping_summary.transmitted != ping_summary.received {
//         warn!("Pinging IP {} resulted in timeouts", ip);
//     }

//     info!("Pinging done");

//     Ok(())
// }
