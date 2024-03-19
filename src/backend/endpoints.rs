//! Endpoints for backend services the orb talks to.

use once_cell::sync::Lazy;
use orb_endpoints::Backend;

/// The backend that orb-core will talk to. Based on env vars.
pub static BACKEND: Lazy<Backend> =
    Lazy::new(Backend::from_env_or_build_type::<{ cfg!(feature = "stage") }>);

// TODO: Consolidate all of this in orb_endpoints crate

macro_rules! make_urls {
    ($(
        $(#[$($attrs:tt)*])*
        $vis:vis static $ident:ident = $s:literal;
    )+) => {$(
        $(#[$($attrs)*])*
        $vis static $ident: Lazy<String> = Lazy::new(|| {
            let backend_channel = match *BACKEND {
                Backend::Prod => "orb",
                Backend::Staging => "stage.orb",
            };
            format!($s, backend_channel)
        });
    )+};
}

make_urls! {
    /// Data backend URL - writes to regionalized S3.
    pub static DATA_BACKEND_URL = "https://data.{}.worldcoin.org";

    /// Management backend URL - writes to regionalized S3.
    pub static MANAGEMENT_BACKEND_URL = "https://management.{}.worldcoin.org";

    /// Signup backend URL.
    pub static SIGNUP_BACKEND_URL="https://signup.{}.worldcoin.org";

    /// Host for network monitoring purposes.
    pub static NETWORK_MONITOR_HOST="signup.{}.worldcoin.org";
}
