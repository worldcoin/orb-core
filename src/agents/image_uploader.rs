//! Image upload agent

#[cfg(not(feature = "no-image-encryption"))]
use crate::agents::encrypt_and_seal;
use crate::{
    agents::{
        camera::{self, Frame},
        python::iris::NormalizedIris,
    },
    backend::{presigned_url::UrlType, upload_image},
    inst_elapsed,
    logger::{LogOnError, DATADOG, NO_TAGS},
    port,
    port::Port,
};
use async_trait::async_trait;
use eyre::Result;
use futures::{channel::oneshot, prelude::*, select};
use orb_wld_data_id::{ImageId, SignupId};
use std::{
    convert::{Infallible, TryInto},
    io::Cursor,
    time::{Duration, Instant, SystemTime},
};

/// Image upload agent
#[derive(Default, Debug)]
pub struct Agent;

/// Image upload agent inputs
#[allow(missing_docs)]
#[derive(Debug)]
pub enum Input {
    /// Upload the self-custody thumbnail for a specific signup. This op is used in a blocking context.
    UploadSelfCustodyThumbnail {
        tx: oneshot::Sender<ImageId>,
        signup_id: SignupId,
        self_custody_thumbnail: camera::rgb::Frame,
    },
    UploadIrisNormalizedImages {
        tx: oneshot::Sender<[Option<ImageId>; 4]>,
        signup_id: SignupId,
        left: Option<NormalizedIris>,
        right: Option<NormalizedIris>,
    },
}

impl Port for Agent {
    type Input = Input;
    type Output = Infallible;

    const INPUT_CAPACITY: usize = 0;
    const OUTPUT_CAPACITY: usize = 0;
}

impl super::Agent for Agent {
    const NAME: &'static str = "data-uploader";
}

#[async_trait]
impl super::AgentTask for Agent {
    #[allow(clippy::mut_mut)] // triggered by `select!` internals
    async fn run(mut self, mut port: port::Inner<Self>) -> Result<()> {
        loop {
            select! {
                input = port.next() => {
                    if let Some(input) = input {
                        self.handle_input(input.value).await?;
                    } else {
                        break;
                    }
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
    async fn handle_input(&mut self, input: Input) -> Result<()> {
        match input {
            Input::UploadSelfCustodyThumbnail { tx, signup_id, self_custody_thumbnail } => {
                let _ = tx
                    .send(upload_self_custody_thumbnail(signup_id, self_custody_thumbnail).await?);
            }
            Input::UploadIrisNormalizedImages { tx, signup_id, left, right } => {
                let _ = tx.send(upload_iris_normalized_images(signup_id, left, right).await?);
            }
        }
        Ok(())
    }
}

impl port::Outer<Agent> {
    /// Upload the self-custody thumbnail of a specific signup from memory.
    pub async fn upload_self_custody_thumbnail(
        &mut self,
        signup_id: SignupId,
        self_custody_thumbnail: camera::rgb::Frame,
    ) -> Result<ImageId> {
        let (tx, rx) = oneshot::channel();
        self.send(port::Input::new(Input::UploadSelfCustodyThumbnail {
            tx,
            signup_id,
            self_custody_thumbnail,
        }))
        .await?;
        Ok(rx.await?)
    }

    /// Upload the Iris normalized images of a specific signup from memory.
    pub async fn upload_iris_normalized_images(
        &mut self,
        signup_id: SignupId,
        left: Option<NormalizedIris>,
        right: Option<NormalizedIris>,
    ) -> Result<[Option<ImageId>; 4]> {
        let (tx, rx) = oneshot::channel();
        self.send(port::Input::new(Input::UploadIrisNormalizedImages {
            tx,
            signup_id,
            left,
            right,
        }))
        .await?;
        Ok(rx.await?)
    }
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
    DATADOG
        .timing(
            format!("orb.main.time.data_collection.upload.{dd_image_tag}.full"),
            inst_elapsed!(t),
            NO_TAGS,
        )
        .or_log();
    match response {
        Ok(()) => {
            DATADOG
                .incr(
                    format!("orb.main.count.data_collection.upload.success.{dd_image_tag}"),
                    NO_TAGS,
                )
                .or_log();
        }
        Err(e) => {
            DATADOG
                .incr(
                    format!("orb.main.count.data_collection.upload.error.{dd_image_tag}"),
                    NO_TAGS,
                )
                .or_log();
            tracing::error!("Uploading image {log_image_path} failed: {e}");
        }
    }
    Ok(())
}

async fn upload_self_custody_thumbnail(
    signup_id: SignupId,
    self_custody_thumbnail: camera::rgb::Frame,
) -> Result<ImageId> {
    tracing::info!("Start directly uploading self-custody thumbnail image");

    let image_id = ImageId::new(&signup_id, self_custody_thumbnail.timestamp());
    let mut data = Cursor::new(Vec::new());
    self_custody_thumbnail.write_png(&mut data, camera::FrameResolution::MAX)?;
    let data = data.into_inner();

    #[cfg(not(feature = "no-image-encryption"))]
    let data = encrypt_and_seal(&data);

    upload_image(&signup_id, &image_id, UrlType::Rgb, data, "direct.thumbnail", "direct.thumbnail")
        .await?;

    Ok(image_id)
}

async fn upload_iris_normalized_images(
    signup_id: SignupId,
    left: Option<NormalizedIris>,
    right: Option<NormalizedIris>,
) -> Result<[Option<ImageId>; 4]> {
    tracing::info!("Start directly uploading iris normalized images");

    let time_now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system time must be after UNIX EPOCH");
    let (left_image_id, left_mask_id, right_image_id, right_mask_id) = (
        ImageId::new(&signup_id, time_now + Duration::from_nanos(1)),
        ImageId::new(&signup_id, time_now + Duration::from_nanos(2)),
        ImageId::new(&signup_id, time_now + Duration::from_nanos(3)),
        ImageId::new(&signup_id, time_now + Duration::from_nanos(4)),
    );
    let mut output = [None, None, None, None];

    for (i, (n, tag, image_id, mask_id)) in [
        (left, "left", left_image_id, left_mask_id),
        (right, "right", right_image_id, right_mask_id),
    ]
    .iter()
    .enumerate()
    {
        if let Some(n) = n {
            let data_image = n.serialized_image();
            #[cfg(not(feature = "no-image-encryption"))]
            let data_image = encrypt_and_seal(&data_image);

            let data_mask = n.serialized_mask();
            #[cfg(not(feature = "no-image-encryption"))]
            let data_mask = encrypt_and_seal(&data_mask);

            upload_image(
                &signup_id,
                image_id,
                UrlType::NormalizedIrisImage,
                data_image,
                &format!("direct.normalized_image_{tag}"),
                &format!("direct.normalized_image_{tag}"),
            )
            .await?;
            upload_image(
                &signup_id,
                mask_id,
                UrlType::NormalizedIrisMask,
                data_mask,
                &format!("direct.normalized_mask_{tag}"),
                &format!("direct.normalized_mask_{tag}"),
            )
            .await?;
            output[i * 2] = Some(image_id.clone());
            output[(i * 2) + 1] = Some(mask_id.clone());
        }
    }
    Ok(output)
}
