# esp_pulser

## Description

This is the firmware for an ESP32-S3 MCU responsible for collecting heartbeat data using an MAX30102 sensor. It is meant to be used with the companion [GUI app](https://github.com/krokosik/esp-pulser-gui).

### Features

- collecting heartbeat with 25 Hz sampling rate and 16 values of sample averaging
- signal processing with an algorithm
    - designed by @aromring for STM32 https://github.com/aromring/MAX30102_by_RF
    - ported to Rust by @andreyk0 https://github.com/andreyk0/cardiac-monitor
- haptic motor, controlled with the DRV2605L driver, triggered on heartbeat detection
- ethernet connectivity via the W5500 SPI chip, with automatic reconnection mechanism
- display for showing the status and assigned IP address (WIP)
    - the display is off by default to limit power consumption
    - enable it by holding the D2 button
- TCP listener socket for accepting remote commands from the GUI companion app
- UDP streaming of raw and processed heartbeat data, calculated bpm and device status
- OTA update functionality using the companion app

## Dev Containers
This repository offers Dev Containers supports for [VS Code Dev Containers](https://code.visualstudio.com/docs/remote/containers#_quick-start-open-an-existing-folder-in-a-container) and it is the recommended way of developing the code. There are a lot of build dependencies that are guaranteed to work inside of it. The linked website has all the necessary information for setting it up on your machine. The only additional requirement is flashing via [USBIP](https://github.com/dorssel/usbipd-win). Note that the device has to be attached to the WSL integration before the container is launched, as it is impossible to attach a device while it is already running.

Once the container is up, before flashing you also need to give permissions to access the usb port. The container is run without root privileges, so from the host shell, one needs to run:
```
docker exec -it -u 0 <CONTAINER_NAME> chmod a+rw /dev/ttyACM0
```

Once that is done, `cargo` can be used for:
- building `cargo build [--release]`
- flashing with log monitor `cargo run`
- other espflash functionalities [docs](https://github.com/esp-rs/espflash/tree/main/cargo-espflash) 
