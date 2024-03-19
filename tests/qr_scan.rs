#[macro_use]
mod common;

use futures::prelude::*;
use orb::{agents::camera, brokers::Orb, plans::qr_scan, port};
use orb_qr_link::DataPolicy;
use std::{fs::File, time::Duration};
use tokio::task;

broker_test!(test_qr_scan_timeout, test_qr_scan_timeout_impl, 60000);
async fn test_qr_scan_timeout_impl() {
    let (_rgb_camera_fake_port_inner, rgb_camera_fake_port_outer) = port::new();
    let mut orb =
        Orb::builder().rgb_camera_fake_port(rgb_camera_fake_port_outer).build().await.unwrap();
    let r = qr_scan::Plan::<qr_scan::operator::Data>::new(Some(Duration::from_millis(50)))
        .run(&mut orb)
        .await
        .unwrap();
    assert!(matches!(r, Err(qr_scan::ScanError::Timeout)));
}

broker_test!(test_qr_scan_raw_qr, test_qr_scan_raw_qr_impl, 60000);
async fn test_qr_scan_raw_qr_impl() {
    let (mut rgb_camera_fake_port_inner, rgb_camera_fake_port_outer) = port::new();
    let qr = task::spawn_blocking(|| {
        camera::rgb::Frame::read_png(File::open("tests/qr_scan/raw_qr.png").unwrap()).unwrap()
    })
    .await
    .unwrap();
    task::spawn(async move {
        while let Some(command) = rgb_camera_fake_port_inner.next().await {
            if let camera::rgb::Command::Fisheye { .. } = command.value {
                break;
            }
        }
        rgb_camera_fake_port_inner.send(port::Output::new(qr)).await.unwrap();
    });
    let mut orb =
        Orb::builder().rgb_camera_fake_port(rgb_camera_fake_port_outer).build().await.unwrap();
    let r = qr_scan::Plan::<qr_scan::user::Data>::new(None).run(&mut orb).await.unwrap().unwrap();
    assert_eq!(r.user_id, "cf37084e-5087-484c-b5a3-3ca3c34016d1");
    assert_eq!(r.data_policy.unwrap(), DataPolicy::FullDataOptIn);
}
