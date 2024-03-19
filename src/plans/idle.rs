//! Run background tasks until the button is pressed.

use crate::{
    brokers::{BrokerFlow, Orb, OrbPlan},
    consts::{BATTERY_VOLTAGE_SHUTDOWN_IDLE_THRESHOLD_MV, BUTTON_LONG_PRESS_DURATION},
    ext::broadcast::ReceiverExt as _,
    mcu,
};
use eyre::Result;
use futures::prelude::*;
use std::{
    task::{Context, Poll},
    time::SystemTime,
};

/// Idle plan.
#[derive(Default)]
pub struct Plan {
    is_pressed: bool,
    press_time: Option<SystemTime>,
}

impl OrbPlan for Plan {
    fn poll_extra(&mut self, orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        while let Poll::Ready(output) = orb.main_mcu.rx_mut().next_broadcast().poll_unpin(cx) {
            match output? {
                // Detect short button press to start a new signup.
                mcu::main::Output::Button(true) => {
                    self.is_pressed = true;
                    self.press_time = Some(SystemTime::now());
                }
                mcu::main::Output::Button(false) => {
                    if self.is_pressed
                        && SystemTime::now()
                            .duration_since(self.press_time.unwrap_or(SystemTime::UNIX_EPOCH))
                            .unwrap()
                            <= BUTTON_LONG_PRESS_DURATION
                    {
                        self.is_pressed = false;
                        return Ok(BrokerFlow::Break);
                    }
                }
                // Trigger shutdown if the battery voltage is below a certain threshold in Idle state.
                mcu::main::Output::BatteryVoltage(battery_voltage) => {
                    let battery_voltage_sum_mv = battery_voltage.battery_cell1_mv
                        + battery_voltage.battery_cell2_mv
                        + battery_voltage.battery_cell3_mv
                        + battery_voltage.battery_cell4_mv;
                    if battery_voltage_sum_mv < BATTERY_VOLTAGE_SHUTDOWN_IDLE_THRESHOLD_MV {
                        tracing::info!(
                            "Shutting down because of low battery voltage: {}",
                            battery_voltage_sum_mv
                        );
                        orb.trigger_shutdown_idle = true;
                        orb.led.shutdown(false);
                        return Ok(BrokerFlow::Break);
                    }
                }
                _ => {}
            }
        }
        Ok(BrokerFlow::Continue)
    }
}

impl Plan {
    /// Runs the idle plan.
    pub async fn run(&mut self, orb: &mut Orb) -> Result<bool> {
        // trigger_shutdown_idle might have been set by the biometric capture plan
        // thus, shut down when back in idle state
        if orb.trigger_shutdown_idle {
            return Ok(false);
        }
        orb.main_mcu.rx_mut().clear()?;
        orb.run(self).await?;
        if orb.trigger_shutdown_idle {
            return Ok(false);
        }
        Ok(true)
    }
}
