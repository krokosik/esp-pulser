use anyhow::{anyhow, Ok};
use display_interface_spi::SPIInterface;
use embedded_graphics::{pixelcolor::Rgb565, prelude::*};
use mipidsi::{models::ST7789, Builder};

use esp_idf_svc::hal::{delay, gpio::*, prelude::*, spi, units::FromValueType};

fn main() -> Result<(), anyhow::Error> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;

    log::info!("Hello, world!");

    let spi = peripherals.spi2;
    let dc = PinDriver::output(peripherals.pins.gpio40)?;
    let sclk = peripherals.pins.gpio36;
    let sdo = peripherals.pins.gpio35; // mosi
    let sdi = peripherals.pins.gpio37; // miso
    let cs = peripherals.pins.gpio42;
    let rst = PinDriver::output(peripherals.pins.gpio41)?;

    let mut tft_power = PinDriver::output(peripherals.pins.gpio7)?;
    let mut backlight = PinDriver::output(peripherals.pins.gpio45)?;

    tft_power.set_high()?;
    backlight.set_high()?;

    let config = spi::config::Config::new()
        .baudrate(26.MHz().into())
        .data_mode(spi::config::MODE_3);

    let spi_device = spi::SpiDeviceDriver::new_single(
        spi,
        sclk,
        sdo,
        Some(sdi),
        Some(cs),
        &spi::SpiDriverConfig::new(),
        &config,
    )?;

    let mut delay = delay::Ets;

    let di = SPIInterface::new(spi_device, dc);
    let mut display = Builder::new(ST7789, di)
        .display_size(240, 135)
        .reset_pin(rst)
        .init(&mut delay)
        .map_err(|_| anyhow!("display init"))?;

    display
        .clear(Rgb565::RED)
        .map_err(|_| anyhow!("clear display"))?;

    Ok(())
}
