//! Audio support.

use crate::{
    config::{BasicConfig, Config},
    consts::{SOUNDS_DIR, SOUND_CARD_NAME},
    monitor,
};
use dashmap::DashMap;
use eyre::{Result, WrapErr};
use futures::prelude::*;
use orb_sound::{Queue, SoundBuilder};
use serde::{Deserialize, Serialize};
use std::{
    fmt::{self, Debug},
    io::Cursor,
    path::Path,
    pin::Pin,
    sync::Arc,
};
use tokio::{fs, sync::Mutex};

const CPU_LOAD_THRESHOLD: f64 = 0.95;

/// Sound queue.
pub trait Player: Debug + Send {
    /// Loads sound files for the given language from the file system.
    fn load_sound_files(
        &self,
        language: Option<&str>,
        ignore_missing_sounds: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>>;

    /// Creates a new sound builder object.
    fn build(&mut self, sound_type: Type) -> Result<SoundBuilder>;

    /// Returns a new handler to the shared queue.
    fn clone(&self) -> Box<dyn Player>;
}

/// Sound queue for the Orb hardware.
pub struct Jetson {
    queue: Arc<Queue>,
    sound_files: Arc<DashMap<Type, SoundFile>>,
    cpu_monitor: Box<dyn monitor::cpu::Monitor>,
}

/// Sound queue which does nothing.
#[derive(Debug)]
pub struct Fake;

/// Available sound types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "sound_type", content = "value")]
pub enum Type {
    /// Sound type for voices.
    Voice(Voice),
    /// Sound type for melodies.
    Melody(Melody),
}

macro_rules! sound_enum {
    (
        $(#[$($enum_attrs:tt)*])*
        $vis:vis enum $name:ident {
            $(
                #[sound_enum(file = $file:expr)]
                $(#[$($sound_attrs:tt)*])*
                $sound:ident,
            )*
        }
    ) => {
        $(#[$($enum_attrs)*])*
        #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
        $vis enum $name {
            $(
                $(#[$($sound_attrs)*])*
                $sound,
            )*
        }

        impl $name {
            async fn load_sound_files(
                sound_files: &DashMap<Type, SoundFile>,
                language: Option<&str>,
                ignore_missing_sounds: bool,
            ) -> Result<()> {
                $(
                    sound_files.insert(
                        Type::$name(Self::$sound),
                        load_sound_file($file, language, ignore_missing_sounds).await?,
                    );
                )*
                Ok(())
            }
        }
    };
}

sound_enum! {
    /// Available voices.
    #[allow(missing_docs)]
    pub enum Voice {
        #[sound_enum(file = "silence")]
        Silence,
        #[sound_enum(file = "voice_show_wifi_hotspot_qr_code")]
        ShowWifiHotspotQrCode,
        #[sound_enum(file = "voice_iris_move_farther")]
        MoveFarther,
        #[sound_enum(file = "voice_iris_move_closer")]
        MoveCloser,
        #[sound_enum(file = "voice_overheating")]
        Overheating,
        #[sound_enum(file = "voice_please_put_the_calibration_target_in_the_frame")]
        PutCalibrationTarget,
        #[sound_enum(file = "voice_whole_pattern_is_visible")]
        CalibrationTargetVisible,
        #[sound_enum(file = "voice_please_do_not_move_the_calibration_target")]
        DoNotMoveCalibrationTarget,
        #[sound_enum(file = "voice_verification_not_successful_please_try_again")]
        VerificationNotSuccessfulPleaseTryAgain,
        #[sound_enum(file = "voice_qr_code_invalid")]
        QrCodeInvalid,
        #[sound_enum(file = "voice_internet_connection_too_slow_to_perform_signups")]
        InternetConnectionTooSlowToPerformSignups,
        #[sound_enum(file = "voice_internet_connection_too_slow_signups_might_take_longer_than_expected")]
        InternetConnectionTooSlowSignupsMightTakeLonger,
        #[sound_enum(file = "voice_wrong_qr_code_format")]
        WrongQrCodeFormat,
        #[sound_enum(file = "voice_timeout")]
        Timeout,
        #[sound_enum(file = "voice_server_error")]
        ServerError,
        #[sound_enum(file = "voice_face_not_found")]
        FaceNotFound,
        #[sound_enum(file = "voice_test_firmware_warning")]
        TestFirmwareWarning,
        #[sound_enum(file = "voice_please_do_not_shutdown")]
        PleaseDontShutDown,
    }
}

sound_enum! {
    /// Available melodies.
    #[allow(missing_docs)]
    pub enum Melody {
        #[sound_enum(file = "sound_bootup")]
        BootUp,
        #[sound_enum(file = "sound_powering_down")]
        PoweringDown,
        #[sound_enum(file = "sound_qr_code_capture")]
        QrCodeCapture,
        #[sound_enum(file = "sound_signup_success")]
        SignupSuccess,
        #[sound_enum(file = "sound_overheating")]
        Overheating, // TODO: Play when the overheating logic is implemented.
        #[sound_enum(file = "sound_internet_connection_successful")]
        InternetConnectionSuccessful,
        #[sound_enum(file = "sound_qr_load_success")]
        QrLoadSuccess,
        #[sound_enum(file = "sound_user_qr_load_success")]
        UserQrLoadSuccess,
        #[sound_enum(file = "sound_iris_scan_success")]
        IrisScanSuccess,
        #[sound_enum(file = "sound_error")]
        SoundError,
        #[sound_enum(file = "sound_start_signup")]
        StartSignup,
        #[sound_enum(file = "sound_iris_scanning_loop_01A")]
        IrisScanningLoop01A,
        #[sound_enum(file = "sound_iris_scanning_loop_01B")]
        IrisScanningLoop01B,
        #[sound_enum(file = "sound_iris_scanning_loop_01C")]
        IrisScanningLoop01C,
        #[sound_enum(file = "sound_iris_scanning_loop_02A")]
        IrisScanningLoop02A,
        #[sound_enum(file = "sound_iris_scanning_loop_02B")]
        IrisScanningLoop02B,
        #[sound_enum(file = "sound_iris_scanning_loop_02C")]
        IrisScanningLoop02C,
        #[sound_enum(file = "sound_iris_scanning_loop_03A")]
        IrisScanningLoop03A,
        #[sound_enum(file = "sound_iris_scanning_loop_03B")]
        IrisScanningLoop03B,
        #[sound_enum(file = "sound_iris_scanning_loop_03C")]
        IrisScanningLoop03C,
        #[sound_enum(file = "sound_start_idle")]
        StartIdle,
    }
}

#[derive(Clone)]
struct SoundFile(Arc<Vec<u8>>);

impl AsRef<[u8]> for SoundFile {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Jetson {
    /// Spawns a new sound queue.
    pub async fn spawn(
        config: Arc<Mutex<Config>>,
        ignore_missing_sounds: bool,
        cpu_monitor: Box<dyn monitor::cpu::Monitor>,
    ) -> Result<Self> {
        let (mut curr_volume, language) = async {
            let Config { basic_config: BasicConfig { sound_volume, language }, .. } =
                config.lock().await.clone();
            (sound_volume, language)
        }
        .await;
        tracing::debug!("Starting with volume {}", curr_volume);
        #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
        let master_volume = move || {
            if let Some(config) = config.lock().now_or_never() {
                let new_volume = config.sound_volume();
                if curr_volume != new_volume {
                    tracing::debug!("Changing volume from {} to {}", curr_volume, new_volume);
                    curr_volume = new_volume;
                }
            }
            curr_volume as f64 / 100.0
        };
        let sound = Self {
            queue: Arc::new(Queue::spawn(SOUND_CARD_NAME, master_volume)?),
            sound_files: Arc::new(DashMap::new()),
            cpu_monitor,
        };
        sound.load_sound_files(language.as_deref(), ignore_missing_sounds).await?;
        Ok(sound)
    }
}

impl Player for Jetson {
    fn load_sound_files(
        &self,
        language: Option<&str>,
        ignore_missing_sounds: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        let sound_files = Arc::clone(&self.sound_files);
        let language = language.map(ToOwned::to_owned);
        Box::pin(async move {
            Voice::load_sound_files(&sound_files, language.as_deref(), ignore_missing_sounds)
                .await?;
            Melody::load_sound_files(&sound_files, language.as_deref(), ignore_missing_sounds)
                .await?;
            tracing::info!("Sound files for language {language:?} loaded successfully");
            Ok(())
        })
    }

    #[allow(clippy::missing_panics_doc)]
    fn build(&mut self, sound_type: Type) -> Result<SoundBuilder> {
        if let Some(monitor::cpu::Report { cpu_load, .. }) = self.cpu_monitor.last_report()? {
            if *cpu_load > CPU_LOAD_THRESHOLD {
                tracing::warn!("Skipping sound due to high CPU load ({cpu_load}): {sound_type:?}");
                return Ok(SoundBuilder::default());
            }
        }
        let sound_file = self.sound_files.get(&sound_type).unwrap();
        // It does Arc::clone under the hood, which is cheap.
        let reader = (!sound_file.as_ref().is_empty()).then(|| Cursor::new(sound_file.clone()));
        Ok(self.queue.sound(reader, format!("{sound_type:?}")))
    }

    fn clone(&self) -> Box<dyn Player> {
        Box::new(Jetson {
            queue: self.queue.clone(),
            sound_files: self.sound_files.clone(),
            cpu_monitor: self.cpu_monitor.clone(),
        })
    }
}

impl Player for Fake {
    fn load_sound_files(
        &self,
        _language: Option<&str>,
        _ignore_missing_sounds: bool,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + '_>> {
        Box::pin(async { Ok(()) })
    }

    fn build(&mut self, _sound_type: Type) -> Result<SoundBuilder> {
        Ok(SoundBuilder::default())
    }

    fn clone(&self) -> Box<dyn Player> {
        Box::new(Fake)
    }
}

/// Returns SoundFile if sound in filesystem entries.
async fn load_sound_file(
    sound: &str,
    language: Option<&str>,
    ignore_missing: bool,
) -> Result<SoundFile> {
    let sounds_dir = Path::new(SOUNDS_DIR);
    if let Some(language) = language {
        let file = sounds_dir.join(format!("{sound}__{language}.wav"));
        if file.exists() {
            let data = fs::read(&file)
                .await
                .wrap_err_with(|| format!("failed to read {}", file.display()))?;
            return Ok(SoundFile(Arc::new(data)));
        }
    }
    let file = sounds_dir.join(format!("{sound}.wav"));
    let data = match fs::read(&file)
        .await
        .wrap_err_with(|| format!("failed to read {}", file.display()))
    {
        Ok(data) => data,
        Err(err) => {
            if ignore_missing {
                tracing::error!("Ignoring missing sounds: {err}");
                Vec::new()
            } else {
                return Err(err);
            }
        }
    };
    Ok(SoundFile(Arc::new(data)))
}

impl fmt::Debug for Jetson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sound").finish()
    }
}
