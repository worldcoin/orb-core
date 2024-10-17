//! Image upload agent
//!
//! This agent will use the files saved to disk by [`crate::agents::image_notary`].
//!
//! It is only enabled with the `internal-data-acquisition` feature.

use crate::{
    backend::{presigned_url::UrlType, upload_image},
    consts::DATA_ACQUISITION_BASE_DIR,
    dd_gauge, dd_incr, dd_timing, ssd,
};
use agentwire::port::{self, Port};
use bytesize::ByteSize;
use eyre::{Error, Result};
use futures::{future::Fuse, pin_mut, prelude::*, select};
use orb_wld_data_id::{ImageId, SignupId};
use rand::{prelude::SliceRandom, thread_rng};
use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    pin::Pin,
    time::{Duration, Instant, SystemTime},
};
use tokio::{fs, task::spawn_blocking, time::sleep};

type UploadImages = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// Image upload agent
#[derive(Default, Debug)]
pub struct Agent;

/// Image upload agent inputs
#[allow(missing_docs)]
#[derive(Debug)]
pub enum Input {
    /// Start uploading all currently available images.
    StartUpload { image_upload_delay: Duration },
    /// Stop upload - killing any pending requests.
    PauseUpload,
}

impl Port for Agent {
    type Input = Input;
    type Output = Infallible;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl agentwire::Agent for Agent {
    const NAME: &'static str = "image-uploader";
}

impl agentwire::agent::Task for Agent {
    type Error = Error;

    #[allow(clippy::mut_mut)] // triggered by `select!` internals
    async fn run(mut self, mut port: port::Inner<Self>) -> Result<(), Self::Error> {
        let network_request = Fuse::terminated();
        pin_mut!(network_request);
        loop {
            select! {
                input = port.next() => {
                    if let Some(input) = input {
                        self.handle_input(input.value, &mut network_request).await?;
                    } else {
                        break;
                    }
                }
                _ = network_request => {
                    tracing::info!("Data upload complete.");
                }
            }
        }
        Ok(())
    }
}

/// NOTE: This agent is not thread-safe and it might create race conditions if
/// it used from multiple places simultaneously. Currently we use this agent
/// only in 2 places. During idle state, and during fraud detection. Both these
/// Orb states are mutually exclusive with respect to execution.
impl Agent {
    #[allow(clippy::unused_async)]
    async fn handle_input(
        &mut self,
        input: Input,
        network_request: &mut Pin<&mut Fuse<UploadImages>>,
    ) -> Result<()> {
        match input {
            Input::StartUpload { image_upload_delay } => {
                let box_var: UploadImages = Box::pin(upload_all_signup_images(image_upload_delay));
                network_request.set(box_var.fuse());
            }
            Input::PauseUpload => {
                //Immediately drop any pending request.
                network_request.set(Fuse::terminated());
            }
        }
        Ok(())
    }
}

async fn upload_saved_images(
    signup_dir: &Path,
    image_dir_name: &str,
    signup_id: &SignupId,
    presigned_url_type: UrlType,
) -> Result<()> {
    let image_dir = signup_dir.join(image_dir_name);
    if !image_dir.is_dir() {
        tracing::warn!("The directory {:?} does not exist, skipping image upload", image_dir);
        return Ok(());
    }
    let mut paths = Vec::new();
    ssd::perform_async(async {
        let mut dir_reader = fs::read_dir(&image_dir).await?;
        while let Some(entry) = dir_reader.next_entry().await? {
            let path = entry.path();
            if path.extension().map_or(false, |path| path == "png") {
                paths.push(path);
            }
        }
        Ok(())
    })
    .await;
    for path in paths {
        let image_id = ImageId::from_image_path(&path)?;
        let img_data = ssd::perform_async(async { fs::read(&path).await }).await;
        let Some(img_data) = img_data else {
            continue;
        };
        upload_image(
            signup_id,
            &image_id,
            presigned_url_type,
            img_data,
            &path.display().to_string(),
            image_dir_name,
        )
        .await?;
    }
    ssd::perform_async(async { fs::remove_dir_all(image_dir).await }).await;
    Ok(())
}

async fn upload_image(
    signup_id: &SignupId,
    image_id: &ImageId,
    presigned_url_type: UrlType,
    img_data: Vec<u8>,
    log_image_path: &str,
    dd_image_tag: &str,
) -> Result<()> {
    tracing::info!("Uploading image: {log_image_path}");
    let t = Instant::now();
    let response =
        upload_image::request(signup_id, image_id, presigned_url_type, img_data, dd_image_tag)
            .await;
    dd_timing!("main.time.data_acquisition.upload" + format!("{}.full", dd_image_tag), t);
    match response {
        Ok(()) => {
            dd_incr!("main.count.data_acquisition.upload.success" + format!("{}", dd_image_tag));
        }
        Err(e) => {
            dd_incr!("main.count.data_acquisition.upload.error" + format!("{}", dd_image_tag));
            tracing::error!("Uploading image {log_image_path} failed: {e}");
        }
    }
    Ok(())
}

async fn upload_signup_images(signup_dir: &Path) -> Result<()> {
    // extract last element of signup directory path as String
    let signup_id = SignupId::from_signup_dir(signup_dir)?;
    let t0 = Instant::now();
    upload_saved_images(signup_dir, "ir_camera", &signup_id, UrlType::Ir).await?;
    dd_timing!("main.time.data_acquisition.upload.batch.ir_camera", t0);
    let t1 = Instant::now();
    upload_saved_images(signup_dir, "rgb_camera", &signup_id, UrlType::Rgb).await?;
    dd_timing!("main.time.data_acquisition.upload.batch.rgb_camera", t1);
    let t2 = Instant::now();
    upload_saved_images(signup_dir, "ir_face", &signup_id, UrlType::IrFace).await?;
    dd_timing!("main.time.data_acquisition.upload.batch.ir_face", t2);
    let t3 = Instant::now();
    upload_saved_images(signup_dir, "thermal", &signup_id, UrlType::Thermal).await?;
    dd_timing!("main.time.data_acquisition.upload.batch.thermal", t3);
    upload_identification_images_impl(signup_id).await?;
    dd_timing!("main.time.data_acquisition.upload.batch.full_signup", t0);
    ssd::perform_async(async { fs::remove_dir_all(signup_dir).await }).await;
    Ok(())
}

/// Return signup paths with a recency-biased ordering.
/// Signups done within the last 24 hours come first, in random order.
/// Signups older than 24 hours are simply sorted, newest to oldest.
async fn get_signup_paths() -> impl Iterator<Item = PathBuf> {
    let mut age_and_path = Vec::new();
    ssd::perform_async(async {
        let mut dir_reader = fs::read_dir(DATA_ACQUISITION_BASE_DIR).await?;
        while let Some(entry) = dir_reader.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                let age = SystemTime::now()
                    .duration_since(entry.metadata().await?.modified()?)
                    .unwrap_or(Duration::MAX);
                age_and_path.push((age, path));
            }
        }
        age_and_path.sort();
        age_and_path.reverse();
        if let Some(one_day_index) =
            age_and_path.iter().position(|&(age, _)| age > Duration::from_secs(60 * 60 * 24))
        {
            age_and_path.split_at_mut(one_day_index).0.shuffle(&mut thread_rng());
        }
        Ok(())
    })
    .await;
    age_and_path.into_iter().map(|(_, path)| path)
}

fn log_data_to_upload_left() -> Result<()> {
    if let Some(ssd::Stats { available_space, signups, documents, documents_size }) = ssd::stats()?
    {
        dd_gauge!("main.gauge.system.ssd.available_space", available_space.to_string());
        dd_gauge!("main.gauge.data_acquisition.to_upload.signup", signups.to_string());
        dd_gauge!("main.gauge.data_acquisition.to_upload.document", documents.to_string());
        tracing::debug!(
            "Image Uploader: Available SSD space: {}, Number of signups to upload: {signups}, \
             Number of documents to upload: {documents}, Total size of documents to upload {}",
            ByteSize::b(available_space),
            if let Ok(size) = documents_size {
                ByteSize::b(size).to_string()
            } else {
                "-1".to_owned()
            },
        );
    }
    Ok(())
}

async fn upload_all_signup_images(image_upload_delay: Duration) -> Result<()> {
    // This long delay helps prevent uploading images while the Orb is connected to a hotspot (i.e. while doing signups)
    // Generally, Orbs in the field will only be in the idle state beyond "image_upload_delay" if they are connected to
    // Wifi to upload overnight
    spawn_blocking(log_data_to_upload_left).await??;
    sleep(image_upload_delay).await;
    for path in get_signup_paths().await {
        spawn_blocking(log_data_to_upload_left).await??;
        tracing::info!("Starting to upload images from {}", path.display());
        if let Err(err) = upload_signup_images(&path).await {
            tracing::error!("Error uploading signup images from {}: {}", path.display(), err);
        }
    }
    Ok(())
}

async fn upload_identification_images_impl(signup_id: SignupId) -> Result<()> {
    let signup_dir = Path::new(DATA_ACQUISITION_BASE_DIR).join(signup_id.to_string());
    spawn_blocking(log_data_to_upload_left).await??;
    tracing::info!("Starting to upload identification images from {}", signup_dir.display());
    let identification_dir = signup_dir.join("identification");
    if identification_dir.is_dir() {
        upload_saved_images(&identification_dir.join("ir"), "left", &signup_id, UrlType::Ir)
            .await?;
        upload_saved_images(&identification_dir.join("ir"), "right", &signup_id, UrlType::Ir)
            .await?;
        upload_saved_images(&identification_dir.join("rgb"), "left", &signup_id, UrlType::Rgb)
            .await?;
        upload_saved_images(&identification_dir.join("rgb"), "right", &signup_id, UrlType::Rgb)
            .await?;
        upload_saved_images(
            &identification_dir.join("rgb"),
            "self_custody_candidate",
            &signup_id,
            UrlType::Rgb,
        )
        .await?;
    } else {
        tracing::warn!(
            "The directory {:?} does not exist, skipping image upload",
            identification_dir
        );
    }
    Ok(())
}
