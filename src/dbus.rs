//! DBus interfaces that are used by orb core to notify other processes of events.

#![allow(missing_docs)]
use zbus::{dbus_interface, dbus_proxy, Result, SignalContext};

/// `Signup` is a DBus interface that emits signals related to signup events.
///
/// At the moment, the only signal emitted is for signups starting.
pub struct Signup;

#[dbus_interface(name = "org.worldcoin.OrbCore1.Signup")]
impl Signup {
    /// Emits a signal when a signup is started.
    #[dbus_interface(signal)]
    pub async fn signup_started(ctxt: &SignalContext<'_>) -> Result<()>;

    /// Emits a signal when a signup is completed
    #[dbus_interface(signal)]
    pub async fn signup_finished(ctx: &SignalContext<'_>, success: bool) -> Result<()>;
}

/// AuthToken is a DBus interface that exposes currently valid backend token via
/// 'token' property.
///
/// When token is refreshed, the property is updated and a signal is emitted.
#[dbus_proxy(
    default_service = "org.worldcoin.AuthTokenManager1",
    default_path = "/org/worldcoin/AuthTokenManager1",
    interface = "org.worldcoin.AuthTokenManager1"
)]
trait AuthToken {
    #[dbus_proxy(property)]
    fn token(&self) -> zbus::Result<String>;
}

#[cfg(test)]
mod tests {
    use super::Signup;
    use zbus::Interface as _;

    #[test]
    fn signup_interface_name_matches_const() {
        assert_eq!(crate::consts::DBUS_SIGNUP_INTERFACE_NAME, &*Signup::name());
    }
}

#[dbus_proxy(
    default_service = "org.worldcoin.OrbSupervisor1",
    default_path = "/org/worldcoin/OrbSupervisor1/Manager",
    interface = "org.worldcoin.OrbSupervisor1.Manager"
)]
pub trait Supervisor {
    #[dbus_proxy(name = "ScheduleShutdown")]
    fn schedule_shutdown(&self, kind: &str, when: u64) -> zbus::Result<()>;
}

#[dbus_proxy(
    default_service = "org.worldcoin.OrbUiState1",
    default_path = "/org/worldcoin/OrbUiState1",
    interface = "org.worldcoin.OrbUiState1"
)]
pub trait SignupState {
    fn orb_signup_state_event(&self, serialized_event: String) -> zbus::Result<()>;
}
