#[cfg(feature = "stage")]
use crate::process::Command;
use crate::{
    agents::{internal_temperature, thermal},
    backend::status,
    config::Config,
    consts::{
        BATTERY_VOLTAGE_SHUTDOWN_IDLE_THRESHOLD_MV, BATTERY_VOLTAGE_SHUTDOWN_SIGNUP_THRESHOLD_MV,
        BUTTON_DOUBLE_PRESS_DEAD_TIME, BUTTON_DOUBLE_PRESS_DURATION, BUTTON_LONG_PRESS_DURATION,
        BUTTON_TRIPLE_PRESS_DURATION, CONFIG_UPDATE_INTERVAL, DEFAULT_MAX_FAN_SPEED,
        GRACEFUL_SHUTDOWN_MAX_DELAY_SECONDS, SHUTDOWN_SOUND_DURATION, STATUS_UPDATE_INTERVAL,
    },
    dbus::SupervisorProxy,
    dd_gauge, dd_incr,
    ext::{broadcast::ReceiverExt as _, mpsc::SenderExt as _},
    identification::{GIT_VERSION, ORB_OS_VERSION},
    mcu::{self, main::Version, Mcu},
    monitor, ssd, ui,
};
use agentwire::{agent, port, Broker, BrokerFlow};
use eyre::{bail, eyre, Error, Result, WrapErr};
use futures::{
    future::{Fuse, FusedFuture},
    prelude::*,
};
#[cfg(feature = "stage")]
use local_ip_address::local_ip;
use nix::unistd::sync;
use orb_messages;
use std::{
    collections::VecDeque,
    convert::Infallible,
    pin::Pin,
    process,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, Mutex},
    task::{self, JoinHandle},
    time::{self, sleep},
};
use tokio_stream::wrappers::IntervalStream;

const SSD_HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(10);

/// Abstract observer plan.
#[allow(missing_docs)]
pub trait Plan: Send {
    fn handle_button(&mut self, _pressed: bool) -> Result<()> {
        Ok(())
    }

    fn handle_internal_temperature(&mut self, _cpu: i16, _gpu: i16, _ssd: i16) -> Result<()> {
        Ok(())
    }

    fn handle_temperature_level(
        &mut self,
        _temperature_level: thermal::TemperatureLevel,
    ) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_success_ack(&mut self, _input: mcu::main::Input) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_temperature(
        &mut self,
        _output: orb_messages::mcu_main::Temperature,
    ) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_battery_capacity(
        &mut self,
        _output: orb_messages::mcu_main::BatteryCapacity,
    ) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_battery_voltage(
        &mut self,
        _output: orb_messages::mcu_main::BatteryVoltage,
    ) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_battery_is_charging(
        &mut self,
        _output: orb_messages::mcu_main::BatteryIsCharging,
    ) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_voltage(&mut self, _output: orb_messages::mcu_main::Voltage) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_motor_range(
        &mut self,
        _output: orb_messages::mcu_main::MotorRange,
    ) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_tof_distance(&mut self, _distance: u32) -> Result<()> {
        Ok(())
    }

    fn handle_fan_status(&mut self, _status: orb_messages::mcu_main::FanStatus) -> Result<()> {
        Ok(())
    }

    fn handle_mcu_versions(&mut self, _versions: mcu::main::Versions) -> Result<()> {
        Ok(())
    }

    fn before_config_update(&mut self) -> Result<bool> {
        Ok(true)
    }

    fn is_fan_control_active(&mut self) -> bool {
        true
    }

    fn handle_gps(
        &mut self,
        _latitude: Option<f64>,
        _longitude: Option<f64>,
        _satellite_count: Option<u8>,
    ) -> Result<()> {
        Ok(())
    }

    fn poll_extra(&mut self, _cx: &mut Context<'_>) -> Result<BrokerFlow> {
        Ok(BrokerFlow::Continue)
    }

    fn poll_status_update(&mut self, _observer: &mut Observer, _cx: &mut Context) -> Result<()> {
        Ok(())
    }

    fn config_update(&mut self, _observer: &mut Observer) -> Result<()> {
        Ok(())
    }
}

/// Default observer plan.
#[derive(Default)]
pub struct DefaultPlan {
    _priv: (),
}

impl Plan for DefaultPlan {
    fn poll_status_update(&mut self, observer: &mut Observer, cx: &mut Context) -> Result<()> {
        if let Poll::Ready(result) = observer.status_update.poll_unpin(cx) {
            result?;
        }
        if observer.status_update.is_terminated()
            && observer.status_update_interval.next().poll_unpin(cx).is_ready()
        {
            let request = observer.status_request.clone();
            let future = async move {
                match status::request(&request).await {
                    Ok(()) => {
                        dd_incr!("main.count.http.status_update.success");
                        tracing::trace!("Status request sent");
                    }
                    Err(err) => {
                        dd_incr!("main.count.http.status_update.error");
                        tracing::error!("Status request failed: {err:?}");
                    }
                }
                Ok(())
            };
            observer.status_update = (Box::pin(future) as StatusUpdate).fuse();
        }
        Ok(())
    }

    fn config_update(&mut self, observer: &mut Observer) -> Result<()> {
        if self.before_config_update()? {
            match observer.config_update.as_mut().map(future::FutureExt::now_or_never) {
                Some(None) => return Ok(()),
                Some(Some(result)) => result??,
                None => {}
            }
            let config = Arc::clone(&observer.config);
            let ui = observer.ui.clone();
            observer.config_update = Some(tokio::spawn(async move {
                if let Ok(new_config) = Config::download().await {
                    *config.lock().await = new_config;
                    config.lock().await.propagate_to_ui(ui.as_ref());
                }
                Ok(())
            }));
        }
        Ok(())
    }
}

impl DefaultPlan {
    /// Runs the default plan of the observer.
    pub async fn run(mut self, mut observer: Observer) -> Result<()> {
        observer.enable_internal_temperature()?;
        observer.enable_thermal()?;
        observer.run(&mut self).await.wrap_err("observer")?;
        // The observer broker exists normally only to request a shutdown.
        // It can't perform a shutdown on its own, because it doesn't have
        // access to the async context.
        observer.shutdown().await.wrap_err("shutdown")?;
        Ok(())
    }
}

/// System broker. Runs parallel background tasks.
#[allow(missing_docs)]
#[derive(Broker)]
#[broker(plan = Plan, error = Error, poll_extra)]
pub struct Observer {
    #[agent(task, init)]
    pub internal_temperature: agent::Cell<internal_temperature::Sensor>,
    #[agent(task, init)]
    pub thermal: agent::Cell<thermal::Agent>,
    config: Arc<Mutex<Config>>,
    ui: Box<dyn ui::Engine>,
    main_mcu: Box<dyn Mcu<mcu::Main>>,
    net_monitor: Box<dyn monitor::net::Monitor>,
    button_long_press_timer: Fuse<Pin<Box<time::Sleep>>>,
    button_double_press_timer: Fuse<Pin<Box<time::Sleep>>>,
    button_press_sequence: VecDeque<Instant>,
    config_update: Option<JoinHandle<Result<()>>>,
    config_update_interval: IntervalStream,
    status_update: Fuse<StatusUpdate>,
    status_update_interval: IntervalStream,
    status_request: status::Request,
    ssd_rx: mpsc::UnboundedReceiver<ssd::Stats>,
    log_line: String,
    last_fan_max_speed: f32,
    battery_is_not_charging_counter: u32,
    network_unblocked: bool,
    battery_tags: Vec<String>,
    signup_flag: Arc<AtomicBool>,
}

/// [`Observer`] builder.
#[derive(Default)]
pub struct Builder {
    config: Option<Arc<Mutex<Config>>>,
    ui: Option<Box<dyn ui::Engine>>,
    main_mcu: Option<Box<dyn Mcu<mcu::Main>>>,
    net_monitor: Option<Box<dyn monitor::net::Monitor>>,
    signup_flag: Option<Arc<AtomicBool>>,
}

type StatusUpdate = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

impl Builder {
    /// Builds a new [`Observer`].
    #[must_use]
    pub fn build(self) -> Observer {
        let Self { config, ui: led, main_mcu, net_monitor, signup_flag } = self;
        let (ssd_tx, ssd_rx) = mpsc::unbounded_channel();
        task::spawn(ssd_health_check(ssd_tx));
        let mut status_update_interval = time::interval(STATUS_UPDATE_INTERVAL);
        status_update_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut status_request = status::Request::default();
        status_request.version.current_release.clone_from(&ORB_OS_VERSION);
        new_observer!(
            config: config.unwrap_or_default(),
            ui: led.unwrap_or_else(|| Box::new(ui::Fake)),
            main_mcu: main_mcu.unwrap_or_else(|| Box::<mcu::main::Fake>::default()),
            net_monitor: net_monitor.unwrap_or_else(|| Box::new(monitor::net::Fake)),
            button_long_press_timer: Fuse::terminated(),
            button_double_press_timer: Fuse::terminated(),
            button_press_sequence: VecDeque::new(),
            config_update: None,
            config_update_interval: IntervalStream::new(time::interval(CONFIG_UPDATE_INTERVAL)),
            status_update: Fuse::terminated(),
            status_update_interval: IntervalStream::new(status_update_interval),
            status_request,
            ssd_rx,
            log_line: String::new(),
            last_fan_max_speed: DEFAULT_MAX_FAN_SPEED,
            battery_is_not_charging_counter: 0,
            network_unblocked: false,
            battery_tags: Vec::new(),
            signup_flag: signup_flag.unwrap_or_default(),
        )
    }

    /// Sets the shared config.
    #[must_use]
    pub fn config(mut self, config: Arc<Mutex<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    /// Sets the LED engine.
    #[must_use]
    pub fn ui(mut self, ui: Box<dyn ui::Engine>) -> Self {
        self.ui = Some(ui);
        self
    }

    /// Sets the main MCU interface.
    #[must_use]
    pub fn main_mcu(mut self, main_mcu: Box<dyn Mcu<mcu::Main>>) -> Self {
        self.main_mcu = Some(main_mcu);
        self
    }

    /// Sets the network monitor interface.
    #[must_use]
    pub fn net_monitor(mut self, net_monitor: Box<dyn monitor::net::Monitor>) -> Self {
        self.net_monitor = Some(net_monitor);
        self
    }

    /// Sets the biometric capture state atomic flag.
    #[must_use]
    pub fn signup_flag(mut self, signup_flag: Arc<AtomicBool>) -> Self {
        self.signup_flag = Some(signup_flag);
        self
    }
}

impl Observer {
    /// Returns a new [`Builder`].
    #[must_use]
    pub fn builder() -> Builder {
        Builder::default()
    }

    /// Shuts down the orb.
    pub async fn shutdown(&mut self) -> Result<Infallible> {
        dd_incr!("main.count.global.shutting_down");
        tracing::info!("Shutting down the Orb");
        // save latest config to disk
        tracing::info!("Starting to write config to disk");
        self.config.lock().await.store().await?;
        sync(); // sync filesystem
        tracing::info!("Config written to disk");

        // pause to avoid killing worldcoin-ui before the shutdown sound is done playing
        sleep(SHUTDOWN_SOUND_DURATION).await;

        // shutdown comes from the MCU in last resort
        self.main_mcu.send(mcu::main::Input::Shutdown(GRACEFUL_SHUTDOWN_MAX_DELAY_SECONDS)).await?;
        let connection = zbus::Connection::session()
            .await
            .wrap_err("failed establishing a `session` dbus connection")?;
        let proxy =
            SupervisorProxy::new(&connection).await.wrap_err("failed creating supervisor proxy")?;
        tracing::info!(
            "scheduling poweroff in 0ms by calling \
             org.worldcoin.OrbSupervisor1.Manager.ScheduleShutdown"
        );
        proxy
            .schedule_shutdown("poweroff", 0)
            .await
            .wrap_err("failed to schedule poweroff to supervisor proxy")?;
        process::exit(0);
    }

    #[allow(clippy::unused_self)]
    fn init_internal_temperature(&mut self) -> internal_temperature::Sensor {
        internal_temperature::Sensor
    }

    #[allow(clippy::unused_self)]
    fn init_thermal(&mut self) -> thermal::Agent {
        thermal::Agent::default()
    }

    #[allow(clippy::needless_pass_by_value)]
    #[allow(clippy::similar_names)] // triggered on `cpu` and `gpu`
    fn handle_internal_temperature(
        &mut self,
        plan: &mut dyn Plan,
        output: port::Output<internal_temperature::Sensor>,
    ) -> Result<BrokerFlow> {
        let cpu: i16 = output.value.cpu;
        let gpu: i16 = output.value.gpu;
        let ssd: i16 = output.value.ssd;
        dd_gauge!("main.gauge.system.temperature", cpu.to_string(), "type:cpu");
        dd_gauge!("main.gauge.system.temperature", gpu.to_string(), "type:gpu");
        dd_gauge!("main.gauge.system.temperature", ssd.to_string(), "type:ssd");
        if let Some(wifi) = output.value.wifi {
            dd_gauge!("main.gauge.system.temperature", wifi.to_string(), "type:wifi");
        }
        plan.handle_internal_temperature(cpu, gpu, ssd)?;
        let thermal_agent = self.thermal.enabled().expect("thermal agent is not enabled");
        thermal_agent.tx.send_now(port::Input::new(thermal::Input::JetsonCpu(cpu)))?;
        thermal_agent.tx.send_now(port::Input::new(thermal::Input::JetsonGpu(gpu)))?;
        self.status_request.temperature.cpu = f64::from(cpu);
        self.status_request.temperature.gpu = f64::from(gpu);
        self.status_request.temperature.ssd = f64::from(ssd);
        if let Some(wifi) = output.value.wifi {
            self.status_request.temperature.wifi = f64::from(wifi);
        }
        Ok(BrokerFlow::Continue)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn handle_thermal(
        &mut self,
        plan: &mut dyn Plan,
        output: port::Output<thermal::Agent>,
    ) -> Result<BrokerFlow> {
        match output.value {
            thermal::Output::FanSpeed(fan_speed) => {
                if plan.is_fan_control_active() {
                    let fan_max_speed = if let Some(config) = &self.config.lock().now_or_never() {
                        config.fan_max_speed.unwrap_or(self.last_fan_max_speed)
                    } else {
                        self.last_fan_max_speed
                    };
                    self.last_fan_max_speed = fan_max_speed;

                    let adjusted_fan_speed =
                        (fan_speed * fan_max_speed / 100.0).clamp(1.0, DEFAULT_MAX_FAN_SPEED);
                    tracing::trace!(
                        "Setting FAN speed to {adjusted_fan_speed} (config max {fan_max_speed})"
                    );
                    self.main_mcu.send_now(mcu::main::Input::FanSpeed(adjusted_fan_speed))?;
                }
            }
            thermal::Output::TemperatureLevel(temperature_level) => {
                plan.handle_temperature_level(temperature_level)?;
            }
        }
        Ok(BrokerFlow::Continue)
    }

    fn poll_extra(
        &mut self,
        plan: &mut dyn Plan,
        cx: &mut Context<'_>,
        _fence: Instant,
    ) -> Result<Option<Poll<()>>> {
        while let Poll::Ready(report) = self.net_monitor.poll_next_unpin(cx) {
            self.network_unblocked = true;
            self.handle_net_monitor(report.ok_or_else(|| eyre!("network monitor exited"))?);
        }
        while let Poll::Ready(output) = self.main_mcu.rx_mut().next_broadcast().poll_unpin(cx) {
            if matches!(self.handle_mcu(plan, output?)?, BrokerFlow::Break) {
                return Ok(Some(Poll::Ready(())));
            }
        }
        if let Poll::Ready(()) = self.button_long_press_timer.poll_unpin(cx) {
            tracing::debug!("Button long press");
            tracing::info!("Shutdown requested by the user");
            self.ui.shutdown(true);
            return Ok(Some(Poll::Ready(())));
        }
        if let Poll::Ready(()) = self.button_double_press_timer.poll_unpin(cx) {
            tracing::debug!("Button double press");
            self.button_press_sequence.clear();
            handle_double_press(self);
        }
        while let Poll::Ready(report) = self.ssd_rx.poll_recv(cx) {
            if let Some(report) = report {
                self.handle_ssd_health_check(&report);
            } else {
                bail!("SSD health check failed");
            }
        }
        if self.network_unblocked {
            while self.config_update_interval.next().poll_unpin(cx).is_ready() {
                plan.config_update(self)?;
            }
            plan.poll_status_update(self, cx)?;
        }
        if matches!(plan.poll_extra(cx)?, BrokerFlow::Break) {
            return Ok(Some(Poll::Ready(())));
        }
        Ok(Some(Poll::Pending))
    }

    // Handles messages from the main MCU.
    #[allow(clippy::too_many_lines)]
    fn handle_mcu(&mut self, plan: &mut dyn Plan, output: mcu::main::Output) -> Result<BrokerFlow> {
        match output {
            mcu::main::Output::SuccessAck(input) => {
                log_mcu_success_ack(&input);
                plan.handle_mcu_success_ack(input)?;
            }
            mcu::main::Output::Button(pressed) => {
                if pressed {
                    tracing::debug!("Button pressed");
                    if self.button_press_sequence.len() >= 3 {
                        self.button_press_sequence.pop_back();
                    }
                    self.button_press_sequence.push_front(Instant::now());
                    if self.button_long_press_timer.is_terminated() {
                        self.button_long_press_timer =
                            Box::pin(time::sleep(BUTTON_LONG_PRESS_DURATION)).fuse();
                    }
                    self.button_double_press_timer = Fuse::terminated();
                } else {
                    tracing::debug!("Button released");
                    if self
                        .button_press_sequence
                        .get(2)
                        .map_or(false, |t| t.elapsed() <= BUTTON_TRIPLE_PRESS_DURATION)
                    {
                        tracing::debug!("Button triple press");
                        self.button_press_sequence.clear();
                    } else if self
                        .button_press_sequence
                        .get(1)
                        .map_or(false, |t| t.elapsed() <= BUTTON_DOUBLE_PRESS_DURATION)
                    {
                        self.button_double_press_timer =
                            Box::pin(time::sleep(BUTTON_DOUBLE_PRESS_DEAD_TIME)).fuse();
                    }
                    self.button_long_press_timer = Fuse::terminated();
                }
                plan.handle_button(pressed)?;
            }
            mcu::main::Output::Temperature(temperature) => {
                handle_mcu_temperature(self, &temperature)?;
                plan.handle_mcu_temperature(temperature)?;
            }
            mcu::main::Output::Voltage(voltage) => {
                log_mcu_voltage(&voltage);
                plan.handle_mcu_voltage(voltage)?;
            }
            mcu::main::Output::BatteryCapacity(capacity) => {
                tracing::trace!("Battery capacity: {}%", capacity.percentage);
                dd_gauge!("main.gauge.system.battery", capacity.percentage.to_string());
                self.status_request.battery.level = f64::from(capacity.percentage);
                self.ui.battery_capacity(capacity.percentage);
                plan.handle_mcu_battery_capacity(capacity)?;
            }
            mcu::main::Output::BatteryVoltage(battery_voltage) => {
                dd_gauge!(
                    "main.gauge.system.voltage",
                    battery_voltage.battery_cell1_mv.to_string(),
                    "type:cell1"
                );
                dd_gauge!(
                    "main.gauge.system.voltage",
                    battery_voltage.battery_cell2_mv.to_string(),
                    "type:cell2"
                );
                dd_gauge!(
                    "main.gauge.system.voltage",
                    battery_voltage.battery_cell3_mv.to_string(),
                    "type:cell3"
                );
                dd_gauge!(
                    "main.gauge.system.voltage",
                    battery_voltage.battery_cell4_mv.to_string(),
                    "type:cell4"
                );
                let battery_voltage_sum_mv = battery_voltage.battery_cell1_mv
                    + battery_voltage.battery_cell2_mv
                    + battery_voltage.battery_cell3_mv
                    + battery_voltage.battery_cell4_mv;
                if self.signup_flag.load(Ordering::Relaxed) {
                    if battery_voltage_sum_mv < BATTERY_VOLTAGE_SHUTDOWN_SIGNUP_THRESHOLD_MV {
                        tracing::info!(
                            "Shutting down during SIGNUP state because of low battery voltage: {} \
                             mV",
                            battery_voltage_sum_mv
                        );
                        self.ui.shutdown(false);
                        return Ok(BrokerFlow::Break);
                    }
                } else if battery_voltage_sum_mv < BATTERY_VOLTAGE_SHUTDOWN_IDLE_THRESHOLD_MV {
                    tracing::info!(
                        "Shutting down during idle state because of low battery voltage: {} mV",
                        battery_voltage_sum_mv
                    );
                    self.ui.shutdown(false);
                    return Ok(BrokerFlow::Break);
                }
                plan.handle_mcu_battery_voltage(battery_voltage)?;
            }
            mcu::main::Output::BatteryIsCharging(output) => {
                self.battery_is_not_charging_counter = if output.battery_is_charging {
                    0
                } else {
                    self.battery_is_not_charging_counter + 1
                };
                // This prevents a bug where the battery charging status alternates between true and false.
                let battery_is_charging = self.battery_is_not_charging_counter < 8;
                if battery_is_charging {
                    dd_incr!("main.count.system.battery.is_charging");
                } else {
                    dd_incr!("main.count.system.battery.is_not_charging");
                }
                self.ui.battery_is_charging(battery_is_charging);
                self.status_request.battery.is_charging = battery_is_charging;
            }
            mcu::main::Output::MotorRange(motor_range) => {
                log_mcu_motor_range(&motor_range);
                plan.handle_mcu_motor_range(motor_range)?;
            }
            mcu::main::Output::FanStatus(status) => {
                if status.fan_id == orb_messages::mcu_main::fan_status::FanId::Main as i32 {
                    dd_gauge!(
                        "main.gauge.system.fan_main_rpm",
                        status.measured_speed_rpm.to_string()
                    );
                } else if status.fan_id == orb_messages::mcu_main::fan_status::FanId::Aux as i32 {
                    dd_gauge!(
                        "main.gauge.system.fan_aux_rpm",
                        status.measured_speed_rpm.to_string()
                    );
                }
                plan.handle_fan_status(status)?;
            }
            mcu::main::Output::AmbientLight(als) => {
                dd_gauge!("main.gauge.system.als", als.ambient_light_lux.to_string());
            }
            mcu::main::Output::FatalError(error) => {
                // convert to enum for clearer representation in Datadog
                let reason_enum = match error.reason {
                    x if x == orb_messages::mcu_main::fatal_error::FatalReason::FatalKFatal as i32 => {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalKFatal)
                    }
                    x if x
                        == orb_messages::mcu_main::fatal_error::FatalReason::FatalAssertHard as i32 =>
                    {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalAssertHard)
                    }
                    x if x
                        == orb_messages::mcu_main::fatal_error::FatalReason::FatalCriticalTemperature
                            as i32 =>
                    {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalCriticalTemperature)
                    }
                    x if x == orb_messages::mcu_main::fatal_error::FatalReason::FatalWatchdog as i32 => {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalWatchdog)
                    }
                    x if x == orb_messages::mcu_main::fatal_error::FatalReason::FatalBrownout as i32 => {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalBrownout)
                    }
                    x if x == orb_messages::mcu_main::fatal_error::FatalReason::FatalLowPower as i32 => {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalLowPower)
                    }
                    x if x
                        == orb_messages::mcu_main::fatal_error::FatalReason::FatalSoftwareUnknown
                            as i32 =>
                    {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalSoftwareUnknown)
                    }
                    x if x == orb_messages::mcu_main::fatal_error::FatalReason::FatalPinReset as i32 => {
                        Ok(orb_messages::mcu_main::fatal_error::FatalReason::FatalPinReset)
                    }
                    _ => Err(()),
                };
                if let Ok(reason) = reason_enum {
                    dd_incr!("main.count.system.mcu_fatal", &format!("main_mcu_reason:{reason:?}"));
                } else {
                    tracing::error!("Unable to parse fatal MCU error");
                }
            }
            mcu::main::Output::Versions(versions) => {
                dd_incr!(
                    "main.count.global.version",
                    &format!("main_mcu:{}", versions.primary),
                    &format!("main_mcu_secondary:{}", versions.secondary),
                    &format!("orb_core:{}", *GIT_VERSION)
                );
                plan.handle_mcu_versions(versions)?;
            }
            mcu::main::Output::Gps(message) => {
                self.handle_gps(plan, message)?;
            }
            mcu::main::Output::Logs(logs) => {
                self.log_line.push_str(&logs);
                if self.log_line.contains('\n') {
                    tracing::info!("main-mcu: {}", self.log_line.trim());
                    self.log_line.clear();
                }
            }
            mcu::main::Output::TofDistance(distance) => {
                plan.handle_mcu_tof_distance(distance)?;
            }
            mcu::main::Output::HardwareDiag(diag) => {
                let component =
                    orb_messages::mcu_main::hardware_diagnostic::Source::try_from(diag.source).ok();
                let status =
                    orb_messages::mcu_main::hardware_diagnostic::Status::try_from(diag.status).ok();
                if let (Some(component), Some(status)) = (component, status) {
                    dd_incr!(
                        "main.count.global.hardware.component_diag",
                        &format!("type:{:?}", component.as_str_name().to_lowercase()),
                        &format!("status:{:?}", status.as_str_name().to_lowercase())
                    );
                }
            }
            mcu::main::Output::BatteryInfo(battery_info) => {
                self.battery_tags = log_battery_info(&battery_info);
            }
            // battery events aren't sent as long as the battery info hasn't been received
            // so that any event is associated with the correct battery
            mcu::main::Output::BatteryReset(reason) => {
                if !self.battery_tags.is_empty() {
                    log_battery_reset_reason(&reason, &self.battery_tags);
                }
            }
            mcu::main::Output::BatteryDiagCommon(diag) => {
                if !self.battery_tags.is_empty() {
                    log_battery_diagnostics_common(&diag, &self.battery_tags);
                }
            }
            mcu::main::Output::BatteryDiagSafety(diag) => {
                if !self.battery_tags.is_empty() {
                    log_battery_diagnostics_safety(&diag, &self.battery_tags);
                }
            }
            mcu::main::Output::BatteryDiagPermanentFail(diag) => {
                if !self.battery_tags.is_empty() {
                    log_battery_diagnostics_permanent_fail(&diag, &self.battery_tags);
                }
            }
            mcu::main::Output::BatteryInfoSocAndStatistics(info) => {
                if !self.battery_tags.is_empty() {
                    log_battery_info_soc_statistics(&info, &self.battery_tags);
                }
            }
            mcu::main::Output::BatteryInfoMaxValues(max_values) => {
                if !self.battery_tags.is_empty() {
                    log_battery_info_max_values(&max_values, &self.battery_tags);
                }
            }
            mcu::main::Output::BatteryStateOfHealth(state_of_health) => {
                if !self.battery_tags.is_empty() {
                    log_battery_state_of_health(&state_of_health, &self.battery_tags);
                }
            }
        }
        Ok(BrokerFlow::Continue)
    }

    fn handle_gps(
        &mut self,
        plan: &mut dyn Plan,
        message: nmea_parser::ParsedMessage,
    ) -> Result<()> {
        let (latitude, longitude, satellite_count) = match message {
            nmea_parser::ParsedMessage::Gga(message) => {
                (message.latitude, message.longitude, message.satellite_count)
            }
            nmea_parser::ParsedMessage::Gll(message) => (message.latitude, message.longitude, None),
            nmea_parser::ParsedMessage::Gns(message) => {
                (message.latitude, message.longitude, message.satellite_count)
            }
            nmea_parser::ParsedMessage::Rmc(message) => (message.latitude, message.longitude, None),
            _ => (None, None, None),
        };
        if let (Some(latitude), Some(longitude)) = (latitude, longitude) {
            self.status_request.location.latitude = latitude;
            self.status_request.location.longitude = longitude;
        }
        plan.handle_gps(latitude, longitude, satellite_count)?;
        Ok(())
    }

    fn handle_net_monitor(&mut self, report: monitor::net::Report) {
        tracing::trace!("Network monitor: {report:?}");
        if !report.is_no_internet() {
            // If internet is available
            dd_gauge!("main.gauge.system.connectivity.ping_time", report.lag.to_string());
            dd_gauge!("main.gauge.system.connectivity.rssi", report.rssi.to_string());
        }
        if report.is_no_internet() {
            self.ui.no_internet();
        } else if report.is_slow_internet() {
            self.ui.slow_internet();
        } else {
            self.ui.good_internet();
        }
        if report.is_no_wlan() {
            self.ui.no_wlan();
        } else if report.is_slow_wlan() {
            self.ui.slow_wlan();
        } else {
            self.ui.good_wlan();
        }
        self.status_request.wifi.quality.signal_level = report.rssi;
        self.status_request.wifi.ssid = report.ssid;
        self.status_request.mac_address = report.mac_address;
    }

    fn handle_ssd_health_check(&mut self, report: &ssd::Stats) {
        self.status_request.ssd.space_left =
            i64::try_from(report.available_space).unwrap_or(i64::MAX);
        self.status_request.ssd.file_left = report.documents;
        self.status_request.ssd.signup_left_to_upload = report.signups;
    }
}

fn log_mcu_success_ack(input: &mcu::main::Input) {
    match input {
        mcu::main::Input::IrLedDuration(ir_led_duration) => {
            dd_gauge!(
                "main.gauge.system.ir_led_duration",
                ir_led_duration.to_string(),
                "type:general"
            );
        }
        mcu::main::Input::IrLedDuration740nm(ir_led_duration) => {
            dd_gauge!("main.gauge.system.ir_led_duration", ir_led_duration.to_string(), "type:740");
        }
        mcu::main::Input::UserLedBrightness(user_led_brightness) => {
            dd_gauge!("main.gauge.system.user_led_brightness", user_led_brightness.to_string());
        }
        mcu::main::Input::LiquidLens(current) => {
            current.map(|current| -> Result<()> {
                dd_gauge!("main.gauge.system.focus", current.to_string());
                Ok(())
            });
        }
        mcu::main::Input::FrameRate(frame_rate) => {
            dd_gauge!("main.gauge.system.frame_rate", frame_rate.to_string());
        }
        mcu::main::Input::Mirror(x, y) => {
            dd_gauge!("main.gauge.mirror.angle", x.to_string(), "type:phi_degrees");
            dd_gauge!("main.gauge.mirror.angle", y.to_string(), "type:theta_degrees");
        }
        mcu::main::Input::FanSpeed(percentage) => {
            dd_gauge!("main.gauge.system.fan_speed", percentage.to_string());
        }
        mcu::main::Input::TofCalibration(calibration) => {
            dd_gauge!("main.gauge.system.tof_calibration", calibration.to_string());
        }
        mcu::main::Input::IrLed(_)
        | mcu::main::Input::MirrorRelative(_, _)
        | mcu::main::Input::PerformMirrorHoming(..)
        | mcu::main::Input::Shutdown(_)
        | mcu::main::Input::Temperature(..)
        | mcu::main::Input::TofTiming(_)
        | mcu::main::Input::TriggeringIrEyeCamera(_)
        | mcu::main::Input::TriggeringIrFaceCamera(_)
        | mcu::main::Input::UserLedPattern(_)
        | mcu::main::Input::ValueGet(..)
        | mcu::main::Input::Version
        | mcu::main::Input::VoltageRequest
        | mcu::main::Input::VoltageRequestPeriod(_)
        | mcu::main::Input::RingLeds(_)
        | mcu::main::Input::CenterLeds(_)
        | mcu::main::Input::OperatorLeds(_)
        | mcu::main::Input::OperatorLedBrightness(_)
        | mcu::main::Input::OperatorLedPattern(_)
        | mcu::main::Input::ConeLedPattern(_)
        | mcu::main::Input::WhiteLedBrightness(_)
        | mcu::main::Input::IrEyeCameraFocusSweepValuesPolynomial(_)
        | mcu::main::Input::PerformIrEyeCameraFocusSweep
        | mcu::main::Input::IrEyeCameraMirrorSweepValuesPolynomial(_)
        | mcu::main::Input::PerformIrEyeCameraMirrorSweep => {}
    }
}

#[cfg(feature = "stage")]
fn speak_ip_address() {
    let espeak_args = ["-a", "10", "-g", "10"];

    if let Ok(ip) = local_ip() {
        let ip = ip.to_string();
        let mut ip_string = String::new();

        for c in ip.chars() {
            if c == '.' {
                ip_string.push_str(" dot");
            } else {
                ip_string.push(' ');
                ip_string.push(c);
            }
        }

        tracing::info!("Local IP Address: {}", ip_string);
        Command::new("/usr/bin/espeak").args(espeak_args).arg(ip_string).spawn().ok();
    } else {
        Command::new("/usr/bin/espeak")
            .args(espeak_args)
            .arg("Could not obtain IP address")
            .spawn()
            .ok();
    }
}

fn handle_double_press(_observer: &mut Observer) {
    #[cfg(feature = "stage")]
    speak_ip_address();
}

#[allow(clippy::too_many_lines)]
fn handle_mcu_temperature(
    observer: &mut Observer,
    output: &orb_messages::mcu_main::Temperature,
) -> Result<()> {
    let thermal_agent = observer.thermal.enabled().expect("thermal agent is not enabled");
    let temp_type = match output {
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainMcu as i32 =>
        {
            thermal_agent
                .tx.send_now(port::Input::new(thermal::Input::MainMcu(output.temperature_c)))?;
            observer.status_request.temperature.main_mcu = f64::from(output.temperature_c);
            "main_mcu"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::SecurityMcu as i32 =>
        {
            thermal_agent
                .tx.send_now(port::Input::new(thermal::Input::SecurityMcu(output.temperature_c)))?;
            observer.status_request.temperature.security_mcu = f64::from(output.temperature_c);
            "security_mcu"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::LiquidLens as i32 =>
        {
            thermal_agent
                .tx.send_now(port::Input::new(thermal::Input::LiquidLens(output.temperature_c)))?;
            observer.status_request.temperature.liquid_lens = f64::from(output.temperature_c);
            "liquid_lens"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit as i32 =>
        {
            thermal_agent
                .tx.send_now(port::Input::new(thermal::Input::FrontUnit(output.temperature_c)))?;
            observer.status_request.temperature.front_unit = f64::from(output.temperature_c);
            "front_unit"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainAccelerometer as i32 =>
        {
            thermal_agent.tx.send_now(port::Input::new(thermal::Input::MainAccelerometer(
                output.temperature_c,
            )))?;
            observer.status_request.temperature.main_accelerometer =
                f64::from(output.temperature_c);
            "main_accelerometer"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::SecurityAccelerometer
                    as i32 =>
        {
            thermal_agent.tx.send_now(port::Input::new(thermal::Input::SecurityAccelerometer(
                output.temperature_c,
            )))?;
            observer.status_request.temperature.security_accelerometer =
                f64::from(output.temperature_c);
            "security_accelerometer"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::BackupBattery as i32 =>
        {
            thermal_agent
                .tx.send_now(port::Input::new(thermal::Input::BackupBattery(output.temperature_c)))?;
            observer.status_request.temperature.backup_battery = f64::from(output.temperature_c);
            "backup_battery"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::BatteryPcb as i32 =>
        {
            observer.status_request.temperature.battery_pcb = f64::from(output.temperature_c);
            "battery_pcb"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::BatteryCell as i32 =>
        {
            observer.status_request.temperature.battery_cell = f64::from(output.temperature_c);
            "battery_cell"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainBoard as i32 =>
        {
            thermal_agent
                .tx.send_now(port::Input::new(thermal::Input::Mainboard(output.temperature_c)))?;
            observer.status_request.temperature.mainboard = f64::from(output.temperature_c);
            "mainboard"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::BatteryPack as i32 =>
        {
            observer.status_request.temperature.battery_pack = f64::from(output.temperature_c);
            "battery_pack"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainBoardUsbHubBot as i32 =>
        {
            observer.status_request.temperature.main_board_usb_hub_bot =
                f64::from(output.temperature_c);
            "main_board_usb_hub_bot"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainBoardUsbHubTop as i32 =>
        {
            observer.status_request.temperature.main_board_usb_hub_top =
                f64::from(output.temperature_c);
            "main_board_usb_hub_top"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainBoardSecuritySupply
                    as i32 =>
        {
            observer.status_request.temperature.main_board_security_supply =
                f64::from(output.temperature_c);
            "main_board_security_supply"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainBoardAudioAmplifier
                    as i32 =>
        {
            observer.status_request.temperature.main_board_audio_amplifier =
                f64::from(output.temperature_c);
            "main_board_audio_amplifier"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::PowerBoardSuperCapCharger
                    as i32 =>
        {
            observer.status_request.temperature.power_board_super_cap_charger =
                f64::from(output.temperature_c);
            "power_board_super_cap_charger"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::PowerBoardPvccSupply
                    as i32 =>
        {
            observer.status_request.temperature.power_board_pvcc_supply =
                f64::from(output.temperature_c);
            "power_board_pvcc_supply"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::PowerBoardSuperCapsLeft
                    as i32 =>
        {
            observer.status_request.temperature.power_board_super_caps_left =
                f64::from(output.temperature_c);
            "power_board_super_caps_left"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::PowerBoardSuperCapsRight
                    as i32 =>
        {
            observer.status_request.temperature.power_board_super_caps_right =
                f64::from(output.temperature_c);
            "power_board_super_caps_right"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit850730LeftTop
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_850_730_left_top =
                f64::from(output.temperature_c);
            "front_unit_850_730_left_top"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit850730LeftBottom
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_850_730_left_bottom =
                f64::from(output.temperature_c);
            "front_unit_850_730_left_bottom"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit850730RightTop
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_850_730_right_top =
                f64::from(output.temperature_c);
            "front_unit_850_730_right_top"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit850730RightBottom
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_850_730_right_bottom =
                f64::from(output.temperature_c);
            "front_unit_850_730_right_bottom"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit940LeftTop
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_940_left_top =
                f64::from(output.temperature_c);
            "front_unit_940_left_top"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit940LeftBottom
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_940_left_bottom =
                f64::from(output.temperature_c);
            "front_unit_940_left_bottom"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit940RightTop
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_940_right_top =
                f64::from(output.temperature_c);
            "front_unit_940_right_top"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit940RightBottom
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_940_right_bottom =
                f64::from(output.temperature_c);
            "front_unit_940_right_bottom"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit940CenterTop
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_940_center_top =
                f64::from(output.temperature_c);
            "front_unit_940_center_top"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit940CenterBottom
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_940_center_bottom =
                f64::from(output.temperature_c);
            "front_unit_940_center_bottom"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnitWhiteTop as i32 =>
        {
            observer.status_request.temperature.front_unit_white_top =
                f64::from(output.temperature_c);
            "front_unit_white_top"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnitShroudRgbTop
                    as i32 =>
        {
            observer.status_request.temperature.front_unit_shroud_rgb_top =
                f64::from(output.temperature_c);
            "front_unit_shroud_rgb_top"
        }
        _ => "undefined_source",
    };
    dd_gauge!(
        "main.gauge.system.temperature",
        output.temperature_c.to_string(),
        &format!("type:{temp_type}")
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn log_mcu_voltage(output: &orb_messages::mcu_main::Voltage) {
    let name = match output {
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::MainMcuInternal as i32 =>
        {
            "supply_3v3_uc"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SecurityMcuInternal as i32 =>
        {
            "security_mcu_internal"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply12v as i32 =>
        {
            "supply_12v"
        }
        output
            if output.source == orb_messages::mcu_main::voltage::VoltageSource::Supply5v as i32 =>
        {
            "supply_5v"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply3v8 as i32 =>
        {
            "supply_3v8"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply3v3 as i32 =>
        {
            "supply_3v3"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply1v8 as i32 =>
        {
            "supply_1v8"
        }
        output if output.source == orb_messages::mcu_main::voltage::VoltageSource::Vbat as i32 => {
            "vbat"
        }
        output if output.source == orb_messages::mcu_main::voltage::VoltageSource::Pvcc as i32 => {
            "pvcc"
        }
        output
            if output.source == orb_messages::mcu_main::voltage::VoltageSource::Caps12v as i32 =>
        {
            "caps_12v"
        }
        output
            if output.source == orb_messages::mcu_main::voltage::VoltageSource::VbatSw as i32 =>
        {
            "vbat_sw"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply3v3Ssd as i32 =>
        {
            "supply_3v3_ssd"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply3v3Wifi as i32 =>
        {
            "supply_3v3_wifi"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply3v3Lte as i32 =>
        {
            "supply_3v3_lte"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply3v6 as i32 =>
        {
            "supply_3v6"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply1v2 as i32 =>
        {
            "supply_1v2"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply2v8 as i32 =>
        {
            "supply_2v8"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply1v8Sec as i32 =>
        {
            "supply_1v8_sec"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply4v7Sec as i32 =>
        {
            "supply_4v7_sec"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::Supply17vCaps as i32 =>
        {
            "supply_17v_caps"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap0 as i32 =>
        {
            "super_cap_0"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap1 as i32 =>
        {
            "super_cap_1"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap2 as i32 =>
        {
            "super_cap_2"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap3 as i32 =>
        {
            "super_cap_3"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap4 as i32 =>
        {
            "super_cap_4"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap5 as i32 =>
        {
            "super_cap_5"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap6 as i32 =>
        {
            "super_cap_6"
        }
        output
            if output.source
                == orb_messages::mcu_main::voltage::VoltageSource::SuperCap7 as i32 =>
        {
            "super_cap_7"
        }
        _ => return,
    };
    dd_gauge!(
        "main.gauge.system.voltage",
        output.voltage_current_mv.to_string(),
        &format!("type:{name}"),
        "aggregation:current"
    );
    dd_gauge!(
        "main.gauge.system.voltage",
        output.voltage_max_mv.to_string(),
        &format!("type:{name}"),
        "aggregation:max"
    );
    dd_gauge!(
        "main.gauge.system.voltage",
        output.voltage_min_mv.to_string(),
        &format!("type:{name}"),
        "aggregation:min"
    );
}

fn log_mcu_motor_range(output: &orb_messages::mcu_main::MotorRange) {
    match output {
        output
            if output.which_motor
                == orb_messages::mcu_main::motor_range::Motor::HorizontalPhi as i32 =>
        {
            dd_gauge!(
                "main.gauge.mirror.range",
                output.range_microsteps.to_string(),
                "type:phi_microsteps"
            );
        }
        output
            if output.which_motor
                == orb_messages::mcu_main::motor_range::Motor::VerticalTheta as i32 =>
        {
            dd_gauge!(
                "main.gauge.mirror.range",
                output.range_microsteps.to_string(),
                "type:theta_microsteps"
            );
        }
        _ => {}
    }
}

async fn ssd_health_check(ssd_tx: mpsc::UnboundedSender<ssd::Stats>) {
    loop {
        match task::spawn_blocking(ssd::stats).await {
            Ok(Ok(Some(stats))) => {
                if ssd_tx.send(stats).is_err() {
                    break;
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => tracing::error!("SSD health check error: {err}"),
            Err(err) => tracing::error!("SSD health check error: {err}"),
        }
        time::sleep(SSD_HEALTH_CHECK_INTERVAL).await;
    }
}

fn log_battery_info(battery_info: &orb_messages::mcu_main::BatteryInfoHwFw) -> Vec<String> {
    let mcu_id = hex::encode(&battery_info.mcu_id[0..]);
    let mut tags = vec![
        format!("battery_id:0x{id}", id = mcu_id),
        format!(
            "battery_hw:{hw_version}",
            hw_version = orb_messages::mcu_main::battery_info_hw_fw::HardwareVersion::try_from(
                battery_info.hw_version
            )
            .unwrap_or(
                orb_messages::mcu_main::battery_info_hw_fw::HardwareVersion::BatteryHwVersionUndetected
            )
            .as_str_name()
        ),
    ];
    if let Some(fw_version) = &battery_info.fw_version {
        let fw_version = Version::from(fw_version);
        tags.push(format!("battery_fw:{fw_version}"));
    }
    dd_incr!("main.count.system.battery_info"; tags.as_slice());
    tags
}

fn log_battery_reset_reason(
    reason: &orb_messages::mcu_main::BatteryResetReason,
    battery_tags: &[String],
) {
    let reason =
        orb_messages::mcu_main::battery_reset_reason::ResetReason::try_from(reason.reset_reason)
            .unwrap_or(orb_messages::mcu_main::battery_reset_reason::ResetReason::Unknown)
            .as_str_name();
    let mut tags = battery_tags.to_owned();
    tags.push(format!("reset_reason:{reason}"));
    dd_incr!("main.count.system.battery_reset_reason"; tags.as_slice());
}

fn log_battery_diagnostics_common(
    diagnostics: &orb_messages::mcu_main::BatteryDiagnosticCommon,
    battery_tags: &[String],
) {
    let mut tags = battery_tags.to_owned();
    let battery_flag_bits = [
        "BATTERY_BUTTON_PRESSED",
        "BATTERY_INSERTED",
        "BATTERY_LOAD_OVER_150MA",
        "BATTERY_IS_CHARGING",
        "BATTERY_USB_PLUGGED_IN",
        "BATTERY_USB_PD_INITIALIZED",
        "BATTERY_USB_PD_ACTIVE",
        "BATTERY_BQ769X2_READS_VALID",
    ];
    for (i, &s) in battery_flag_bits.iter().enumerate() {
        if diagnostics.flags & (1 << i) != 0 {
            tags.push(format!("battery_flags:{s}"));
        }
    }

    let control_statuses_bits = ["LD_ON", "LD_TIMEOUT", "DEEPSLEEP"];
    for (i, &s) in control_statuses_bits.iter().enumerate() {
        if diagnostics.bq769_control_status & (1 << i) != 0 {
            tags.push(format!("bq769_control_status:{s}"));
        }
    }

    let battery_states_bits = [
        "CFGUPDATE",
        "PCHG_MODE",
        "SLEEP_EN",
        "POR",
        "WD",
        "COW_CHK",
        "OTPW",
        "OTPB",
        "SEC0",
        "SEC1",
        "FUSE",
        "SS",
        "PF",
        "SD_CMD",
        "Reserved",
        "SLEEP",
    ];
    for (i, &s) in battery_states_bits.iter().enumerate() {
        if diagnostics.battery_status & (1 << i) != 0 {
            tags.push(format!("battery_status:{s}"));
        }
    }

    let fet_statuses_bits = [
        "CHG_FET", "PCHG_FET", "DSG_FET", "PDSG_FET", "DCHG_PIN", "DDSG_PIN", "ALRT_PIN",
        "Reserved",
    ];
    for (i, &s) in fet_statuses_bits.iter().enumerate() {
        if diagnostics.fet_status & (1 << i) != 0 {
            tags.push(format!("fet_status:{s}"));
        }
    }

    let balancer_states_bits = ["DISCONNECTED", "ACTIVE", "NOT_ACTIVE", "ERROR"];
    for (i, &s) in balancer_states_bits.iter().enumerate() {
        if diagnostics.balancer_state & (1 << i) != 0 {
            tags.push(format!("balancer_state:{s}"));
        }
    }

    dd_incr!("main.count.system.battery_diagnostic"; tags);

    let mut tags = battery_tags.to_owned();
    tags.push(String::from("type:current_ma"));
    dd_gauge!(
        "main.gauge.system.battery_diagnostic",
        diagnostics.current_ma.to_string();
        tags.as_slice()
    );
}

fn log_battery_diagnostics_safety(
    diagnostics: &orb_messages::mcu_main::BatteryDiagnosticSafety,
    battery_tags: &[String],
) {
    let mut tags = battery_tags.to_owned();
    if diagnostics.safety_alert_a != 0 {
        tags.push(format!("safety_alert_a:{}", diagnostics.safety_alert_a));
    }
    if diagnostics.safety_status_a != 0 {
        tags.push(format!("safety_status_a:{}", diagnostics.safety_status_a));
    }
    if diagnostics.safety_alert_b != 0 {
        tags.push(format!("safety_alert_b:{}", diagnostics.safety_alert_b));
    }
    if diagnostics.safety_status_b != 0 {
        tags.push(format!("safety_status_b:{}", diagnostics.safety_status_b));
    }
    if diagnostics.safety_alert_c != 0 {
        tags.push(format!("safety_alert_c:{}", diagnostics.safety_alert_c));
    }
    if diagnostics.safety_status_c != 0 {
        tags.push(format!("safety_status_c:{}", diagnostics.safety_status_c));
    }
    dd_incr!("main.count.system.battery_diagnostic_safety"; tags.as_slice());
}

fn log_battery_diagnostics_permanent_fail(
    diagnostics: &orb_messages::mcu_main::BatteryDiagnosticPermanentFail,
    battery_tags: &[String],
) {
    let mut tags = battery_tags.to_owned();

    if diagnostics.permanent_fail_alert_a != 0 {
        tags.push(format!("permanent_fail_alert_a:{}", diagnostics.permanent_fail_alert_a));
    }
    if diagnostics.permanent_fail_status_a != 0 {
        tags.push(format!("permanent_fail_status_a:{}", diagnostics.permanent_fail_status_a));
    }
    if diagnostics.permanent_fail_alert_b != 0 {
        tags.push(format!("permanent_fail_alert_b:{}", diagnostics.permanent_fail_alert_b));
    }
    if diagnostics.permanent_fail_status_b != 0 {
        tags.push(format!("permanent_fail_status_b:{}", diagnostics.permanent_fail_status_b));
    }
    if diagnostics.permanent_fail_alert_c != 0 {
        tags.push(format!("permanent_fail_alert_c:{}", diagnostics.permanent_fail_alert_c));
    }
    if diagnostics.permanent_fail_status_c != 0 {
        tags.push(format!("permanent_fail_status_c:{}", diagnostics.permanent_fail_status_c));
    }
    if diagnostics.permanent_fail_alert_d != 0 {
        tags.push(format!("permanent_fail_alert_d:{}", diagnostics.permanent_fail_alert_d));
    }
    if diagnostics.permanent_fail_status_d != 0 {
        tags.push(format!("permanent_fail_status_d:{}", diagnostics.permanent_fail_status_d));
    }
    dd_incr!("main.count.system.battery_diagnostic_pf"; tags.as_slice());
}

fn log_battery_info_soc_statistics(
    battery_info: &orb_messages::mcu_main::BatteryInfoSocAndStatistics,
    battery_tags: &[String],
) {
    let mut tags = battery_tags.to_owned();
    let soc_calibration_str =
        orb_messages::mcu_main::battery_info_soc_and_statistics::SocCalibration::try_from(
            battery_info.soc_calibration,
        )
        .unwrap_or(
            orb_messages::mcu_main::battery_info_soc_and_statistics::SocCalibration::StateSocCalUnknown,
        )
        .as_str_name();
    tags.push(format!("soc_calibration_state:{soc_calibration_str}"));
    let soc_state_str =
        orb_messages::mcu_main::battery_info_soc_and_statistics::SocState::try_from(
            battery_info.soc_state,
        )
        .unwrap_or(
            orb_messages::mcu_main::battery_info_soc_and_statistics::SocState::StateSocUnknown,
        )
        .as_str_name();
    tags.push(format!("soc_state:{soc_state_str}"));
    dd_incr!("main.count.system.battery_info_soc_statistics"; &tags);

    battery_tags.clone_into(&mut tags);
    tags.push("type:number_of_button_presses".to_string());
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.number_of_button_presses.to_string();
        tags.as_slice()
    );
    tags.pop();
    tags.push("type:number_of_insertions".to_string());
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.number_of_insertions.to_string();
        tags.as_slice()
    );
    tags.pop();
    tags.push("type:number_of_charges".to_string());
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.number_of_charges.to_string();
        tags.as_slice()
    );
    tags.pop();
    tags.push("type:number_of_written_flash_variables".to_string());
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.number_of_written_flash_variables.to_string();
        tags.as_slice()
    );
}

fn log_battery_info_max_values(
    battery_info: &orb_messages::mcu_main::BatteryInfoMaxValues,
    battery_tags: &[String],
) {
    let mut tags = battery_tags.to_owned();
    tags.push(String::from("type:maximum_capacity_mah"));
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.maximum_capacity_mah.to_string();
        tags.as_slice()
    );
    tags.pop();
    tags.push(String::from("type:maximum_cell_temp_decidegrees"));
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.maximum_cell_temp_decidegrees.to_string();
        tags.as_slice()
    );
    tags.pop();
    tags.push(String::from("type:maximum_pcb_temp_decidegrees"));
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.maximum_pcb_temp_decidegrees.to_string();
        tags.as_slice()
    );
    tags.pop();
    tags.push(String::from("type:maximum_charge_current_ma"));
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.maximum_charge_current_ma.to_string();
        tags.as_slice()
    );
    tags.pop();
    tags.push(String::from("type:maximum_discharge_current_ma"));
    dd_gauge!(
        "main.gauge.system.battery_info",
        battery_info.maximum_discharge_current_ma.to_string();
        tags.as_slice()
    );
}

fn log_battery_state_of_health(
    battery_state_of_health: &orb_messages::mcu_main::BatteryStateOfHealth,
    battery_tags: &[String],
) {
    let mut tags = battery_tags.to_owned();
    tags.push(String::from("type:state_of_health_percentage"));
    dd_gauge!(
        "main.gauge.system.battery_state_of_health",
        battery_state_of_health.percentage.to_string();
        tags.as_slice()
    );
    tags.pop();
}
