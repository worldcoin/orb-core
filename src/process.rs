//! A set of helpers for spawning sub-processes in a sandboxes.
/// # Examples
///
/// Simply spawn a command, same way as with `std::process::Command` except that the executable should be an absolute path.
/// ```
/// use orb::process::Command;
/// let command = Command::new("ls").spawn().unwrap();
/// ```
///
/// For a demonstration, see how file descriptors are closed. This example opens
/// "/dev/zero" in the parent process and spawns a child process, which lists
/// all open file descriptors. When spawend with `std::process::Command`, the
/// child has access to the parent's "/dev/zero" file descriptor. When spawned
/// with `process::Command`, the child does not inherit the file
/// descriptor.
///
/// ```
/// use orb::process::Command;
///
/// let extra_fd =
///     unsafe { libc::open("/dev/zero\0".as_ptr() as *const libc::c_char, libc::O_RDWR) };
/// assert!(extra_fd >= 0);
///
/// // Use std::process::Command() to demonstrate file descriptor inheritance.
/// let std_output = String::from_utf8(
///     std::process::Command::new("ls").arg("-l").arg("/proc/self/fd").output().unwrap().stdout,
/// )
/// .unwrap();
/// assert!(std_output.find("/dev/zero").is_some());
///
/// // No file descriptors inherited when using hardened vertion of Command
/// let hardened_output = String::from_utf8(
///     Command::new("ls").arg("-l").arg("/proc/self/fd").output().unwrap().stdout,
/// )
/// .unwrap();
/// assert_eq!(hardened_output.find("/dev/zero"), None);
/// ```
use close_fds::close_open_fds;
use libc::c_int;
use std::{convert::AsRef, os::unix::process::CommandExt, path::Path};

/// A wrapper for std::process::Command, which closes all fds except stdin, stdout, stderr and `keep_fds`.
pub struct Command {}

impl Command {
    /// Return a `std::process::Command` which closes all fds except stdin, stdout, stderr.
    #[allow(clippy::new_ret_no_self)]
    pub fn new<P: AsRef<Path>>(program: P) -> std::process::Command {
        Command::new_with_keep_fds(program, &[])
    }

    /// Return a `std::process::Command`, which closes all fds except stdin, stdout, stderr and `keep_fds`.
    fn new_with_keep_fds<P: AsRef<Path>>(program: P, keep_fds: &[c_int]) -> std::process::Command {
        if !program.as_ref().is_absolute() {
            tracing::warn!("{} is not an absolute path", program.as_ref().display());
        }
        let mut command = std::process::Command::new(program.as_ref().as_os_str());
        let keep_fds = keep_fds.to_vec();
        unsafe {
            command.pre_exec(move || {
                close_open_fds(libc::STDERR_FILENO + 1, &keep_fds);
                Ok(())
            });
        }
        command
    }
}
