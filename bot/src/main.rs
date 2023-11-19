#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(incomplete_features)]

use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Pull};
use embassy_time::{Duration, Timer};
use fixed::traits::ToFixed;
use rp2040_panic_usb_boot as _;

use embassy_rp::adc::{Adc, Channel, Config as ConfigAdc, InterruptHandler as InterruptHandlerAdc};
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::USB;
use embassy_rp::pwm::{Config as PwmConfig, Pwm};
use embassy_rp::usb::{Driver, InterruptHandler as InterruptHandlerUsb};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandlerUsb<USB>;
    ADC_IRQ_FIFO => InterruptHandlerAdc;
});

const PWN_DIV_INT: u8 = 250;
const PWM_TOP: u16 = 10000;

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
            let is_white = gp26_level < 200;
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
        // let c1 = pwm_config(duty_a, duty_b);
        // let c2 = pwm_config(duty_a, duty_b);

        // pwm_1.set_config(&c1);
        // pwm_2.set_config(&c2);
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
