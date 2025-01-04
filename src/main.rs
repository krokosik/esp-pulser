use std::io::Read;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::{Arc, Mutex};
use std::{env, thread};

use anyhow::anyhow;
use display_interface_spi::SPIInterface;
use drv2605::{Drv2605, Effect};
use embedded_graphics::{mono_font::*, pixelcolor::Rgb565, prelude::*, text::*};
use esp_idf_svc::hal::modem::WifiModemPeripheral;
use esp_idf_svc::ipv4::{self, IpInfo};
use esp_idf_svc::ping;
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{AsyncWifi, AuthMethod, ClientConfiguration, Configuration, EspWifi};
use esp_idf_svc::{eventloop::EspSystemEventLoop, nvs::EspDefaultNvsPartition};
use log::{error, warn};
use mipidsi::{models::ST7789, options::*, Builder};

use esp_idf_svc::hal::{
    adc, delay, gpio::*, i2c, prelude::*, spi, task::*, timer::*, units::FromValueType,
};

use log::info;

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASS");

#[derive(Debug)]
struct Status {
    haptic: bool,
    display: bool,
    network: bool,
    adc: bool,
    ip_info: Option<IpInfo>,
    target_ip: Option<Ipv4Addr>,
    target_port: Option<u16>,
}

impl Status {
    fn is_ok(&self) -> bool {
        self.haptic && self.display && self.network && self.adc
    }
}

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
    ));

    let ip_info = match &wifi {
        Ok(wifi) => {
            let ip_info = wifi.wifi().sta_netif().get_ip_info()?;
            Some(ip_info)
        }
        Err(e) => {
            warn!("Error connecting to wifi: {:?}", e);
            None
        }
    };

    info!("Wifi DHCP info: {:?}", ip_info);

    let pins = peripherals.pins;

    info!("Starting ADC");
    // let adc_config = adc::oneshot::config::AdcChannelConfig {
    //     attenuation: adc::attenuation::DB_11,
    //     ..Default::default()
    // };
    // let adc_driver = adc::oneshot::AdcDriver::new(peripherals.adc2)?;
    // let mut adc = adc::oneshot::AdcChannelDriver::new(adc_driver, pins.gpio16, &adc_config)?;
    let mut bpm_input = PinDriver::input(pins.gpio16)?;
    bpm_input.set_pull(Pull::Down)?;

    info!("ADC started");

    // #################################### HAPTIC ###############################################
    let haptic: Result<Drv2605<i2c::I2cDriver<'_>>, anyhow::Error> = (|| {
        let i2c = peripherals.i2c0;
        let sda = pins.gpio3;
        let scl = pins.gpio4;

        let i2c_driver = i2c::I2cDriver::new(
            i2c,
            sda,
            scl,
            &i2c::config::Config::new().baudrate(400.kHz().into()),
        )?;

        let mut haptic = Drv2605::new(i2c_driver);

        haptic.init_open_loop_erm()?;
        haptic.set_single_effect(Effect::PulsingStrongOne100)?;

        Ok(haptic)
    })();

    // #################################### DISPLAY ###############################################
    let display: Result<mipidsi::Display<_, ST7789, _>, anyhow::Error> = (|| {
        let spi = peripherals.spi2;
        let dc = PinDriver::output(pins.gpio40)?;
        let sclk = pins.gpio36;
        let sdo = pins.gpio35; // mosi
        let sdi = pins.gpio37; // miso
        let tft_cs = pins.gpio42;
        // let eth_cs = pins.gpio10;
        let rst = PinDriver::output(pins.gpio41)?;

        let mut tft_power = PinDriver::output(pins.gpio7)?;
        let mut backlight = PinDriver::output(pins.gpio45)?;

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
            &spi::SpiDriverConfig::new().dma(spi::Dma::Auto(240 * 135 * 2 + 8)),
        )?;

        let tft_spi_device = spi::SpiDeviceDriver::new(spi_driver, Some(tft_cs), &config)?;

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

        // Create a text at position (20, 30) and draw it using the previously defined style.
        Text::with_text_style(
            "Test",
            Point::new(100, 30),
            CHARACTER_STYLE_WHITE,
            TEXT_STYLE,
        )
        .draw(&mut display)
        .map_err(|_| anyhow!("draw text"))?;

        Ok(display)
    })();

    let mut timer = TimerDriver::new(peripherals.timer00, &TimerConfig::new())?;

    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 3333))?;
    info!("Socket bound to {:?}", socket.local_addr()?);

    let mut status = Arc::new(Mutex::new(Status {
        haptic: true, //haptic.is_ok(),
        display: display.is_ok(),
        network: wifi.is_ok(),
        adc: true,
        ip_info,
        target_ip: None,
        target_port: None,
    }));

    {
        let status = status.clone();
        thread::spawn(move || {
            let socket =
                TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 3334)).unwrap();

            match socket.accept() {
                Ok((mut stream, addr)) => {
                    info!("Connection from: {:?}", addr);

                    let mut buf = [0; 1024];
                    loop {
                        match stream.read(&mut buf) {
                            Ok(0) => {
                                info!("Connection closed");
                                let mut status = status.lock().unwrap();
                                status.target_ip = None;
                                status.target_port = None;
                                break;
                            }
                            Ok(n) => {
                                let data = &buf[..n];
                                let data = std::str::from_utf8(data).unwrap();
                                info!("Received: {}", data);

                                if let Ok(addr) = data.parse::<SocketAddrV4>() {
                                    let mut status = status.lock().unwrap();
                                    status.target_ip = Some(*addr.ip());
                                    status.target_port = Some(addr.port());
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
        });
    }

    {
        if !status.lock().unwrap().is_ok() {
            error!("Not all systems are ready: {:?}", status);
            return Err(anyhow!("Not all systems are ready"));
        }
    }

    let mut haptic = haptic.unwrap();
    let mut display = display.unwrap();

    block_on(async {
        loop {
            bpm_input.wait_for_high().await?;

            haptic.set_go(true)?;

            // if let Ok(status) = status.try_lock() {
            //     if let Some(target_ip) = status.target_ip {
            //         if let Some(target_port) = status.target_port {
            //             let ip = target_ip;
            //             let port = target_port;

            //             match socket.send_to(&[1; 1], SocketAddrV4::new(ip, port)) {
            //                 Ok(_) => (),
            //                 Err(e) => {
            //                     warn!("Error sending data: {:?}", e);
            //                 }
            //             };
            //         }
            //     }
            // }

            timer.delay(timer.tick_hz() / 10).await?;

            haptic.set_go(false)?;
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

const CHARACTER_STYLE_WHITE: MonoTextStyle<'_, Rgb565> =
    MonoTextStyle::new(&ascii::FONT_10X20, Rgb565::WHITE);

const CHARACTER_STYLE_GREEN: MonoTextStyle<'_, Rgb565> =
    MonoTextStyle::new(&ascii::FONT_10X20, Rgb565::GREEN);

const CHARACTER_STYLE_RED: MonoTextStyle<'_, Rgb565> =
    MonoTextStyle::new(&ascii::FONT_10X20, Rgb565::RED);

const TEXT_STYLE: TextStyle = TextStyleBuilder::new()
    .alignment(Alignment::Center)
    .line_height(LineHeight::Percent(150))
    .build();
