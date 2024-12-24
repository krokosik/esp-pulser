use anyhow::{anyhow, Ok};
use display_interface_spi::SPIInterface;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::timer::EspTaskTimerService;
use mipidsi::{models::ST7789, options::Orientation, Builder};

use esp_idf_svc::eth;
use esp_idf_svc::hal::{delay, gpio::*, prelude::*, spi, units::FromValueType};
use esp_idf_svc::{ipv4, ping};

use log::{info, warn};

fn main() -> Result<(), anyhow::Error> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let sysloop = EspSystemEventLoop::take()?;
    let timer_service = EspTaskTimerService::new()?;
    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    let spi = peripherals.spi2;
    let dc = PinDriver::output(pins.gpio40)?;
    let sclk = pins.gpio36;
    let sdo = pins.gpio35; // mosi
    let sdi = pins.gpio37; // miso
    let tft_cs = pins.gpio42;
    let eth_cs = pins.gpio10;
    let rst = PinDriver::output(pins.gpio41)?;

    let mut tft_power = PinDriver::output(pins.gpio7)?;
    let mut backlight = PinDriver::output(pins.gpio45)?;

    info!("Pins initialized");

    tft_power.set_high()?;
    backlight.set_high()?;

    info!("Display power on");

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

    let mut eth = eth::EspEth::wrap(eth::EthDriver::new_spi(
        &spi_driver,
        pins.gpio27,
        Some(eth_cs),
        None::<AnyIOPin>,
        eth::SpiEthChipset::W5500,
        20_u32.MHz().into(),
        Some(&[0x02, 0x00, 0x00, 0x12, 0x34, 0x56]),
        None,
        sysloop.clone(),
    )?)?;

    info!("SPI initialized");

    let mut delay = delay::Ets;

    let di = SPIInterface::new(tft_spi_device, dc);
    let mut display = Builder::new(ST7789, di)
        .display_size(240, 135)
        .orientation(Orientation::new())
        .reset_pin(rst)
        .init(&mut delay)
        .map_err(|_| anyhow!("display init"))?;

    info!("Display initialized");

    display
        .clear(Rgb565::RED)
        .map_err(|_| anyhow!("clear display"))?;

    info!("Display cleared");

    // Wait for the Eth peripheral and network layer 3 to come up - in an async way because we can
    let ip_info = esp_idf_svc::hal::task::block_on(async {
        let mut eth = eth::AsyncEth::wrap(&mut eth, sysloop.clone(), timer_service)?;

        info!("Starting eth...");

        eth.start().await?;

        info!("Waiting for DHCP lease...");

        eth.wait_netif_up().await?;

        let ip_info = eth.eth().netif().get_ip_info()?;

        info!("Eth DHCP info: {:?}", ip_info);

        Ok(ip_info)
    })?;

    ping(ip_info.subnet.gateway)?;

    Ok(())
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
