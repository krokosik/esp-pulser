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
            $spi_driver,
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
        Builder::new(
            ST7789,
            SPIInterface::new(
                spi::SpiDeviceDriver::new(
                    $spi_driver,
                    Some($tft_cs_pin),
                    &spi::config::Config::new()
                        .baudrate(26.MHz().into())
                        .data_mode(spi::config::MODE_3),
                )?,
                PinDriver::output($dc_pin)?,
            ),
        )
        .display_size(135, 240)
        .orientation(Orientation::new().rotate(Rotation::Deg90))
        .display_offset(52, 40)
        .invert_colors(ColorInversion::Inverted)
        .reset_pin(PinDriver::output($tft_rst_pin)?)
        .init(&mut delay::Ets)
        .map_err(|_| anyhow!("Failed to initialize display"))
    }};
}
