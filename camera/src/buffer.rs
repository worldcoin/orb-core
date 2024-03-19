use crate::{ioctl, mmap, munmap, Device};
use libc::{MAP_SHARED, PROT_READ, PROT_WRITE};
use std::{io, mem, ptr, slice, time::Duration};
use v4l2_sys::{
    v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE, v4l2_buffer, v4l2_memory_V4L2_MEMORY_MMAP,
    v4l2_requestbuffers, V4L2_BUF_FLAG_QUEUED, VIDIOC_DQBUF, VIDIOC_QBUF, VIDIOC_QUERYBUF,
    VIDIOC_REQBUFS,
};

/// Set of video4linux buffers.
#[derive(Debug)]
pub struct Buffer<'a> {
    device: &'a Device,
    count: u32,
    buffers: Vec<&'a mut [u8]>,
}

/// Dequeued buffer description returned from [`Buffer::dequeue`].
#[derive(Debug)]
pub struct Dequeued {
    /// Index of the dequeued buffer.
    pub index: u32,
    /// Time when the first data byte was captured.
    pub timestamp: Duration,
}

impl<'a> Buffer<'a> {
    /// Request specified number of buffers for the device.
    ///
    /// # Panics
    ///
    /// If the driver can't allocate required number of buffers.
    pub fn new(device: &'a Device, count: u32) -> io::Result<Self> {
        let mut buffers = Vec::with_capacity(count as usize);

        let mut req: v4l2_requestbuffers = unsafe { mem::zeroed() };
        req.memory = v4l2_memory_V4L2_MEMORY_MMAP;
        req.count = count;
        req.type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        unsafe { ioctl(device.fd, VIDIOC_REQBUFS, ptr::addr_of_mut!(req).cast())? };
        assert_eq!(req.count, count);

        for i in 0..req.count {
            let mut buffer: v4l2_buffer = unsafe { mem::zeroed() };
            buffer.memory = v4l2_memory_V4L2_MEMORY_MMAP;
            buffer.index = i;
            buffer.type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
            unsafe { ioctl(device.fd, VIDIOC_QUERYBUF, ptr::addr_of_mut!(buffer).cast())? };

            let ptr = unsafe {
                mmap(
                    ptr::null_mut(),
                    buffer.length as usize,
                    PROT_READ | PROT_WRITE,
                    MAP_SHARED,
                    device.fd,
                    buffer.m.offset.into(),
                )?
            };
            let slice = unsafe { slice::from_raw_parts_mut(ptr.cast(), buffer.length as usize) };
            buffers.push(slice);
        }

        Ok(Self { device, count, buffers })
    }

    /// Sends the buffer to the queue for filling with new frames.
    pub fn enqueue(&self, index: u32) -> io::Result<()> {
        let mut buffer: v4l2_buffer = unsafe { mem::zeroed() };
        buffer.memory = v4l2_memory_V4L2_MEMORY_MMAP;
        buffer.type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buffer.index = index;
        unsafe { ioctl(self.device.fd, VIDIOC_QBUF, ptr::addr_of_mut!(buffer).cast())? };
        Ok(())
    }

    /// Tries to get a buffer filled with a new frame. Returns `None` if there
    /// are no new frames. Otherwise returns `Some(index)` with the buffer
    /// index.
    pub fn dequeue(&self) -> io::Result<Option<Dequeued>> {
        let mut buffer: v4l2_buffer = unsafe { mem::zeroed() };
        buffer.memory = v4l2_memory_V4L2_MEMORY_MMAP;
        buffer.type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        let ret = unsafe { ioctl(self.device.fd, VIDIOC_DQBUF, ptr::addr_of_mut!(buffer).cast())? };
        if ret.is_some() && buffer.flags & V4L2_BUF_FLAG_QUEUED == 0 {
            // FIXME current vcmipi driver doesn't return timestamps
            // let timestamp = {
            //     #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            //     Duration::new(
            //         buffer.timestamp.tv_sec as u64,
            //         buffer.timestamp.tv_usec as u32 * 1000,
            //     )
            // };
            let timestamp = crate::now()?;
            Ok(Some(Dequeued { index: buffer.index, timestamp }))
        } else {
            Ok(None)
        }
    }

    /// Returns the buffer data with the specified index.
    #[must_use]
    pub fn get(&self, index: u32) -> &[u8] {
        self.buffers[index as usize]
    }

    /// Returns the number of buffers in the buffer set.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.count
    }

    fn free(&mut self) -> io::Result<()> {
        while let Some(buffer) = self.buffers.pop() {
            unsafe { munmap(buffer.as_mut_ptr().cast(), buffer.len())? };
        }

        let mut req: v4l2_requestbuffers = unsafe { mem::zeroed() };
        req.memory = v4l2_memory_V4L2_MEMORY_MMAP;
        req.count = 0;
        req.type_ = v4l2_buf_type_V4L2_BUF_TYPE_VIDEO_CAPTURE;
        unsafe { ioctl(self.device.fd, VIDIOC_REQBUFS, ptr::addr_of_mut!(req).cast())? };

        Ok(())
    }
}

impl Drop for Buffer<'_> {
    fn drop(&mut self) {
        if self.free().is_err() {
            log::error!("Couldn't deinitialize video4linux buffers");
        }
    }
}
