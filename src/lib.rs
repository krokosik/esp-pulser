#![feature(type_alias_impl_trait)]
use std::sync::{Arc, Mutex};

use anyhow::anyhow;
use display_interface_spi::SPIInterface;
use esp_idf_svc::{
    eth::{AsyncEth, EspEth, EthDriver, SpiEth, SpiEthChipset},
    eventloop::EspSystemEventLoop,
    hal::{
        delay,
        gpio::{Input, InputPin, Output, OutputPin, PinDriver, Pull},
        i2c,
        peripheral::Peripheral,
        prelude::*,
        spi,
        task::block_on,
        timer::{TimerConfig, TimerDriver},
    },
    ipv4::IpInfo,
    timer::EspTaskTimerService,
};
use mipidsi::{
    models::ST7789,
    options::{ColorInversion, Orientation, Rotation},
    Builder,
};

pub struct Board<'d> {
    pub i2c_driver: Mutex<i2c::I2cDriver<'d>>,
    pub spi_driver: Option<Arc<spi::SpiDriver<'d>>>,
    // pub adc_driver: Option<adc::oneshot::AdcChannelDriver<'d>>,
    pub eth_driver: Option<EthPeripheral<'d>>,
    pub display_driver: Option<TftDisplay<'d>>,
    pub sys_loop: EspSystemEventLoop,
    pub timer_service: EspTaskTimerService,
    pub d0_btn: BtnPin0<'d>,
    pub d1_btn: BtnPin1<'d>,
    pub d2_btn: BtnPin2<'d>,
    pub backlight: DisplayBacklightPin<'d>,
    pub timer: TimerDriver<'d>,
    _i2c_power: I2CPowerPin<'d>,
}

pub type EthPeripheral<'d> = AsyncEth<EspEth<'d, SpiEth<Arc<spi::SpiDriver<'d>>>>>;
pub type BtnPin0<'d> = PinDriver<'d, impl InputPin, Input>;
pub type BtnPin1<'d> = PinDriver<'d, impl InputPin, Input>;
pub type BtnPin2<'d> = PinDriver<'d, impl InputPin, Input>;
pub type I2CPowerPin<'d> = PinDriver<'d, impl OutputPin, Output>;
pub type DisplayBacklightPin<'d> = PinDriver<'d, impl OutputPin, Output>;

pub type TftDisplay<'d> = mipidsi::Display<
    display_interface_spi::SPIInterface<
        esp_idf_svc::hal::spi::SpiDeviceDriver<
            'd,
            std::sync::Arc<esp_idf_svc::hal::spi::SpiDriver<'d>>,
        >,
        PinDriver<'d, impl OutputPin, Output>,
    >,
    mipidsi::models::ST7789,
    PinDriver<'d, impl OutputPin, Output>,
>;

impl<'d> Board<'d> {
    pub fn new(
        peripherals: Peripherals,
        sys_loop: EspSystemEventLoop,
        timer_service: EspTaskTimerService,
    ) -> Self {
        let pins = peripherals.pins;

        // Enable the I2C TFT Power pin
        let mut i2c_power = PinDriver::output(pins.gpio7).unwrap();
        i2c_power.set_high().unwrap();

        let i2c_driver = Self::init_i2c(peripherals.i2c0, pins.gpio3, pins.gpio4).unwrap();
        let spi_driver =
            Self::init_spi(peripherals.spi2, pins.gpio36, pins.gpio35, pins.gpio37).ok();

        let eth_driver = if let Some(spi_driver) = &spi_driver {
            Self::init_eth(
                spi_driver.clone(),
                pins.gpio13,
                pins.gpio10,
                pins.gpio12,
                sys_loop.clone(),
                timer_service.clone(),
            )
            .ok()
        } else {
            None
        };
        let display_driver = if let Some(spi_driver) = &spi_driver {
            Self::init_display(spi_driver.clone(), pins.gpio42, pins.gpio40, pins.gpio41).ok()
        } else {
            None
        };

        let mut d0_btn = PinDriver::input(pins.gpio0).unwrap();
        let mut d1_btn = PinDriver::input(pins.gpio1).unwrap();
        let mut d2_btn = PinDriver::input(pins.gpio2).unwrap();
        d0_btn.set_pull(Pull::Down).unwrap();
        d1_btn.set_pull(Pull::Down).unwrap();
        d2_btn.set_pull(Pull::Down).unwrap();

        let backlight = PinDriver::output(pins.gpio45).unwrap();

        let timer = TimerDriver::new(peripherals.timer00, &TimerConfig::new()).unwrap();

        Board {
            sys_loop,
            timer_service,
            i2c_driver: Mutex::new(i2c_driver),
            spi_driver,
            // adc_driver,
            eth_driver,
            display_driver,
            d0_btn,
            d1_btn,
            d2_btn,
            backlight,
            timer,
            _i2c_power: i2c_power,
        }
    }

    fn init_i2c<I2C: i2c::I2c>(
        i2c: impl Peripheral<P = I2C> + 'd,
        sda: impl Peripheral<P = impl InputPin + OutputPin> + 'd,
        scl: impl Peripheral<P = impl InputPin + OutputPin> + 'd,
    ) -> anyhow::Result<i2c::I2cDriver<'d>> {
        log::info!("Initializing I2C...");
        let res = Ok(i2c::I2cDriver::new(
            i2c,
            sda,
            scl,
            &i2c::config::Config::new().baudrate(400.kHz().into()),
        )?);
        log::info!("I2C initialized");
        res
    }

    fn init_spi<SPI: spi::SpiAnyPins>(
        spi: impl Peripheral<P = SPI> + 'd,
        sck: impl Peripheral<P = impl OutputPin> + 'd,
        sdo: impl Peripheral<P = impl OutputPin> + 'd,
        sdi: impl Peripheral<P = impl InputPin> + 'd,
    ) -> anyhow::Result<Arc<spi::SpiDriver<'d>>> {
        log::info!("Initializing SPI...");
        let res = Ok(Arc::new(spi::SpiDriver::new(
            spi,
            sck,
            sdo,
            Some(sdi),
            &spi::SpiDriverConfig::new().dma(spi::Dma::Auto(4096)),
        )?));
        log::info!("SPI initialized");
        res
    }

    // fn init_adc<ADC: adc::Adc>(
    //     adc: impl Peripheral<P = ADC> + 'd,
    //     adc_pin: impl Peripheral<P = impl gpio::ADCPin<Adc = ADC>> + 'd,
    // ) -> anyhow::Result<adc::oneshot::AdcChannelDriver<'d, ADC>> {
    //     let adc_driver = adc::oneshot::AdcDriver::new(adc)?;
    //     Ok(adc::oneshot::AdcChannelDriver::new(
    //         adc_driver,
    //         adc_pin,
    //         &adc::oneshot::config::AdcChannelConfig::default(),
    //     )?)
    // }

    fn init_eth(
        spi_driver: Arc<spi::SpiDriver<'d>>,
        eth_int: impl Peripheral<P = impl InputPin> + 'd,
        eth_cs: impl Peripheral<P = impl OutputPin> + 'd,
        eth_rst: impl Peripheral<P = impl OutputPin> + 'd,
        sys_loop: EspSystemEventLoop,
        timer_service: EspTaskTimerService,
    ) -> anyhow::Result<AsyncEth<EspEth<'d, SpiEth<Arc<spi::SpiDriver<'d>>>>>> {
        let eth = EspEth::wrap(EthDriver::new_spi(
            spi_driver,
            eth_int,
            Some(eth_cs),
            Some(eth_rst),
            SpiEthChipset::W5500,
            20_u32.MHz().into(),
            Some(&[0x98, 0x76, 0xB6, 0x12, 0xF9, 0x93]),
            None,
            sys_loop.clone(),
        )?)?;
        Ok(AsyncEth::wrap(eth, sys_loop, timer_service)?)
    }

    #[allow(clippy::type_complexity)]
    fn init_display(
        spi_driver: Arc<spi::SpiDriver<'d>>,
        tft_cs_pin: impl Peripheral<P = impl OutputPin> + 'd,
        dc_pin: impl Peripheral<P = impl OutputPin> + 'd,
        tft_rst_pin: impl Peripheral<P = impl OutputPin> + 'd,
    ) -> anyhow::Result<
        mipidsi::Display<
            display_interface_spi::SPIInterface<
                esp_idf_svc::hal::spi::SpiDeviceDriver<
                    'd,
                    std::sync::Arc<esp_idf_svc::hal::spi::SpiDriver<'d>>,
                >,
                PinDriver<'d, impl OutputPin, Output>,
            >,
            mipidsi::models::ST7789,
            PinDriver<'d, impl OutputPin, Output>,
        >,
    > {
        log::info!("Initializing display...");
        let dc = PinDriver::output(dc_pin)?;
        let rst = PinDriver::output(tft_rst_pin)?;

        let spi_device = spi::SpiDeviceDriver::new(
            spi_driver.clone(),
            Some(tft_cs_pin),
            &spi::config::Config::new()
                .baudrate(26.MHz().into())
                .data_mode(spi::config::MODE_3),
        )?;

        let res = Builder::new(ST7789, SPIInterface::new(spi_device, dc))
            .display_size(135, 240)
            .orientation(Orientation::new().rotate(Rotation::Deg90))
            .display_offset(52, 40)
            .invert_colors(ColorInversion::Inverted)
            .reset_pin(rst)
            .init(&mut delay::Ets)
            .map_err(|_| anyhow!("Failed to initialize display"));
        log::info!("Display initialized");
        res
    }
}

pub fn connect_eth<T>(eth_async: &mut AsyncEth<EspEth<T>>) -> anyhow::Result<IpInfo> {
    block_on(async {
        log::info!("Starting eth...");

        if !eth_async.eth().is_started()? {
            eth_async.start().await?;
        }

        log::info!("Waiting for DHCP lease...");

        eth_async.wait_netif_up().await?;

        let ip_info = eth_async.eth().netif().get_ip_info()?;

        log::info!("Eth DHCP info: {:?}", ip_info);

        Ok(ip_info)
    })
}
