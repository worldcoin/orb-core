use crate::{close, eventfd, read, select, write, Device};
use libc::{
    c_int, c_void, fd_set, suseconds_t, time_t, timeval, EFD_CLOEXEC, FD_ISSET, FD_SET, FD_ZERO,
};
use std::{
    cmp, io,
    mem::{self, forget, ManuallyDrop, MaybeUninit},
    ptr,
    rc::Rc,
    task::{RawWaker, RawWakerVTable, Waker},
    time::Duration,
};

static VTABLE: RawWakerVTable = RawWakerVTable::new(wake_clone, wake_wake, wake_wake, wake_drop);

/// A handle for putting the current thread to sleep via its
/// [`wait`](Waiter::wait) method.
pub struct Waiter {
    waker: Wake,
    device: c_int,
}

#[repr(transparent)]
#[derive(Clone)]
pub(crate) struct Wake {
    eventfd: Rc<c_int>,
}

impl Waiter {
    pub(crate) fn new(waker: &Wake, device: &Device) -> Self {
        Self { waker: waker.clone(), device: device.fd }
    }

    /// Puts the current thread to sleep until either a new frame data becomes
    /// ready or woken by an asynchronous event.
    pub fn wait(&self, timeout: Duration) -> io::Result<()> {
        let mut tv = timeval {
            tv_sec: timeout.as_secs() as time_t,
            tv_usec: suseconds_t::from(timeout.subsec_micros()),
        };
        unsafe {
            #[allow(invalid_value, clippy::uninit_assumed_init)]
            let mut fd_set: fd_set = MaybeUninit::uninit().assume_init();
            FD_ZERO(&mut fd_set);
            FD_SET(self.device, &mut fd_set);
            FD_SET(*self.waker.eventfd, &mut fd_set);
            let n = select(
                cmp::max(self.device, *self.waker.eventfd) + 1,
                &mut fd_set,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut tv,
            )?;
            if n > 0 && FD_ISSET(*self.waker.eventfd, &fd_set) {
                let mut arg: u64 = 0;
                read(
                    *self.waker.eventfd,
                    ptr::addr_of_mut!(arg).cast::<c_void>(),
                    mem::size_of_val(&arg),
                )?;
            }
        }
        Ok(())
    }

    /// Puts the current thread to sleep until woken by an asynchronous event.
    pub fn wait_event(&self, timeout: Duration) -> io::Result<()> {
        let mut tv = timeval {
            tv_sec: timeout.as_secs() as time_t,
            tv_usec: suseconds_t::from(timeout.subsec_micros()),
        };
        unsafe {
            #[allow(invalid_value, clippy::uninit_assumed_init)]
            let mut fd_set: fd_set = MaybeUninit::uninit().assume_init();
            FD_ZERO(&mut fd_set);
            FD_SET(*self.waker.eventfd, &mut fd_set);
            let n = select(
                *self.waker.eventfd + 1,
                &mut fd_set,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut tv,
            )?;
            if n > 0 && FD_ISSET(*self.waker.eventfd, &fd_set) {
                let mut arg: u64 = 0;
                read(
                    *self.waker.eventfd,
                    ptr::addr_of_mut!(arg).cast::<c_void>(),
                    mem::size_of_val(&arg),
                )?;
            }
        }
        Ok(())
    }
}

impl Wake {
    pub(crate) fn new() -> io::Result<Self> {
        let eventfd = unsafe { eventfd(1, EFD_CLOEXEC)? };
        Ok(Self { eventfd: Rc::new(eventfd) })
    }

    pub(crate) fn into_waker(self) -> Waker {
        unsafe { Waker::from_raw(self.into_raw_waker()) }
    }

    fn into_raw_waker(self) -> RawWaker {
        let wake = ManuallyDrop::new(self);
        let data = unsafe { Rc::into_raw(ptr::read(&wake.eventfd)) };
        RawWaker::new(data.cast(), &VTABLE)
    }

    unsafe fn from_raw_waker(data: *const ()) -> Self {
        Self { eventfd: unsafe { Rc::from_raw(data.cast()) } }
    }

    fn wakeup(&self) {
        unsafe {
            let arg: u64 = 1;
            write(*self.eventfd, ptr::addr_of!(arg).cast::<c_void>(), mem::size_of_val(&arg))
                .expect("write to eventfd failed");
        }
    }
}

impl Drop for Wake {
    fn drop(&mut self) {
        unsafe {
            if let Some(eventfd) = Rc::get_mut(&mut self.eventfd) {
                if let Err(err) = close(*eventfd) {
                    log::error!("Couldn't close eventfd: {err}");
                }
            }
        }
    }
}

unsafe fn wake_clone(data: *const ()) -> RawWaker {
    let waker = unsafe { Wake::from_raw_waker(data) };
    let cloned_waker = waker.clone();
    forget(waker);
    cloned_waker.into_raw_waker()
}

unsafe fn wake_wake(data: *const ()) {
    unsafe { Wake::from_raw_waker(data).wakeup() };
}

unsafe fn wake_drop(data: *const ()) {
    unsafe { drop(Wake::from_raw_waker(data)) };
}
