//! MIT-SHM segment and patch upload for the spotlight software path.

use crate::overlay::{OverlayError, SpotlightFrame, runtime_error};

use std::os::fd::IntoRawFd;
use std::ptr;

use x11rb::connection::Connection;
use x11rb::protocol::shm::{self, ConnectionExt as ShmConnectionExt};
use x11rb::rust_connection::RustConnection;

/// Copy a rendered patch into `dst` at `origin`.
///
/// `dst` holds a `dst_width × N` surface with four bytes per pixel. Each patch row is
/// copied into the matching surface row, so the patch does not need to cover the full width.
pub(crate) fn blit_patch(
    dst: &mut [u8],
    dst_width: u16,
    patch: &SpotlightFrame,
    origin: (u16, u16),
) -> Result<(), OverlayError> {
    let dst_stride = usize::from(dst_width) * 4;
    let src_stride = usize::from(patch.width) * 4;
    let expected_len = src_stride * usize::from(patch.height);
    if patch.pixels.len() != expected_len {
        return Err(runtime_error(format!(
            "spotlight patch has {} bytes, expected {expected_len}",
            patch.pixels.len()
        )));
    }
    if usize::from(origin.0) + usize::from(patch.width) > usize::from(dst_width) {
        return Err(runtime_error(format!(
            "spotlight patch width {} at x={} exceeds surface width {dst_width}",
            patch.width, origin.0
        )));
    }

    for row in 0..usize::from(patch.height) {
        let src_start = row * src_stride;
        let dst_start = (usize::from(origin.1) + row) * dst_stride + usize::from(origin.0) * 4;
        let dst_end = dst_start + src_stride;
        let Some(dst_row) = dst.get_mut(dst_start..dst_end) else {
            return Err(runtime_error(format!(
                "spotlight patch row {row} at y={} exceeds the surface buffer",
                origin.1
            )));
        };
        dst_row.copy_from_slice(&patch.pixels[src_start..src_start + src_stride]);
    }

    Ok(())
}

/// A mapped MIT-SHM segment sized to hold a full spotlight surface.
pub(crate) struct ShmSegment {
    seg: shm::Seg,
    addr: *mut u8,
    len: usize,
    width: u16,
    height: u16,
}

impl ShmSegment {
    /// Create and map a segment that can hold `width × height` BGRA pixels.
    pub(crate) fn new(
        connection: &RustConnection,
        width: u16,
        height: u16,
    ) -> Result<Self, OverlayError> {
        let len = width as usize * height as usize * 4;
        let size =
            u32::try_from(len).map_err(|_| runtime_error("spotlight SHM segment exceeds 4 GiB"))?;
        let seg = connection.generate_id().map_err(runtime_error)?;

        let reply = connection
            .shm_create_segment(seg, size, false)
            .map_err(runtime_error)?
            .reply()
            .map_err(runtime_error)?;
        let fd = reply.shm_fd.into_raw_fd();

        // SAFETY: `fd` is a fresh shared-memory descriptor for a segment of `size`
        // bytes, and `len` equals `size`. MAP_SHARED makes the mapping visible to
        // the X server.
        let addr = unsafe {
            libc::mmap(
                ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        // The mapping keeps its own reference to the shared-memory object.
        unsafe { libc::close(fd) };

        if addr == libc::MAP_FAILED {
            let _ = connection.shm_detach(seg);
            return Err(runtime_error(format!(
                "could not map spotlight SHM segment: {}",
                std::io::Error::last_os_error()
            )));
        }

        // Initialize the mapping before any pixel slice reads its contents.
        // SAFETY: `addr` maps `len` writable bytes.
        unsafe { ptr::write_bytes(addr.cast::<u8>(), 0, len) };

        Ok(Self {
            seg,
            addr: addr.cast(),
            len,
            width,
            height,
        })
    }

    pub(crate) fn seg(&self) -> shm::Seg {
        self.seg
    }

    /// Grow the segment when the surface no longer fits. Returns true when the
    /// segment was replaced, because the old pixel contents are gone.
    pub(crate) fn ensure_capacity(
        &mut self,
        connection: &RustConnection,
        width: u16,
        height: u16,
    ) -> Result<bool, OverlayError> {
        if width <= self.width && height <= self.height {
            return Ok(false);
        }
        let next = Self::new(connection, self.width.max(width), self.height.max(height))?;
        self.destroy(connection);
        *self = next;
        Ok(true)
    }

    /// Detach the segment and release the client mapping.
    pub(crate) fn destroy(&mut self, connection: &RustConnection) {
        if self.addr.is_null() {
            return;
        }
        let _ = connection.shm_detach(self.seg);
        // SAFETY: `addr` and `len` describe the mapping created in `new`.
        unsafe { libc::munmap(self.addr.cast(), self.len) };
        self.addr = ptr::null_mut();
        self.len = 0;
    }

    /// Mutable access to the mapped pixels.
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        debug_assert!(!self.addr.is_null(), "SHM segment was destroyed");
        // SAFETY: `addr` points to `len` writable bytes until `destroy` runs.
        unsafe { std::slice::from_raw_parts_mut(self.addr, self.len) }
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        if !self.addr.is_null() {
            // SAFETY: `addr` and `len` describe the mapping created in `new`.
            unsafe { libc::munmap(self.addr.cast(), self.len) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blit_copies_each_patch_row_at_the_origin() {
        let mut dst = vec![0_u8; 4 * 4 * 4];
        let patch = SpotlightFrame {
            width: 2,
            height: 2,
            pixels: (1_u8..=16).collect(),
        };

        blit_patch(&mut dst, 4, &patch, (1, 2)).unwrap();

        assert_eq!(&dst[36..44], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&dst[52..60], &[9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(&dst[44..48], &[0, 0, 0, 0]);
    }

    #[test]
    fn blit_rejects_a_patch_wider_than_the_surface() {
        let mut dst = vec![0_u8; 4 * 4 * 4];
        let patch = SpotlightFrame {
            width: 3,
            height: 1,
            pixels: vec![0; 12],
        };

        assert!(blit_patch(&mut dst, 4, &patch, (2, 0)).is_err());
    }

    #[test]
    fn blit_rejects_a_patch_taller_than_the_surface() {
        let mut dst = vec![0_u8; 4 * 4 * 4];
        let patch = SpotlightFrame {
            width: 1,
            height: 2,
            pixels: vec![0; 8],
        };

        assert!(blit_patch(&mut dst, 4, &patch, (0, 3)).is_err());
    }
}
