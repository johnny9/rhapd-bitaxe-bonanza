#![no_std]
#![no_main]
#![feature(trivial_bounds)]

use defmt::unwrap;
use panic_rtt_target as _;

esp_bootloader_esp_idf::esp_app_desc!();

use embassy_executor::Spawner;
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use esp_hal::{
    analog::adc,
    clock::CpuClock,
    gpio::{Input, InputConfig, Level, Output, OutputConfig},
    i2c, ledc,
    time::Rate,
    timer::systimer::SystemTimer,
};
use static_cell::StaticCell;

mod board_safety;
mod bridge;
mod bridge_protocol;
mod control;
mod fan_logic;
mod flash_logic;
mod rhap;
mod swd;
mod uart;

pub type AsicUart = esp_hal::peripherals::UART1<'static>;
pub type I2cDriver = i2c::master::I2c<'static, esp_hal::Async>;
pub type UsbDriver = esp_hal::otg_fs::asynch::Driver<'static>;
pub type UsbDevice = embassy_usb::UsbDevice<'static, UsbDriver>;

const VERSION: u16 = 0x0100;

static MANUFACTURER: &str = "OSMU";
static PRODUCT: &str = "Bitaxe Bonanza RHAP-D";
const BRIDGE_DATA_BAUDRATE: u32 = bridge_protocol::DATA_BAUDRATE;

/// Returns this device's serial number, the hex-encoded MAC address.
fn serial_number() -> &'static str {
    let mac_address = esp_hal::efuse::Efuse::mac_address();
    static SERIAL_NUMBER_BUF: StaticCell<[u8; 12]> = StaticCell::new();
    let buf = SERIAL_NUMBER_BUF.init([0; 12]);
    hex::encode_to_slice(mac_address, &mut buf[..]).unwrap();
    unsafe { core::str::from_utf8_unchecked(buf) }
}

static USB_EP_OUT_BUFFER: StaticCell<[u8; 1024]> = StaticCell::new();

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    rtt_target::rtt_init_defmt!();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    let timer0 = SystemTimer::new(p.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    let usb = esp_hal::otg_fs::Usb::new(p.USB0, p.GPIO20, p.GPIO19);
    let usb_config = esp_hal::otg_fs::asynch::Config::default();
    let usb_driver = UsbDriver::new(usb, USB_EP_OUT_BUFFER.init([0u8; 1024]), usb_config);

    //esp_alloc::heap_allocator!(size: 72 * 1024);

    let usb_config = {
        let mut config = embassy_usb::Config::new(0x1209, 0x6102);
        config.device_release = VERSION;
        config.manufacturer = Some(MANUFACTURER);
        config.product = Some(PRODUCT);
        config.serial_number = Some(serial_number());
        config.max_power = 100;
        config.max_packet_size_0 = 64;
        config.device_class = 0xef;
        config.device_sub_class = 0x02;
        config.device_protocol = 0x01;
        config.composite_with_iads = true;
        config
    };

    let mut builder = {
        static CONFIG_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static BOS_DESCRIPTOR: StaticCell<[u8; 256]> = StaticCell::new();
        static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();

        embassy_usb::Builder::new(
            usb_driver,
            usb_config,
            CONFIG_DESCRIPTOR.init([0; 256]),
            BOS_DESCRIPTOR.init([0; 256]),
            &mut [],
            CONTROL_BUF.init([0; 64]),
        )
    };

    let control_class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    let asic_uart_class = {
        static STATE: StaticCell<State> = StaticCell::new();
        let state = STATE.init(State::new());
        CdcAcmClass::new(&mut builder, state, 64)
    };

    let asic_uart = {
        let config = esp_hal::uart::Config::default()
            .with_baudrate(BRIDGE_DATA_BAUDRATE)
            .with_rx(esp_hal::uart::RxConfig::default().with_fifo_full_threshold(64));
        esp_hal::uart::Uart::new(p.UART1, config)
            .unwrap()
            .with_rx(p.GPIO18)
            .with_tx(p.GPIO17)
            .into_async()
    };

    let bridge_uart = esp_hal::uart::Uart::new(
        p.UART0,
        esp_hal::uart::Config::default().with_baudrate(115200),
    )
    .unwrap()
    .with_rx(p.GPIO44)
    .with_tx(p.GPIO43)
    .into_async();
    let bridge = bridge::initialize(bridge_uart);

    let i2c = {
        let config = esp_hal::i2c::master::Config::default();
        let sda = p.GPIO47;
        let scl = p.GPIO48;
        i2c::master::I2c::new(p.I2C0, config)
            .unwrap()
            .with_sda(sda)
            .with_scl(scl)
            .into_async()
    };

    control::gpio::initialize(control::gpio::Pins {
        asic_reset: Output::new(p.GPIO1, Level::Low, OutputConfig::default()),
        asic_trip: Input::new(
            p.GPIO2,
            InputConfig::default().with_pull(esp_hal::gpio::Pull::Down),
        ),
        five_volt_enable: Output::new(p.GPIO21, Level::Low, OutputConfig::default()),
        safety: board_safety::BoardSafety::default(),
        vr_en: Output::new(p.GPIO10, Level::Low, OutputConfig::default()),
        vr_pgood: Input::new(p.GPIO11, InputConfig::default()),
    });

    let mut adc_config = adc::AdcConfig::default();
    let domain1 =
        adc_config.enable_pin_with_cal::<_, adc::AdcCalCurve<_>>(p.GPIO5, adc::Attenuation::_11dB);
    let domain2 =
        adc_config.enable_pin_with_cal::<_, adc::AdcCalCurve<_>>(p.GPIO4, adc::Attenuation::_11dB);
    let domain3 =
        adc_config.enable_pin_with_cal::<_, adc::AdcCalCurve<_>>(p.GPIO3, adc::Attenuation::_11dB);
    let adc_pins = control::adc::Pins {
        adc: adc::Adc::new(p.ADC1, adc_config),
        domain1,
        domain2,
        domain3,
    };

    // 1004x: fan wiring matches Proto, directly on the ESP32-S3.
    let (fan_pwm, fan_pins) = {
        use ledc::{
            channel::{self, ChannelIFace},
            timer::{self, TimerIFace},
            LowSpeed,
        };
        static FAN_PWM_TIMER: StaticCell<ledc::timer::Timer<'static, LowSpeed>> = StaticCell::new();
        let mut ledc = ledc::Ledc::new(p.LEDC);
        ledc.set_global_slow_clock(ledc::LSGlobalClkSource::APBClk);
        let mut timer0 = ledc.timer::<LowSpeed>(timer::Number::Timer0);
        timer0
            .configure(timer::config::Config {
                duty: timer::config::Duty::Duty8Bit,
                clock_source: timer::LSClockSource::APBClk,
                frequency: Rate::from_khz(25),
            })
            .unwrap();
        let timer0 = FAN_PWM_TIMER.init(timer0);
        let mut pwm = ledc.channel(channel::Number::Channel0, p.GPIO8);
        pwm.configure(channel::config::Config {
            timer: timer0,
            duty_pct: 0, // active-low full speed at boot
            pin_config: channel::config::PinConfig::PushPull,
        })
        .unwrap();
        let tach = Input::new(
            p.GPIO9,
            InputConfig::default().with_pull(esp_hal::gpio::Pull::Up),
        );
        (pwm, control::fan::Pins { tach })
    };

    let programmer = control::programmer::Programmer::new(swd::Swd::new(
        esp_hal::gpio::Flex::new(p.GPIO14),
        esp_hal::gpio::Flex::new(p.GPIO15),
    ));

    unwrap!(spawner.spawn(usb_task(builder.build())));
    unwrap!(spawner.spawn(control::gpio::trip_task()));
    unwrap!(spawner.spawn(bridge::monitor_task(bridge)));
    unwrap!(spawner.spawn(control::fan::pwm_task(fan_pwm)));
    unwrap!(spawner.spawn(control::usb_task(
        control_class,
        i2c,
        adc_pins,
        fan_pins,
        programmer,
        bridge
    )));
    unwrap!(spawner.spawn(uart::usb_task(asic_uart_class, asic_uart)));
}

#[embassy_executor::task]
async fn usb_task(mut usb: UsbDevice) -> ! {
    usb.run().await
}
