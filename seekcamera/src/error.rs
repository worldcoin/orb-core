use seekcamera_sys::seekcamera_error_t;

/// Enumerated type representing types of events used by the camera manager.
#[derive(Clone, Copy, Debug, thiserror::Error)]
pub enum Error {
    /// Device communication failure.
    #[error("device communication failure")]
    DeviceCommunication,
    /// Invalid parameter is received.
    #[error("invalid parameter is received")]
    InvalidParameter,
    /// Insufficient permissions to access a resource.
    #[error("insufficient permissions to access a resource")]
    Permissions,
    /// There is no device.
    #[error("there is no device")]
    NoDevice,
    /// No device is found.
    #[error("no device is found")]
    DeviceNotFound,
    /// Device is busy.
    #[error("device is busy")]
    DeviceBusy,
    /// Device timeout.
    #[error("device timeout")]
    Timeout,
    /// Overflow is detected.
    #[error("overflow is detected")]
    Overflow,
    /// Unknown request is made.
    #[error("unknown request is made")]
    UnknownRequest,
    /// Operation is interrupted.
    #[error("operation is interrupted")]
    Interrupted,
    /// The system is out of memory.
    #[error("the system is out of memory")]
    OutOfMemory,
    /// Operation is not supported.
    #[error("operation is not supported")]
    NotSupported,
    /// Source of the error is unknown.
    #[error("source of the error is unknown")]
    Other,
    /// Request cannot be performed.
    #[error("request cannot be performed")]
    CannotPerformRequest,
    /// Flash access failure.
    #[error("flash access failure")]
    FlashAccessFailure,
    /// Implementation error.
    #[error("implementation error")]
    ImplementationError,
    /// Request is already pending.
    #[error("request is already pending")]
    RequestPending,
    /// Firmware image is invalid.
    #[error("firmware image is invalid")]
    InvalidFirmwareImage,
    /// Invalid key is encountered.
    #[error("invalid key is encountered")]
    InvalidKey,
    /// Sensor communication failure.
    #[error("sensor communication failure")]
    SensorCommunication,
    /// Value is out of range.
    #[error("value is out of range")]
    OutOfRange,
    /// Verification failure.
    #[error("verification failure")]
    VerifyFailed,
    /// System call failure.
    #[error("system call failure")]
    SyscallFailed,
    /// File does not exist.
    #[error("file does not exist")]
    FileDoesNotExist,
    /// Directory does not exist.
    #[error("directory does not exist")]
    DirectoryDoesNotExist,
    /// File read failure.
    #[error("file read failure")]
    FileReadFailed,
    /// File write failure.
    #[error("file write failure")]
    FileWriteFailed,
    /// Method is not implemented.
    #[error("method is not implemented")]
    NotImplemented,
    /// Camera connects in an unpaired state.
    #[error("camera connects in an unpaired state")]
    NotPaired,
    /// Generic error code.
    #[error("generic error code: {}", 0)]
    Generic(seekcamera_error_t),
}

pub(crate) fn result_from(code: seekcamera_error_t) -> Result<(), Error> {
    match code {
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_SUCCESS => Ok(()),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_DEVICE_COMMUNICATION => {
            Err(Error::DeviceCommunication)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_INVALID_PARAMETER => {
            Err(Error::InvalidParameter)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_PERMISSIONS => Err(Error::Permissions),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_NO_DEVICE => Err(Error::NoDevice),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_DEVICE_NOT_FOUND => {
            Err(Error::DeviceNotFound)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_DEVICE_BUSY => Err(Error::DeviceBusy),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_TIMEOUT => Err(Error::Timeout),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_OVERFLOW => Err(Error::Overflow),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_UNKNOWN_REQUEST => {
            Err(Error::UnknownRequest)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_INTERRUPTED => Err(Error::Interrupted),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_OUT_OF_MEMORY => {
            Err(Error::OutOfMemory)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_NOT_SUPPORTED => {
            Err(Error::NotSupported)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_OTHER => Err(Error::Other),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_CANNOT_PERFORM_REQUEST => {
            Err(Error::CannotPerformRequest)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_FLASH_ACCESS_FAILURE => {
            Err(Error::FlashAccessFailure)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_IMPLEMENTATION_ERROR => {
            Err(Error::ImplementationError)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_REQUEST_PENDING => {
            Err(Error::RequestPending)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_INVALID_FIRMWARE_IMAGE => {
            Err(Error::InvalidFirmwareImage)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_INVALID_KEY => Err(Error::InvalidKey),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_SENSOR_COMMUNICATION => {
            Err(Error::SensorCommunication)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_OUT_OF_RANGE => Err(Error::OutOfRange),
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_VERIFY_FAILED => {
            Err(Error::VerifyFailed)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_SYSCALL_FAILED => {
            Err(Error::SyscallFailed)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_FILE_DOES_NOT_EXIST => {
            Err(Error::FileDoesNotExist)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_DIRECTORY_DOES_NOT_EXIST => {
            Err(Error::DirectoryDoesNotExist)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_FILE_READ_FAILED => {
            Err(Error::FileReadFailed)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_FILE_WRITE_FAILED => {
            Err(Error::FileWriteFailed)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_NOT_IMPLEMENTED => {
            Err(Error::NotImplemented)
        }
        seekcamera_sys::seekcamera_error_t_SEEKCAMERA_ERROR_NOT_PAIRED => Err(Error::NotPaired),
        other => Err(Error::Generic(other)),
    }
}
