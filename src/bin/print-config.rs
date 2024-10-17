use std::env;

use eyre::Result;
use orb::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    if env::var("ORB_ID").expect("ORB_ID environmental variable is missing").is_empty() {
        panic!("ORB_ID environmental variable is missing")
    }
    if env::var("DBUS_SESSION_BUS_ADDRESS")
        .expect("DBUS_SESSION_BUS_ADDRESS environmental variable is missing")
        .is_empty()
    {
        panic!("DBUS_SESSION_BUS_ADDRESS environmental variable is missing")
    }

    orb::short_lived_token::wait_for_token().await;
    let config = Config::download().await?;
    println!("{}", serde_json::to_string_pretty(&config).unwrap());

    Ok(())
}
