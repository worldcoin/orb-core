#[cfg(feature = "stage")]
use std::process::Command;
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use eyre::{eyre, Result};
use futures::{
    future::{Fuse, FusedFuture},
    prelude::*,
};
#[cfg(feature = "stage")]
use local_ip_address::local_ip;
use orb_messages;
use tokio::{sync::Mutex, task, task::JoinHandle, time};
use tokio_stream::wrappers::IntervalStream;

use orb_macros::Broker;

use crate::{
    agents::{internal_temperature, thermal},
    backend::status,
    config::Config,
    consts::{
        BUTTON_DOUBLE_PRESS_DEAD_TIME, BUTTON_DOUBLE_PRESS_DURATION, BUTTON_LONG_PRESS_DURATION,
        BUTTON_TRIPLE_PRESS_DURATION, CONFIG_UPDATE_INTERVAL, DEFAULT_MAX_FAN_SPEED,
        STATUS_UPDATE_INTERVAL,
    },
    ext::{broadcast::ReceiverExt as _, mpsc::SenderExt},
    identification::{CURRENT_RELEASE, GIT_VERSION},
    led,
    logger::{LogOnError, DATADOG, NO_TAGS},
    mcu,
    mcu::{main::Version, Mcu},
    monitor, port, sound,
};

use super::{AgentCell, BrokerFlow};

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
                        DATADOG.incr("orb.main.count.http.status_update.success", NO_TAGS).or_log();
                        tracing::trace!("Status request sent");
                    }
                    Err(err) => {
                        DATADOG.incr("orb.main.count.http.status_update.error", NO_TAGS).or_log();
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
            let sound = observer.sound.clone();
            observer.config_update = Some(tokio::spawn(async move {
                let old_lang = config.lock().await.language().clone();
                if let Ok(new_config) = Config::download().await {
                    let new_lang = new_config.language().clone();
                    *config.lock().await = new_config;
                    if old_lang != new_lang {
                        let sound_files_fut = sound.load_sound_files(new_lang.as_deref(), true);
                        sound_files_fut.await?;
                    }
                }
                Ok(())
            }));
        }
        Ok(())
    }
}

impl DefaultPlan {
    /// Runs the default plan of the observer in the background.
    pub fn spawn(mut self, mut observer: Observer) -> Result<JoinHandle<()>> {
        observer.enable_internal_temperature()?;
        observer.enable_thermal()?;
        Ok(task::spawn(async move {
            observer.run(&mut self).await.expect("observer task failure");
        }))
    }
}

/// System broker. Runs parallel background tasks.
#[allow(missing_docs)]
#[derive(Broker)]
pub struct Observer {
    #[agent(task)]
    pub internal_temperature: AgentCell<internal_temperature::Sensor>,
    #[agent(task)]
    pub thermal: AgentCell<thermal::Agent>,
    config: Arc<Mutex<Config>>,
    sound: Box<dyn sound::Player>,
    led: Box<dyn led::Engine>,
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
    log_line: String,
    last_fan_max_speed: f32,
    battery_is_not_charging_counter: u32,
    network_unblocked: bool,
    battery_tags: Vec<String>,
}

/// [`Observer`] builder.
#[derive(Default)]
pub struct Builder {
    config: Option<Arc<Mutex<Config>>>,
    sound: Option<Box<dyn sound::Player>>,
    led: Option<Box<dyn led::Engine>>,
    main_mcu: Option<Box<dyn Mcu<mcu::Main>>>,
    net_monitor: Option<Box<dyn monitor::net::Monitor>>,
}

type StatusUpdate = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

impl Builder {
    /// Builds a new [`Observer`].
    pub fn build(self) -> Observer {
        let Self { config, sound, led, main_mcu, net_monitor } = self;
        let mut status_update_interval = time::interval(STATUS_UPDATE_INTERVAL);
        status_update_interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut status_request = status::Request::default();
        status_request.version.current_release = CURRENT_RELEASE.clone();
        new_observer!(
            config: config.unwrap_or_default(),
            sound: sound.unwrap_or_else(|| Box::new(sound::Fake)),
            led: led.unwrap_or_else(|| Box::new(led::Fake)),
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
            log_line: String::new(),
            last_fan_max_speed: DEFAULT_MAX_FAN_SPEED,
            battery_is_not_charging_counter: 0,
            network_unblocked: false,
            battery_tags: Vec::new(),
        )
    }

    /// Sets the shared config.
    #[must_use]
    pub fn config(mut self, config: Arc<Mutex<Config>>) -> Self {
        self.config = Some(config);
        self
    }

    /// Sets the sound player.
    #[must_use]
    pub fn sound(mut self, sound: Box<dyn sound::Player>) -> Self {
        self.sound = Some(sound);
        self
    }

    /// Sets the LED engine.
    #[must_use]
    pub fn led(mut self, led: Box<dyn led::Engine>) -> Self {
        self.led = Some(led);
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
}

impl Observer {
    /// Returns a new [`Builder`].
    #[must_use]
    pub fn builder() -> Builder {
        Builder::default()
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
        DATADOG.gauge("orb.main.gauge.system.temperature", cpu.to_string(), ["type:cpu"])?;
        DATADOG.gauge("orb.main.gauge.system.temperature", gpu.to_string(), ["type:gpu"])?;
        DATADOG.gauge("orb.main.gauge.system.temperature", ssd.to_string(), ["type:ssd"])?;
        plan.handle_internal_temperature(cpu, gpu, ssd)?;
        let thermal_agent = self.thermal.enabled().expect("thermal agent is not enabled");
        thermal_agent.send_now(port::Input::new(thermal::Input::JetsonCpu(cpu)))?;
        thermal_agent.send_now(port::Input::new(thermal::Input::JetsonGpu(gpu)))?;
        self.status_request.temperature.cpu = f64::from(cpu);
        self.status_request.temperature.gpu = f64::from(gpu);
        self.status_request.temperature.ssd = f64::from(ssd);
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
            self.handle_mcu(plan, output?)?;
        }
        if let Poll::Ready(()) = self.button_long_press_timer.poll_unpin(cx) {
            tracing::debug!("Button long press");
            tracing::info!("Shutdown requested by the user");
            self.led.shutdown(true);
            return Ok(Some(Poll::Ready(())));
        }
        if let Poll::Ready(()) = self.button_double_press_timer.poll_unpin(cx) {
            tracing::debug!("Button double press");
            self.button_press_sequence.clear();
            handle_double_press(self);
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
    fn handle_mcu(&mut self, plan: &mut dyn Plan, output: mcu::main::Output) -> Result<()> {
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
                DATADOG
                    .gauge(
                        "orb.main.gauge.system.battery",
                        capacity.percentage.to_string(),
                        NO_TAGS,
                    )
                    .or_log();
                self.status_request.battery.level = f64::from(capacity.percentage);
                self.led.battery_capacity(capacity.percentage);
                plan.handle_mcu_battery_capacity(capacity)?;
            }
            mcu::main::Output::BatteryVoltage(battery_voltage) => {
                DATADOG
                    .gauge(
                        "orb.main.gauge.system.voltage",
                        battery_voltage.battery_cell1_mv.to_string(),
                        ["type:cell1"],
                    )
                    .or_log();
                DATADOG
                    .gauge(
                        "orb.main.gauge.system.voltage",
                        battery_voltage.battery_cell2_mv.to_string(),
                        ["type:cell2"],
                    )
                    .or_log();
                DATADOG
                    .gauge(
                        "orb.main.gauge.system.voltage",
                        battery_voltage.battery_cell3_mv.to_string(),
                        ["type:cell3"],
                    )
                    .or_log();
                DATADOG
                    .gauge(
                        "orb.main.gauge.system.voltage",
                        battery_voltage.battery_cell4_mv.to_string(),
                        ["type:cell4"],
                    )
                    .or_log();
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
                    if !self.status_request.battery.is_charging {
                        // TODO: Improve the sound before uncommenting
                        // self.sound
                        //     .build(sound::Type::Melody(Melody::BatteryPlugIn))?
                        //     .priority(2)
                        //     .push()?;
                    }
                    DATADOG.incr("orb.main.count.system.battery.is_charging", NO_TAGS).or_log();
                } else {
                    DATADOG.incr("orb.main.count.system.battery.is_not_charging", NO_TAGS).or_log();
                }
                self.led.battery_is_charging(battery_is_charging);
                self.status_request.battery.is_charging = battery_is_charging;
            }
            mcu::main::Output::MotorRange(motor_range) => {
                log_mcu_motor_range(&motor_range);
                plan.handle_mcu_motor_range(motor_range)?;
            }
            mcu::main::Output::FanStatus(status) => {
                if status.fan_id == orb_messages::mcu_main::fan_status::FanId::Main as i32 {
                    DATADOG
                        .gauge(
                            "orb.main.gauge.system.fan_main_rpm",
                            status.measured_speed_rpm.to_string(),
                            NO_TAGS,
                        )
                        .or_log();
                } else if status.fan_id == orb_messages::mcu_main::fan_status::FanId::Aux as i32 {
                    DATADOG
                        .gauge(
                            "orb.main.gauge.system.fan_aux_rpm",
                            status.measured_speed_rpm.to_string(),
                            NO_TAGS,
                        )
                        .or_log();
                }
                plan.handle_fan_status(status)?;
            }
            mcu::main::Output::AmbientLight(als) => {
                DATADOG
                    .gauge("orb.main.gauge.system.als", als.ambient_light_lux.to_string(), NO_TAGS)
                    .or_log();
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
                    DATADOG
                        .incr("orb.main.count.system.mcu_fatal", [format!(
                            "main_mcu_reason:{reason:?}",
                        )])
                        .or_log();
                } else {
                    tracing::error!("Unable to parse fatal MCU error");
                }
            }
            mcu::main::Output::Versions(versions) => {
                DATADOG
                    .incr("orb.main.count.global.version", [
                        format!("main_mcu:{}", versions.primary),
                        format!("main_mcu_secondary:{}", versions.secondary),
                        format!("orb_core:{}", *GIT_VERSION),
                    ])
                    .or_log();
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
                    orb_messages::mcu_main::hardware_diagnostic::Source::try_from(diag.source);
                let status =
                    orb_messages::mcu_main::hardware_diagnostic::Status::try_from(diag.status);
                if let (Ok(component), Ok(status)) = (component, status) {
                    DATADOG
                        .incr("orb.main.count.global.hardware.component_diag", [
                            format!("type:{:?}", component.as_str_name().to_lowercase()),
                            format!("status:{:?}", status.as_str_name().to_lowercase()),
                        ])
                        .or_log();
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
        }
        Ok(())
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
        DATADOG
            .gauge(
                "orb.main.gauge.system.connectivity.ping_time",
                if report.is_no_internet() { String::new() } else { report.lag.to_string() },
                NO_TAGS,
            )
            .or_log();
        DATADOG
            .gauge(
                "orb.main.gauge.system.connectivity.rssi",
                if report.is_no_wlan() { String::new() } else { report.rssi.to_string() },
                NO_TAGS,
            )
            .or_log();
        if report.is_no_internet() {
            self.led.no_internet();
        } else if report.is_slow_internet() {
            self.led.slow_internet();
        } else {
            self.led.good_internet();
        }
        if report.is_no_wlan() {
            self.led.no_wlan();
        } else if report.is_slow_wlan() {
            self.led.slow_wlan();
        } else {
            self.led.good_wlan();
        }
        self.status_request.wifi.quality.signal_level = report.rssi;
        self.status_request.wifi.ssid = report.ssid;
    }
}

fn log_mcu_success_ack(input: &mcu::main::Input) {
    match input {
        mcu::main::Input::IrLedDuration(ir_led_duration) => {
            DATADOG
                .gauge("orb.main.gauge.system.ir_led_duration", ir_led_duration.to_string(), [
                    "type:general",
                ])
                .or_log();
        }
        mcu::main::Input::IrLedDuration740nm(ir_led_duration) => {
            DATADOG
                .gauge("orb.main.gauge.system.ir_led_duration", ir_led_duration.to_string(), [
                    "type:740",
                ])
                .or_log();
        }
        mcu::main::Input::UserLedBrightness(user_led_brightness) => {
            DATADOG
                .gauge(
                    "orb.main.gauge.system.user_led_brightness",
                    user_led_brightness.to_string(),
                    NO_TAGS,
                )
                .or_log();
        }
        mcu::main::Input::LiquidLens(current) => {
            current.map(|current| -> Result<()> {
                DATADOG.gauge("orb.main.gauge.system.focus", current.to_string(), NO_TAGS).or_log();
                Ok(())
            });
        }
        mcu::main::Input::FrameRate(frame_rate) => {
            DATADOG
                .gauge("orb.main.gauge.system.frame_rate", frame_rate.to_string(), NO_TAGS)
                .or_log();
        }
        mcu::main::Input::Mirror(x, y) => {
            DATADOG
                .gauge("orb.main.gauge.mirror.angle", x.to_string(), ["type:horizontal"])
                .or_log();
            DATADOG.gauge("orb.main.gauge.mirror.angle", y.to_string(), ["type:vertical"]).or_log();
        }
        mcu::main::Input::FanSpeed(percentage) => {
            DATADOG
                .gauge("orb.main.gauge.system.fan_speed", percentage.to_string(), NO_TAGS)
                .or_log();
        }
        mcu::main::Input::TofCalibration(calibration) => {
            DATADOG
                .gauge("orb.main.gauge.system.tof_calibration", calibration.to_string(), NO_TAGS)
                .or_log();
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
        Command::new("espeak").args(espeak_args).arg(ip_string).spawn().ok();
    } else {
        Command::new("espeak").args(espeak_args).arg("Could not obtain IP address").spawn().ok();
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
                .send_now(port::Input::new(thermal::Input::MainMcu(output.temperature_c)))?;
            observer.status_request.temperature.main_mcu = f64::from(output.temperature_c);
            "main_mcu"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::SecurityMcu as i32 =>
        {
            thermal_agent
                .send_now(port::Input::new(thermal::Input::SecurityMcu(output.temperature_c)))?;
            observer.status_request.temperature.security_mcu = f64::from(output.temperature_c);
            "security_mcu"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::LiquidLens as i32 =>
        {
            thermal_agent
                .send_now(port::Input::new(thermal::Input::LiquidLens(output.temperature_c)))?;
            observer.status_request.temperature.liquid_lens = f64::from(output.temperature_c);
            "liquid_lens"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::FrontUnit as i32 =>
        {
            thermal_agent
                .send_now(port::Input::new(thermal::Input::FrontUnit(output.temperature_c)))?;
            observer.status_request.temperature.front_unit = f64::from(output.temperature_c);
            "front_unit"
        }
        output
            if output.source
                == orb_messages::mcu_main::temperature::TemperatureSource::MainAccelerometer
                    as i32 =>
        {
            thermal_agent.send_now(port::Input::new(thermal::Input::MainAccelerometer(
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
            thermal_agent.send_now(port::Input::new(thermal::Input::SecurityAccelerometer(
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
                .send_now(port::Input::new(thermal::Input::BackupBattery(output.temperature_c)))?;
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
                .send_now(port::Input::new(thermal::Input::Mainboard(output.temperature_c)))?;
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
        _ => "undefined_source",
    };
    DATADOG
        .gauge("orb.main.gauge.system.temperature", output.temperature_c.to_string(), [format!(
            "type:{temp_type}"
        )])
        .or_log();
    Ok(())
}

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
        _ => return,
    };
    DATADOG
        .gauge("orb.main.gauge.system.voltage", output.voltage_current_mv.to_string(), [
            format!("type:{name}"),
            "aggregation:current".to_string(),
        ])
        .or_log();
    DATADOG
        .gauge("orb.main.gauge.system.voltage", output.voltage_max_mv.to_string(), [
            format!("type:{name}"),
            "aggregation:max".to_string(),
        ])
        .or_log();
    DATADOG
        .gauge("orb.main.gauge.system.voltage", output.voltage_min_mv.to_string(), [
            format!("type:{name}"),
            "aggregation:min".to_string(),
        ])
        .or_log();
}

fn log_mcu_motor_range(output: &orb_messages::mcu_main::MotorRange) {
    match output {
        output
            if output.which_motor
                == orb_messages::mcu_main::motor_range::Motor::HorizontalPhi as i32 =>
        {
            DATADOG
                .gauge("orb.main.gauge.mirror.range", output.range_microsteps.to_string(), [
                    "type:horizontal",
                ])
                .or_log();
        }
        output
            if output.which_motor
                == orb_messages::mcu_main::motor_range::Motor::VerticalTheta as i32 =>
        {
            DATADOG
                .gauge("orb.main.gauge.mirror.range", output.range_microsteps.to_string(), [
                    "type:vertical",
                ])
                .or_log();
        }
        _ => {}
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
    DATADOG.incr("orb.main.count.system.battery_info", tags.as_slice()).or_log();
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
    DATADOG.incr("orb.main.count.system.battery_reset_reason", tags.as_slice()).or_log();
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

    DATADOG.incr("orb.main.count.system.battery_diagnostic", tags).or_log();

    let mut tags = battery_tags.to_owned();
    tags.push(String::from("type:current_ma"));
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_diagnostic",
            diagnostics.current_ma.to_string(),
            tags.as_slice(),
        )
        .or_log();
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
    DATADOG.incr("orb.main.count.system.battery_diagnostic_safety", tags.as_slice()).or_log();
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
    DATADOG.incr("orb.main.count.system.battery_diagnostic_pf", tags.as_slice()).or_log();
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
    DATADOG.incr("orb.main.count.system.battery_info_soc_statistics", tags).or_log();

    tags = battery_tags.to_owned();
    tags.push("type:number_of_button_presses".to_string());
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.number_of_button_presses.to_string(),
            tags.as_slice(),
        )
        .or_log();
    tags.pop();
    tags.push("type:number_of_insertions".to_string());
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.number_of_insertions.to_string(),
            tags.as_slice(),
        )
        .or_log();
    tags.pop();
    tags.push("type:number_of_charges".to_string());
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.number_of_charges.to_string(),
            tags.as_slice(),
        )
        .or_log();
    tags.pop();
    tags.push("type:number_of_written_flash_variables".to_string());
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.number_of_written_flash_variables.to_string(),
            tags.as_slice(),
        )
        .or_log();
}

fn log_battery_info_max_values(
    battery_info: &orb_messages::mcu_main::BatteryInfoMaxValues,
    battery_tags: &[String],
) {
    let mut tags = battery_tags.to_owned();
    tags.push(String::from("type:maximum_capacity_mah"));
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.maximum_capacity_mah.to_string(),
            tags.as_slice(),
        )
        .or_log();
    tags.pop();
    tags.push(String::from("type:maximum_cell_temp_decidegrees"));
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.maximum_cell_temp_decidegrees.to_string(),
            tags.as_slice(),
        )
        .or_log();
    tags.pop();
    tags.push(String::from("type:maximum_pcb_temp_decidegrees"));
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.maximum_pcb_temp_decidegrees.to_string(),
            tags.as_slice(),
        )
        .or_log();
    tags.pop();
    tags.push(String::from("type:maximum_charge_current_ma"));
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.maximum_charge_current_ma.to_string(),
            tags.as_slice(),
        )
        .or_log();
    tags.pop();
    tags.push(String::from("type:maximum_discharge_current_ma"));
    DATADOG
        .gauge(
            "orb.main.gauge.system.battery_info",
            battery_info.maximum_discharge_current_ma.to_string(),
            tags.as_slice(),
        )
        .or_log();
}
