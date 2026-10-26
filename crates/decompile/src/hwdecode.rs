//! HEVC decoding on the Apple media engine (VideoToolbox).
//!
//! Measured on the Apple M3 with FFmpeg 8.1 (status.md): the 87 s iPhone HEVC
//! Main 10 recording took 8.9 s to decode in software and 4.0 s in hardware,
//! with 62 CPU-seconds against 5.7. The same recording as 8-bit H.264 took 3.0 s
//! in software and 11.2 s in hardware, so only HEVC comes here (`decode.rs`).
//!
//! `ffmpeg-next` has no wrapper for hardware devices, so this module calls
//! libavcodec through `ffmpeg_next::ffi`. It holds all of the `unsafe` code in
//! the workspace. Each block says what makes it sound.
//!
//! A frame from the hardware decoder lives on the GPU side. [`download`] copies
//! it to a software frame (`nv12` or `p010`), and the sampling code reads that
//! like any other frame. Any failure is reported as text, and `decode.rs`
//! decodes the file again in software and says why in `decode_note`.

#[cfg(target_os = "macos")]
mod videotoolbox {
    use std::ptr;

    use ffmpeg_next::ffi;
    use ffmpeg_next::frame::Video as VideoFrame;
    use ffmpeg_next::util::format::Pixel;

    /// A VideoToolbox device context, shared by the decoders that use it.
    pub(crate) struct HardwareDevice {
        buffer: *mut ffi::AVBufferRef,
    }

    impl HardwareDevice {
        pub(crate) fn create() -> Result<Self, String> {
            let mut buffer: *mut ffi::AVBufferRef = ptr::null_mut();
            // SAFETY: `buffer` is a valid out-pointer for the new device
            // reference; the device name and options may be null for
            // VideoToolbox.
            let code = unsafe {
                ffi::av_hwdevice_ctx_create(
                    &mut buffer,
                    ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
                    ptr::null(),
                    ptr::null_mut(),
                    0,
                )
            };
            if code < 0 || buffer.is_null() {
                return Err(format!(
                    "no VideoToolbox device ({})",
                    ffmpeg_next::Error::from(code)
                ));
            }
            Ok(Self { buffer })
        }
    }

    impl Drop for HardwareDevice {
        fn drop(&mut self) {
            // SAFETY: `buffer` came from `av_hwdevice_ctx_create`, is unreferenced
            // only here, and decoders hold their own references (`attach`).
            unsafe { ffi::av_buffer_unref(&mut self.buffer) };
        }
    }

    /// Point a not-yet-opened decoder at `device` and allow only VideoToolbox
    /// frames.
    pub(crate) fn attach(
        context: &mut ffmpeg_next::codec::Context,
        device: &HardwareDevice,
    ) -> Result<(), String> {
        // SAFETY: `context` is a valid, unopened codec context. The reference
        // taken here is owned by the context, and libavcodec releases it when
        // the context is freed. `device.buffer` is alive for the call.
        unsafe {
            let raw = context.as_mut_ptr();
            let reference = ffi::av_buffer_ref(device.buffer);
            if reference.is_null() {
                return Err("could not reference the VideoToolbox device".to_string());
            }
            (*raw).hw_device_ctx = reference;
            (*raw).get_format = Some(videotoolbox_only);
        }
        Ok(())
    }

    /// Accept VideoToolbox frames and nothing else. If libavcodec cannot start
    /// hardware decoding it calls this again without the hardware format, and
    /// answering `NONE` makes decoding fail instead of quietly running in
    /// software under a hardware label.
    ///
    /// Does nothing that can panic: unwinding out of an `extern "C"` function
    /// aborts the process.
    unsafe extern "C" fn videotoolbox_only(
        _context: *mut ffi::AVCodecContext,
        formats: *const ffi::AVPixelFormat,
    ) -> ffi::AVPixelFormat {
        let mut format = formats;
        // SAFETY: libavcodec passes a list ended by AV_PIX_FMT_NONE.
        unsafe {
            while *format != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
                if *format == ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX {
                    return *format;
                }
                format = format.add(1);
            }
        }
        ffi::AVPixelFormat::AV_PIX_FMT_NONE
    }

    /// Whether `software` already holds a frame in a pixel format other than the
    /// one `hardware` copies to (`nv12` or `p010`).
    fn software_format_changed(hardware: &VideoFrame, software: &VideoFrame) -> bool {
        // SAFETY: `hardware` is a valid frame. A VideoToolbox frame carries a
        // frames context, whose data is an `AVHWFramesContext`; both pointers
        // are checked for null before they are read.
        let target = unsafe {
            let frames = (*hardware.as_ptr()).hw_frames_ctx;
            if frames.is_null() || (*frames).data.is_null() {
                return false;
            }
            (*((*frames).data as *const ffi::AVHWFramesContext)).sw_format as i32
        };
        // SAFETY: `software` is a valid frame and `format` is a plain field.
        let held = unsafe { (*software.as_ptr()).format };
        held != ffi::AVPixelFormat::AV_PIX_FMT_NONE as i32 && held != target
    }

    /// Copy a decoded hardware frame to `software` (`nv12` or `p010`), with its
    /// timestamps and color metadata.
    pub(crate) fn download(hardware: &VideoFrame, software: &mut VideoFrame) -> Result<(), String> {
        if hardware.format() != Pixel::VIDEOTOOLBOX {
            return Err(format!(
                "the decoder returned a {:?} frame instead of a VideoToolbox frame",
                hardware.format()
            ));
        }
        // The transfer fills the buffers `software` already has, in the size and
        // pixel format they were made for, and libavutil requires the new frame
        // to match. A stream that changes size or bit depth would be copied
        // into the old buffers and cropped, or refused, so start from an empty
        // frame instead.
        if software.width() != hardware.width()
            || software.height() != hardware.height()
            || software_format_changed(hardware, software)
        {
            *software = VideoFrame::empty();
        }
        // SAFETY: both frames are valid for the calls, and the side data array is
        // the one `software` owns. `software` is empty or has the size and
        // pixel format that `hardware` copies to, which libavutil requires, so
        // the transfer allocates its buffers or fills ones that fit.
        unsafe {
            let code = ffi::av_hwframe_transfer_data(software.as_mut_ptr(), hardware.as_ptr(), 0);
            if code < 0 {
                return Err(format!(
                    "copying a frame from the media engine failed ({})",
                    ffmpeg_next::Error::from(code)
                ));
            }
            // The transfer copies properties only into a frame it allocates,
            // and pts, range, and transfer characteristic are read later.
            // Copying adds the side data of `hardware` to what `software`
            // already holds from the last frame, which grows without end on
            // HDR video, so release the old entries first.
            let raw = software.as_mut_ptr();
            ffi::av_frame_side_data_free(&mut (*raw).side_data, &mut (*raw).nb_side_data);
            let code = ffi::av_frame_copy_props(software.as_mut_ptr(), hardware.as_ptr());
            if code < 0 {
                return Err(format!(
                    "copying frame properties failed ({})",
                    ffmpeg_next::Error::from(code)
                ));
            }
        }
        if software.width() != hardware.width() || software.height() != hardware.height() {
            return Err(format!(
                "the copied frame is {}x{}, not the decoded {}x{}",
                software.width(),
                software.height(),
                hardware.width(),
                hardware.height()
            ));
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub(crate) use videotoolbox::{attach, download, HardwareDevice};

/// How many side data entries (HDR metadata, SEI messages) a frame holds.
#[cfg(all(test, target_os = "macos"))]
pub(crate) fn side_data_count(frame: &ffmpeg_next::frame::Video) -> usize {
    // SAFETY: `frame` is a valid frame, and `nb_side_data` is a plain field.
    let count = unsafe { (*frame.as_ptr()).nb_side_data };
    count.max(0) as usize
}

/// Other platforms have no hardware path: `choose_decoder` never asks for it,
/// and these exist only so the decode code compiles.
#[cfg(not(target_os = "macos"))]
mod unavailable {
    use ffmpeg_next::frame::Video as VideoFrame;

    pub(crate) struct HardwareDevice;

    impl HardwareDevice {
        pub(crate) fn create() -> Result<Self, String> {
            Err("VideoToolbox is only available on macOS".to_string())
        }
    }

    pub(crate) fn attach(
        _context: &mut ffmpeg_next::codec::Context,
        _device: &HardwareDevice,
    ) -> Result<(), String> {
        Err("VideoToolbox is only available on macOS".to_string())
    }

    pub(crate) fn download(_: &VideoFrame, _: &mut VideoFrame) -> Result<(), String> {
        Err("VideoToolbox is only available on macOS".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) use unavailable::{attach, download, HardwareDevice};
