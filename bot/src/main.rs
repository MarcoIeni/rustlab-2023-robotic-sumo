#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(incomplete_features)]

use core::str::FromStr;

use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Pull, Output};
use embassy_time::{Duration, Timer};
use fixed::traits::ToFixed;
use rp2040_panic_usb_boot as _;

use embassy_rp::adc::{Adc, Channel, Config as ConfigAdc, InterruptHandler as InterruptHandlerAdc};
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::USB;
use embassy_rp::pwm::{Config as PwmConfig, Pwm};
use embassy_rp::usb::{Driver, InterruptHandler as InterruptHandlerUsb};

use cyw43_pio::PioSpi;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Config as ConfigNet, Ipv4Address, Ipv4Cidr, Stack, StackResources};
use embassy_rp::peripherals::{DMA_CH0, PIN_23, PIN_25, PIO0};
use embassy_rp::pio::{InterruptHandler as InterruptHandlerPio, Pio};
use embedded_io_async::Write;
use heapless::Vec;
use rp2040_panic_usb_boot as _;
use static_cell::make_static;

const SOCKET_BUFFER_SIZE: usize = 128;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandlerUsb<USB>;
    ADC_IRQ_FIFO => InterruptHandlerAdc;
    PIO0_IRQ_0 => InterruptHandlerPio<PIO0>;
});

const WIFI_SSID: &'static str = include_str!("WIFI_SSID.txt");
const WIFI_SECRET: &'static str = include_str!("WIFI_SECRET.txt");
const PWN_DIV_INT: u8 = 250;
const PWM_TOP: u16 = 10000;

#[embassy_executor::task]
async fn wifi_task(
    runner: cyw43::Runner<
        'static,
        Output<'static, PIN_23>,
        PioSpi<'static, PIN_25, PIO0, 0, DMA_CH0>,
    >,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}
fn pwm_config(duty_a: u16, duty_b: u16) -> PwmConfig {
    let mut c = PwmConfig::default();
    c.invert_a = false;
    c.invert_b = false;
    c.phase_correct = false;
    c.enable = true;
    c.divider = PWN_DIV_INT.to_fixed();
    c.compare_a = duty_a;
    c.compare_b = duty_b;
    c.top = PWM_TOP;
    c
}

#[embassy_executor::task]
async fn logger_task(driver: Driver<'static, USB>) {
    embassy_usb_logger::run!(1024, log::LevelFilter::Info, driver);
}

const MAX_DUTY: u16 = 3500;
const STOP: (u16, u16) = (0, 0);
const FORWARD: (u16, u16) = (0, MAX_DUTY);
const BACK: (u16, u16) = (MAX_DUTY, 0);

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Init USB logger
    let driver = Driver::new(p.USB, Irqs);
    spawner.spawn(logger_task(driver)).unwrap();

    // Init ADC
    let mut adc = Adc::new(p.ADC, Irqs, ConfigAdc::default());

    // Init input pins
    let mut color_level = Channel::new_pin(p.PIN_26, Pull::None);
    // Do nothing

    // Init PWM pins
    // left
    let mut pwm_1 = Pwm::new_output_ab(p.PWM_CH1, p.PIN_2, p.PIN_3, pwm_config(0, 0));
    // right
    let mut pwm_2 = Pwm::new_output_ab(p.PWM_CH3, p.PIN_6, p.PIN_7, pwm_config(0, 0));

    let left = Input::new(p.PIN_0, Pull::None);
    let right = Input::new(p.PIN_1, Pull::None);

    ///////////////////// TCP /////////////

    // Use cyw43 firmware
    let fw = include_bytes!("../deps/cyw43-firmware/43439A0.bin");
    let clm = include_bytes!("../deps/cyw43-firmware/43439A0_clm.bin");

    // Init cyw43
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );
    let state = make_static!(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    spawner.spawn(wifi_task(runner)).unwrap();
    control.init(clm).await;
    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    // Generate random seed
    let seed = 0x0123_4567_89ab_cdef; // chosen by fair dice roll. guarenteed to be random.

    // Init network stack
    let config = ConfigNet::ipv4_static(embassy_net::StaticConfigV4 {
        address: Ipv4Cidr::new(Ipv4Address::from_str("192.168.88.13").unwrap(), 24),
        dns_servers: Vec::new(),
        gateway: Some(Ipv4Address::from_str("192.168.88.1").unwrap()),
    });
    let stack = &*make_static!(Stack::new(
        net_device,
        config,
        make_static!(StackResources::<2>::new()),
        seed
    ));
    spawner.spawn(net_task(stack)).unwrap();

    // Join wifi network
    log::info!(
        "Joining access point {} (link up: {})",
        WIFI_SSID,
        stack.is_link_up()
    );
    // loop {
    //     match control.join_wpa2(WIFI_SSID, WIFI_SECRET).await {
    //         Ok(_) => break,
    //         Err(err) => {
    //             log::info!("join failed with status={}", err.status);
    //             Timer::after(Duration::from_millis(1000)).await;
    //         }
    //     }
    // }

    // // Connect to TCP server
    // loop {
    //     log::info!("connecting...");
    //     let mut rx_buffer = [0u8; SOCKET_BUFFER_SIZE];
    //     let mut tx_buffer = [0u8; SOCKET_BUFFER_SIZE];
    //     let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    //     socket.set_timeout(Some(Duration::from_secs(300)));
    //     let address = Ipv4Address::from_str("10.1.1.254").unwrap();
    //     if let Err(err) = socket.connect((address, 9001)).await {
    //         log::warn!("connection error: {:?}", err);
    //         Timer::after(Duration::from_millis(1000)).await;
    //         continue;
    //     }

    //     let msg = b"Hello world!\n";
    //     loop {
    //         log::info!("tx: {}", core::str::from_utf8(msg).unwrap());
    //         if let Err(err) = socket.write_all(msg).await {
    //             log::warn!("connection error: {:?}", err);
    //             break;
    //         }
    //         Timer::after(Duration::from_millis(1000)).await;
    //     }
    // }
    ////////////////////////////////////////////////////

    // Read pin
    let mut curr_direction = FORWARD;
    loop {
        let left_level = left.get_level();
        let is_left_free = left_level == Level::High;
        let right_level = right.get_level();
        let is_right_free = right_level == Level::High;

        // log::info!(
        //     "GP26: {}, left(GP0): {}, right(GP1): {}",
        //     gp26_level,
        //     level2str(left_level),
        //     level2str(right_level),
        // );
        let seen_obstacle = !is_left_free || !is_right_free;
        let (duty_a, duty_b) = if seen_obstacle {
            log::info!("Obstacle detected. Go Forward");
            FORWARD
        } else {
            let gp26_level = adc.read(&mut color_level).await.unwrap();
            let is_white = gp26_level < 150;
            log::info!("Obstacle not detected. Color level: {}", gp26_level);
            if is_white {
                curr_direction = match curr_direction {
                    FORWARD => BACK,
                    BACK => FORWARD,
                    _ => unreachable!(),
                };
            }
            curr_direction
        };
        // go back
        let c1 = pwm_config(duty_a, duty_b);
        let c2 = pwm_config(duty_a, duty_b);

        pwm_1.set_config(&c1);
        pwm_2.set_config(&c2);
    }
}

// fn go(direction: (u16, u16)) {

// }

fn level2str(l: Level) -> &'static str {
    match l {
        Level::Low => "LO",
        Level::High => "HI",
    }
}
