use anyhow::{Ok, Result};
use esp_idf_svc::{
    eth::EspEth, eventloop::EspSystemEventLoop, hal::task::block_on, ipv4::IpInfo,
    timer::EspTaskTimerService,
};

#[macro_export]
macro_rules! i2c_init {
    ($i2c:expr, $sda:expr, $scl:expr) => {{
        i2c::I2cDriver::new(
            $i2c,
            $sda,
            $scl,
            &i2c::config::Config::new().baudrate(400.kHz().into()),
        )
    }};
}

#[macro_export]
macro_rules! spi_init {
    ($spi:expr, $sck:expr, $sdo:expr, $sdi:expr) => {{
        spi::SpiDriver::new(
            $spi,
            $sck,
            $sdo,
            Some($sdi),
            &spi::SpiDriverConfig::new().dma(spi::Dma::Auto(4096)),
        )
    }};
}

#[macro_export]
macro_rules! adc_init {
    ($adc:expr, $pin:expr) => {{
        adc::oneshot::AdcChannelDriver::new(
            adc::oneshot::AdcDriver::new($adc)?,
            $pin,
            &adc::oneshot::config::AdcChannelConfig {
                attenuation: adc::attenuation::DB_11,
                ..Default::default()
            },
        )
    }};
}

#[macro_export]
macro_rules! eth_init {
    ($spi_driver:expr, $eth_int:expr, $eth_cs:expr, $eth_rst:expr, $sys_loop:expr) => {{
        esp_idf_svc::eth::EspEth::wrap(esp_idf_svc::eth::EthDriver::new_spi(
            $spi_driver.clone(),
            $eth_int,
            Some($eth_cs),
            Some($eth_rst),
            esp_idf_svc::eth::SpiEthChipset::W5500,
            20_u32.MHz().into(),
            Some(&[0x98, 0x76, 0xB6, 0x12, 0xF9, 0x93]),
            None,
            $sys_loop.clone(),
        )?)
    }};
}

#[macro_export]
macro_rules! display_init {
    ($spi_driver:expr, $tft_cs_pin:expr, $dc_pin:expr, $tft_rst_pin:expr) => {{
        let dc = PinDriver::output($dc_pin);
        if let Err(e) = dc {
            return Err(anyhow!(e));
        }
        let dc = dc.unwrap();
        let rst = PinDriver::output($tft_rst_pin);
        if let Err(e) = rst {
            return Err(anyhow!(e));
        }
        let rst = rst.unwrap();

        let spi_device = spi::SpiDeviceDriver::new(
            $spi_driver.clone(),
            Some($tft_cs_pin),
            &spi::config::Config::new()
                .baudrate(26.MHz().into())
                .data_mode(spi::config::MODE_3),
        );
        if let Err(e) = spi_device {
            return Err(anyhow!(e));
        }
        let spi_device = spi_device.unwrap();
        Builder::new(ST7789, SPIInterface::new(spi_device, dc))
            .display_size(135, 240)
            .orientation(Orientation::new().rotate(Rotation::Deg90))
            .display_offset(52, 40)
            .invert_colors(ColorInversion::Inverted)
            .reset_pin(rst)
            .init(&mut delay::Ets)
            .map_err(|_| anyhow!("Failed to initialize display"))
    }};
}

pub fn connect_eth<T>(
    eth: &mut EspEth<T>,
    sys_loop: EspSystemEventLoop,
    timer_service: EspTaskTimerService,
) -> Result<IpInfo> {
    block_on(async {
        let mut eth_async = esp_idf_svc::eth::AsyncEth::wrap(eth, sys_loop.clone(), timer_service)?;

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
