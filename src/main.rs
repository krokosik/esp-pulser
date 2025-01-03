use std::env;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

use anyhow::anyhow;
use display_interface_spi::SPIInterface;
// use drv2605::{Drv2605, Effect};
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};
use esp_idf_svc::hal::modem::WifiModemPeripheral;
use esp_idf_svc::ipv4;
use esp_idf_svc::ping;
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{AsyncWifi, AuthMethod, ClientConfiguration, Configuration, EspWifi};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use log::warn;
use mipidsi::{models::ST7789, options::*, Builder};

use esp_idf_svc::hal::{
    adc, delay, gpio::*, i2c, prelude::*, spi, task::*, timer::*, units::FromValueType,
};

use log::info;

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASS");

// mod pulse_sensor;

fn main() -> Result<(), anyhow::Error> {
    // It is necessary to call this function once. Otherwise some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let timer_service = EspTaskTimerService::new()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let wifi = block_on_send(connect_wifi(
        peripherals.modem,
        sys_loop.clone(),
        nvs,
        timer_service,
    ))?;

    let ip_info = wifi.wifi().sta_netif().get_ip_info()?;

    info!("Wifi DHCP info: {:?}", ip_info);

    let pins = peripherals.pins;

    let spi = peripherals.spi2;
    let dc = PinDriver::output(pins.gpio40)?;
    let sclk = pins.gpio36;
    let sdo = pins.gpio35; // mosi
    let sdi = pins.gpio37; // miso
    let tft_cs = pins.gpio42;
    // let eth_cs = pins.gpio10;
    let rst = PinDriver::output(pins.gpio41)?;

    info!("Starting ADC");
    let adc_config = adc::oneshot::config::AdcChannelConfig {
        attenuation: adc::attenuation::DB_11,
        ..Default::default()
    };
    let adc_driver = adc::oneshot::AdcDriver::new(peripherals.adc2)?;
    let mut adc = adc::oneshot::AdcChannelDriver::new(adc_driver, pins.gpio16, &adc_config)?;

    info!("ADC started");

    let mut samples = [0u8; 1024];

    // let i2c = peripherals.i2c0;
    // let sda = pins.gpio3;
    // let scl = pins.gpio4;

    // let i2c_driver = i2c::I2cDriver::new(
    //     i2c,
    //     sda,
    //     scl,
    //     &i2c::config::Config::new().baudrate(400.kHz().into()),
    // )?;

    let mut tft_power = PinDriver::output(pins.gpio7)?;
    let mut backlight = PinDriver::output(pins.gpio45)?;

    info!("Pins initialized");

    tft_power.set_high()?;
    backlight.set_high()?;

    info!("Display power on");

    // let mut haptic = Drv2605::new(i2c_driver);

    // info!("Haptic driver says: {:?}", haptic.init_open_loop_erm());

    // info!(
    //     "Haptic driver effect set to: {:?}",
    //     haptic.set_single_effect(Effect::Alert1000ms)
    // );

    // haptic.set_go(true)?;

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

    let mut timer = TimerDriver::new(peripherals.timer00, &TimerConfig::new())?;

    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 3333))?;
    socket.connect(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 45), 34254))?;
    info!("Socket bound to {:?}", socket.local_addr()?);

    block_on(async {
        loop {
            timer.delay(timer.tick_hz() / 500).await?;

            let res = adc.read()?;
            match socket.send(&res.to_be_bytes()) {
                Ok(_) => (),
                Err(e) => {
                    warn!("Error sending data: {:?}", e);
                    continue;
                }
            }
        }
    })
}

fn block_on_send<F>(f: F) -> F::Output
where
    F: core::future::Future + Send + 'static, // These constraints are why this additional example exists in the first place
{
    block_on(f)
}

async fn connect_wifi<M>(
    modem: M,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    timer_service: EspTaskTimerService,
) -> anyhow::Result<AsyncWifi<EspWifi<'static>>>
where
    M: WifiModemPeripheral + 'static,
{
    let mut wifi = AsyncWifi::wrap(
        EspWifi::new(modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
        timer_service,
    )?;

    let wifi_configuration: Configuration = Configuration::Client(ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        bssid: None,
        auth_method: AuthMethod::WPA2Personal,
        password: PASSWORD.try_into().unwrap(),
        channel: None,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_configuration)?;

    wifi.start().await?;
    info!("Wifi started");

    wifi.connect().await?;
    info!("Wifi connected");

    wifi.wait_netif_up().await?;
    info!("Wifi netif up");

    Ok(wifi)
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
