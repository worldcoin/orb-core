//! Signup endpoint.

use crate::{
    backend::endpoints::MANAGEMENT_BACKEND_URL,
    identification::{get_orb_token, ORB_ID},
};
use eyre::Result;
use orb_wld_data_id::S3Region;
use serde::Deserialize;

#[allow(missing_docs)]
#[derive(Deserialize, Debug)]
pub struct Response {
    pub region: String,
}

/// Gets the AWS S3 region.
pub async fn request() -> Result<Response> {
    let request = super::client()?
        .get(format!("{}/api/v1/region", *MANAGEMENT_BACKEND_URL))
        .basic_auth(&*ORB_ID, Some(get_orb_token()?));
    tracing::debug!("Sending request {:#?}", request);
    let response = request.send().await?;
    tracing::debug!("Received response {:#?}", response);
    response.error_for_status_ref()?;
    let response = response.json::<Response>().await?;
    tracing::debug!("JSON body {:#?}", response);
    Ok(response)
}

/// Queries the current S3 region from the Orb Service.
pub async fn get_region() -> Result<(S3Region, String)> {
    match request().await {
        Ok(Response { region }) => Ok((region.parse()?, region)),
        Err(e) => {
            tracing::error!("Cannot determine S3 region, using Unknown: {}", e);
            Ok((S3Region::Unknown, "unknown".to_string()))
        }
    }
}
