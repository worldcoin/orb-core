#![warn(unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic)]

use agentwire::{port, BrokerFlow};
use eyre::{bail, Result};
use futures::{channel::mpsc, prelude::*};
use orb::{
    agents::{camera, mirror},
    async_main,
    brokers::{Orb, OrbPlan},
    calibration::Calibration,
    config::Config,
    consts::{
        CALIBRATION_FILE_PATH, IR_CAMERA_FRAME_RATE, RGB_FPS, RGB_REDUCED_HEIGHT, RGB_REDUCED_WIDTH,
    },
    ext::mpsc::SenderExt as _,
    mcu, monitor,
    plans::biometric_capture::{IR_TARGET_MEAN, MIN_SHARPNESS},
    ui::{self, Engine},
};
use std::{
    io::{prelude::*, stdin, stdout, Stdout},
    sync::Arc,
    task::{Context, Poll},
    thread,
};
use termion::{
    event::Key,
    input::TermRead,
    raw::{IntoRawMode, RawTerminal},
};
use tokio::sync::Mutex;

enum Command {
    Recalibrate(Calibration),
    SwitchEye(bool),
    Quit(Calibration),
    EyePidControllerToggle(bool),
    ThermalCameraToggle(bool),
    ThermalCameraCalibrate,
}

struct Plan {
    command_rx: mpsc::Receiver<Command>,
    command: Option<Command>,
}

impl OrbPlan for Plan {
    fn poll_extra(&mut self, _orb: &mut Orb, cx: &mut Context<'_>) -> Result<BrokerFlow> {
        match self.command_rx.poll_next_unpin(cx) {
            Poll::Ready(command @ Some(_)) => {
                self.command = command;
                Ok(BrokerFlow::Break)
            }
            Poll::Ready(None) => bail!("channel closed unexpectedly"),
            Poll::Pending => Ok(BrokerFlow::Continue),
        }
    }
}

impl Plan {
    #[allow(clippy::unused_async)]
    async fn new(calibration: Calibration) -> Self {
        let (command_tx, command_rx) = mpsc::channel(100);
        spawn_command_thread(command_tx, calibration);
        Self { command_rx, command: None }
    }

    async fn run(&mut self, orb: &mut Orb) -> Result<()> {
        orb.main_mcu.send(mcu::main::Input::FrameRate(IR_CAMERA_FRAME_RATE)).await?;
        orb.disable_ir_led().await?;
        orb.main_mcu.send(mcu::main::Input::LiquidLens(None)).await?;
        #[cfg(feature = "livestream")]
        orb.enable_livestream()?;
        orb.enable_ir_net().await?;
        orb.enable_rgb_net(true).await?;
        orb.start_ir_eye_camera().await?;
        orb.start_ir_face_camera().await?;
        orb.start_rgb_camera(RGB_FPS).await?;
        orb.start_thermal_camera().await?;
        orb.start_depth_camera().await?;
        orb.start_ir_auto_exposure(IR_TARGET_MEAN).await?;
        orb.start_ir_auto_focus(MIN_SHARPNESS, false).await?;
        orb.enable_eye_tracker()?;
        orb.enable_mirror()?;
        orb.set_fisheye(RGB_REDUCED_WIDTH, RGB_REDUCED_HEIGHT, false).await?;
        let mut calibration = loop {
            orb.run(self).await?;
            match self.command.take().expect("command should be set") {
                Command::Quit(calibration) => {
                    break calibration;
                }
                Command::Recalibrate(calibration) => {
                    orb.mirror
                        .enabled()
                        .expect("mirror should be enabled")
                        .send(port::Input::new(mirror::Command::Recalibrate(calibration)))
                        .await?;
                }
                Command::SwitchEye(target_left_eye) => {
                    orb.set_target_left_eye(target_left_eye).await?;
                }
                Command::EyePidControllerToggle(enable_eye_pid_controller) => {
                    if enable_eye_pid_controller {
                        orb.enable_eye_pid_controller()?;
                    } else if orb.eye_pid_controller.is_enabled() {
                        orb.stop_eye_pid_controller().await?;
                    }
                }
                Command::ThermalCameraToggle(enable_thermal_camera) => {
                    if enable_thermal_camera {
                        orb.start_thermal_camera().await?;
                    } else {
                        orb.stop_thermal_camera().await?;
                    }
                }
                Command::ThermalCameraCalibrate => {
                    if let Some(thermal_camera) = orb.thermal_camera.enabled() {
                        thermal_camera
                            .send(port::Input::new(camera::thermal::Command::FscCalibrate))
                            .await?;
                    }
                }
            }
        };
        orb.stop_eye_tracker().await?;
        if orb.eye_pid_controller.is_enabled() {
            if let Some(mirror_offset) = orb.stop_eye_pid_controller().await? {
                calibration.mirror.phi_offset_degrees += mirror_offset.phi_degrees;
                calibration.mirror.theta_offset_degrees += mirror_offset.theta_degrees;
            }
        }
        orb.stop_ir_auto_focus().await?;
        #[cfg(feature = "livestream")]
        orb.disable_livestream();
        if orb.thermal_camera.is_enabled() {
            orb.stop_thermal_camera().await?;
        }
        orb.stop_depth_camera().await?;
        orb.stop_rgb_camera().await?;
        orb.stop_ir_face_camera().await?;
        orb.stop_ir_eye_camera().await?;
        orb.disable_agents();
        calibration.store(CALIBRATION_FILE_PATH).await?;
        Ok(())
    }
}

fn spawn_command_thread(mut command_tx: mpsc::Sender<Command>, mut calibration: Calibration) {
    fn recalibrate(
        stdout: &RawTerminal<Stdout>,
        command_tx: &mut mpsc::Sender<Command>,
        calibration: &Calibration,
    ) {
        command_tx.send_now(Command::Recalibrate(calibration.clone())).unwrap();
        stdout.suspend_raw_mode().unwrap();
        println!("{}", serde_json::to_string_pretty(&calibration).unwrap());
        stdout.activate_raw_mode().unwrap();
    }
    thread::spawn(move || {
        let stdin = stdin();
        let mut stdout = stdout().into_raw_mode().unwrap();
        write!(
            stdout,
            "{}{}q to exit, arrow keys to move, space to switch the active eye, p to run \
             eye_pid_controller, t to toggle thermal camera, T to calibrate thermal camera.{}",
            termion::clear::All,
            termion::cursor::Goto(1, 1),
            termion::cursor::Goto(1, 3)
        )
        .unwrap();
        stdout.flush().unwrap();
        let mut target_left_eye = false;
        let mut enable_eye_pid_controller = false;
        let mut enable_thermal_camera = true;
        for c in stdin.keys() {
            write!(stdout, "{}{}", termion::cursor::Goto(1, 3), termion::clear::AfterCursor)
                .unwrap();
            match c.unwrap() {
                Key::Char('q') => {
                    drop(stdout);
                    command_tx.send_now(Command::Quit(calibration)).unwrap();
                    break;
                }
                Key::Char(' ') => {
                    target_left_eye = !target_left_eye;
                    command_tx.send_now(Command::SwitchEye(target_left_eye)).unwrap();
                }
                Key::Char('p') => {
                    enable_eye_pid_controller = !enable_eye_pid_controller;
                    command_tx
                        .send_now(Command::EyePidControllerToggle(enable_eye_pid_controller))
                        .unwrap();
                }
                Key::Char('t') => {
                    enable_thermal_camera = !enable_thermal_camera;
                    command_tx
                        .send_now(Command::ThermalCameraToggle(enable_thermal_camera))
                        .unwrap();
                }
                Key::Char('T') => {
                    command_tx.send_now(Command::ThermalCameraCalibrate).unwrap();
                }
                Key::Left => {
                    calibration.mirror.phi_offset_degrees -= 0.1;
                    recalibrate(&stdout, &mut command_tx, &calibration);
                }
                Key::Right => {
                    calibration.mirror.phi_offset_degrees += 0.1;
                    recalibrate(&stdout, &mut command_tx, &calibration);
                }
                Key::Up => {
                    calibration.mirror.theta_offset_degrees -= 0.1;
                    recalibrate(&stdout, &mut command_tx, &calibration);
                }
                Key::Down => {
                    calibration.mirror.theta_offset_degrees += 0.1;
                    recalibrate(&stdout, &mut command_tx, &calibration);
                }
                _ => {}
            }
            write!(stdout, "{}", termion::cursor::Goto(1, 3)).unwrap();
            stdout.flush().unwrap();
        }
    });
}

fn main() -> Result<()> {
    async_main(run())
}

async fn run() -> Result<()> {
    let ui = ui::Jetson::spawn();
    let config = Arc::new(Mutex::new(Config::load_or_default().await));
    config.lock().await.propagate_to_ui(&ui);
    let cpu_monitor = Box::new(monitor::cpu::Jetson::spawn());
    let main_mcu = mcu::main::Jetson::spawn()?;
    let net_monitor = monitor::net::Jetson::spawn(Arc::clone(&config))?;
    ui.pause();
    orb::short_lived_token::wait_for_token().await;
    Config::download_and_store(Arc::clone(&config)).await?;
    let mut orb = Box::pin(
        Orb::builder()
            .config(config)
            .ui(Box::new(ui))
            .main_mcu(Box::new(main_mcu))
            .net_monitor(Box::new(net_monitor))
            .cpu_monitor(cpu_monitor)
            .build(),
    )
    .await?;
    Plan::new(orb.calibration().clone()).await.run(&mut orb).await?;
    Ok(())
}
