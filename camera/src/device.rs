use crate::{
    close, ioctl, open,
    wait::{Waiter, Wake},
};
use libc::{c_int, c_uint, c_void, O_CLOEXEC, O_NONBLOCK, O_RDWR};
use std::{
    ffi::{CStr, CString},
    io, mem,
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr,
    task::Context,
};
use v4l2_sys::{
    v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE, v4l2_ctrl_type,
    v4l2_ctrl_type_V4L2_CTRL_TYPE_INTEGER64, v4l2_ext_control, v4l2_ext_controls, v4l2_format,
    v4l2_pix_format, v4l2_queryctrl, V4L2_CTRL_FLAG_DISABLED, V4L2_CTRL_FLAG_NEXT_CTRL,
    VIDIOC_G_EXT_CTRLS, VIDIOC_G_FMT, VIDIOC_QUERYCTRL, VIDIOC_STREAMOFF, VIDIOC_STREAMON,
    VIDIOC_S_EXT_CTRLS, VIDIOC_S_FMT,
};

/// IMX392 device interface.
#[derive(Debug)]
pub struct Device {
    pub(crate) fd: c_int,
}

/// Camera format returned by [`Device::format`] method.
#[derive(Debug)]
pub struct Format {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// The pixel format or type of compression, set by the application.
    pub pixel_format: c_uint,
    /// Distance in bytes between the leftmost pixels in two adjacent lines.
    pub bytes_per_line: c_uint,
    /// Size in bytes of the buffer to hold a complete image, set by the driver.
    pub size: c_uint,
}

impl Device {
    /// Opens the camera device.
    pub fn open<P: AsRef<Path>>(path: P) -> io::Result<Self> {
        let path = CString::new(path.as_ref().as_os_str().as_bytes())?;
        let fd = unsafe { open(path.as_ptr(), O_RDWR | O_NONBLOCK | O_CLOEXEC)? };
        Ok(Self { fd })
    }

    /// Reads the current device format.
    pub fn format(&self) -> io::Result<Format> {
        let mut v4l_fmt: v4l2_format = unsafe { mem::zeroed() };
        v4l_fmt.type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        unsafe { ioctl(self.fd, VIDIOC_G_FMT, ptr::addr_of_mut!(v4l_fmt).cast::<c_void>())? };
        Ok(Format::from(unsafe { v4l_fmt.fmt.pix }))
    }

    /// Attempts to change the device format.
    pub fn set_format(&self, format: &Format) -> io::Result<Format> {
        let mut v4l_fmt: v4l2_format = unsafe { mem::zeroed() };
        v4l_fmt.type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        unsafe { format.update_raw(&mut v4l_fmt.fmt.pix) };
        unsafe { ioctl(self.fd, VIDIOC_S_FMT, ptr::addr_of_mut!(v4l_fmt).cast::<c_void>())? };
        Ok(Format::from(unsafe { v4l_fmt.fmt.pix }))
    }

    /// Starts frames capturing.
    pub fn start(&self) -> io::Result<()> {
        let mut type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        unsafe { ioctl(self.fd, VIDIOC_STREAMON, ptr::addr_of_mut!(type_).cast::<c_void>())? };
        Ok(())
    }

    /// Stops frames capturing.
    pub fn stop(&self) -> io::Result<()> {
        let mut type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        unsafe { ioctl(self.fd, VIDIOC_STREAMOFF, ptr::addr_of_mut!(type_).cast::<c_void>())? };
        Ok(())
    }

    /// Creates an asynchronous context with the ability to wait for either an
    /// asynchronous event or a new frame data.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use futures::{channel::mpsc::channel, prelude::*};
    /// use orb_camera::Device;
    /// use std::{pin::Pin, task::Poll, thread, time::Duration};
    ///
    /// let camera = Device::open("/dev/video0").unwrap();
    /// let (tx, mut rx) = channel::<u32>(0);
    /// thread::spawn(move || {
    ///     camera.with_waiter_context(|waiter, cx| {
    ///         loop {
    ///             if let Poll::Ready(x) = Pin::new(&mut rx).poll_next(cx) {
    ///                 dbg!(x);
    ///             }
    ///             waiter.wait(Duration::from_millis(100));
    ///             // Here we have 3 possible states:
    ///             // a) `rx` stream has new data
    ///             // b) a new frame is ready
    ///             // c) timeout has passed
    ///         }
    ///     })
    /// });
    /// ```
    pub fn with_waiter_context<R>(
        &self,
        f: impl FnOnce(Waiter, &mut Context<'_>) -> R,
    ) -> io::Result<R> {
        let wake = Wake::new()?;
        let waiter = Waiter::new(&wake, self);
        let waker = wake.into_waker();
        let mut cx = Context::from_waker(&waker);
        Ok(f(waiter, &mut cx))
    }

    /// Sets camera control value by name.
    pub fn set_control(&self, name: &str, value: i64) -> io::Result<()> {
        let (id, type_) = self.find_control(name)?;
        let mut ext_ctls: v4l2_ext_controls = unsafe { mem::zeroed() };
        let mut ext_ctl: v4l2_ext_control = unsafe { mem::zeroed() };
        ext_ctl.id = id;
        #[allow(clippy::cast_possible_truncation)]
        if type_ == v4l2_ctrl_type_V4L2_CTRL_TYPE_INTEGER64 {
            ext_ctl.__bindgen_anon_1.value64 = value;
        } else {
            ext_ctl.__bindgen_anon_1.value = value as i32;
        }
        ext_ctls.__bindgen_anon_1.ctrl_class = type_;
        ext_ctls.count = 1;
        ext_ctls.controls = &mut ext_ctl;
        unsafe {
            ioctl(self.fd, VIDIOC_S_EXT_CTRLS, ptr::addr_of_mut!(ext_ctls).cast::<c_void>())?
        };
        Ok(())
    }

    /// Gets camera control value by name.
    pub fn get_control(&self, name: &str) -> io::Result<i64> {
        let (id, type_) = self.find_control(name)?;
        let mut ext_ctls: v4l2_ext_controls = unsafe { mem::zeroed() };
        let mut ext_ctl: v4l2_ext_control = unsafe { mem::zeroed() };
        ext_ctl.id = id;
        ext_ctls.__bindgen_anon_1.ctrl_class = type_;
        ext_ctls.count = 1;
        ext_ctls.controls = &mut ext_ctl;
        unsafe {
            ioctl(self.fd, VIDIOC_G_EXT_CTRLS, ptr::addr_of_mut!(ext_ctls).cast::<c_void>())?
        };
        #[allow(clippy::cast_possible_truncation)]
        unsafe {
            Ok(if type_ == v4l2_ctrl_type_V4L2_CTRL_TYPE_INTEGER64 {
                ext_ctl.__bindgen_anon_1.value64
            } else {
                ext_ctl.__bindgen_anon_1.value.into()
            })
        }
    }

    fn find_control(&self, name: &str) -> io::Result<(u32, v4l2_ctrl_type)> {
        let c_name = CString::new(name).unwrap();
        let mut queryctl: v4l2_queryctrl = unsafe { mem::zeroed() };
        // Due to proprietary NVidia IDs we have to review the ID by its name.
        loop {
            queryctl.id |= V4L2_CTRL_FLAG_NEXT_CTRL;
            let result = unsafe {
                ioctl(self.fd, VIDIOC_QUERYCTRL, ptr::addr_of_mut!(queryctl).cast::<c_void>())
            };
            if let Err(err) = &result {
                if matches!(err.kind(), io::ErrorKind::InvalidInput) {
                    panic!("Camera control with name `{name}` not found");
                }
            }
            result?;
            let end = queryctl.name.iter().position(|&x| x == 0).unwrap();
            let query_name = CStr::from_bytes_with_nul(&queryctl.name[..=end]).unwrap();
            if queryctl.flags & V4L2_CTRL_FLAG_DISABLED == 0 && query_name == c_name.as_c_str() {
                break;
            }
        }
        Ok((queryctl.id, queryctl.type_))
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            if let Err(err) = close(self.fd) {
                log::error!("Couldn't close video4linux device descriptor: {err}");
            }
        }
    }
}

impl From<v4l2_pix_format> for Format {
    fn from(fmt: v4l2_pix_format) -> Self {
        Self {
            width: fmt.width,
            height: fmt.height,
            pixel_format: fmt.pixelformat,
            bytes_per_line: fmt.bytesperline,
            size: fmt.sizeimage,
        }
    }
}

impl Format {
    fn update_raw(&self, fmt: &mut v4l2_pix_format) {
        fmt.width = self.width;
        fmt.height = self.height;
        fmt.pixelformat = self.pixel_format;
    }
}
