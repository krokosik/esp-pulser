use anyhow::anyhow;
use display_interface_spi::SPIInterface;
use drv2605::{Drv2605, Effect};
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};
use mipidsi::{models::ST7789, options::*, Builder};

use esp_idf_svc::hal::{delay, gpio::*, i2c, prelude::*, spi, units::FromValueType};

use log::info;

fn main() -> Result<(), anyhow::Error> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let pins = peripherals.pins;

    let spi = peripherals.spi2;
    let dc = PinDriver::output(pins.gpio40)?;
    let sclk = pins.gpio36;
    let sdo = pins.gpio35; // mosi
    let sdi = pins.gpio37; // miso
    let tft_cs = pins.gpio42;
    // let eth_cs = pins.gpio10;
    let rst = PinDriver::output(pins.gpio41)?;

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
        &spi::SpiDriverConfig::new().dma(spi::Dma::Auto(240 * 135 * 2 + 8)),
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
        .reset_pin(rst)
        .init(&mut delay)
        .map_err(|_| anyhow!("display init"))?;

    info!("Display initialized");

    display
        .clear(Rgb565::RED)
        .map_err(|_| anyhow!("clear display"))?;

    info!("Display cleared");

    let character_style = MonoTextStyle::new(&ascii::FONT_10X20, Rgb565::WHITE);

    // Create a new text style.
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .line_height(LineHeight::Percent(150))
        .build();

    // Create a text at position (20, 30) and draw it using the previously defined style.
    Text::with_text_style("Test", Point::new(100, 30), character_style, text_style)
        .draw(&mut display)
        .map_err(|_| anyhow!("draw text"))?;

    info!("Text drawn");

    haptic.set_go(true)?;

    loop {
        delay::FreeRtos::delay_ms(1000);
    }
}
