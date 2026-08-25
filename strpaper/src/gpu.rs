//! D3D11 video pipeline: hardware decode hand-off and GPU colour conversion.
//!
//! `Gpu` owns a hardware D3D11 device. The device is shared with Media
//! Foundation through `IMFDXGIDeviceManager` (so the H.264 decoder runs on the
//! GPU's video engine instead of the CPU), and its fixed-function video
//! processor converts decoded NV12 surfaces to BGRA on the GPU. The UI thread
//! then performs one small copy per frame instead of software-decoding and
//! converting millions of pixels.

use std::cell::RefCell;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_UNKNOWN,
};
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext,
    ID3D11VideoContext1, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_CPU_ACCESS_READ, D3D11_BIND_RENDER_TARGET,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ, D3D11_SDK_VERSION,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, D3D11CreateDevice,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFDXGIBuffer, IMFDXGIDeviceManager, IMFSample, MF_SOURCE_READER_D3D_MANAGER,
};

/// Hardware device + Media Foundation interop for one decode session.
pub struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    video_context1: Option<ID3D11VideoContext1>,
    manager: IMFDXGIDeviceManager,
    /// Conversion pipeline for one (src, dst) size; only the decode thread
    /// touches this, hence RefCell rather than a lock.
    pipeline: RefCell<Option<Pipeline>>,
}

struct Pipeline {
    key: (u32, u32, u32, u32),
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    output: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    staging: ID3D11Texture2D,
}

impl Gpu {
    /// Create a hardware D3D11 device and register it with Media Foundation.
    ///
    /// Enumerates DXGI adapters to prefer the discrete GPU (NVIDIA / AMD)
    /// instead of the default adapter, which on hybrid laptops is often the
    /// weak integrated GPU.
    pub fn new() -> Result<Gpu, String> {
        unsafe {
            use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIAdapter};

            let factory: IDXGIFactory1 =
                CreateDXGIFactory1().map_err(|e| format!("CreateDXGIFactory1 failed: {e}"))?;

            // Prefer the first adapter whose description contains "NVIDIA" or
            // "AMD" (discrete GPU).  Fall back to the first hardware adapter.
            let mut best: Option<IDXGIAdapter> = None;
            let mut fallback: Option<IDXGIAdapter> = None;
            for i in 0.. {
                let adapter = match factory.EnumAdapters1(i) {
                    Ok(a) => a,
                    Err(_) => break,
                };
                let desc = match adapter.GetDesc1() {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                let name = String::from_utf16_lossy(
                    &desc.Description[..desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len())]
                );
                let lower = name.to_ascii_lowercase();
                if lower.contains("nvidia") || lower.contains("amd") || lower.contains("radeon") {
                    best = Some(adapter.into());
                    break;
                }
                if fallback.is_none() {
                    fallback = Some(adapter.into());
                }
            }

            let adapter = best.or(fallback)
                .ok_or("no DXGI hardware adapter found")?;

            // Log the selected adapter so the user can verify the right GPU.
            if let Ok(d) = adapter.GetDesc() {
                let name = String::from_utf16_lossy(
                    &d.Description[..d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len())]
                );
                crate::logger::log(&format!("GPU: using adapter \"{name}\""));
            }

            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            D3D11CreateDevice(
                Some(&adapter),
                D3D_DRIVER_TYPE_UNKNOWN,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| format!("D3D11CreateDevice failed: {e}"))?;

            let device = device.ok_or("no D3D11 device")?;
            let context = context.ok_or("no D3D11 context")?;
            let video_device = device
                .cast::<ID3D11VideoDevice>()
                .map_err(|e| format!("no D3D11 video device: {e}"))?;
            let video_context = context
                .cast::<ID3D11VideoContext>()
                .map_err(|e| format!("no D3D11 video context: {e}"))?;
            let video_context1 = context.cast::<ID3D11VideoContext1>().ok();

            let mut token = 0u32;
            let mut manager: Option<IMFDXGIDeviceManager> = None;
            windows::Win32::Media::MediaFoundation::MFCreateDXGIDeviceManager(
                &mut token,
                &mut manager,
            )
            .map_err(|e| format!("MFCreateDXGIDeviceManager failed: {e}"))?;
            let manager = manager.ok_or("no DXGI device manager")?;
            manager
                .ResetDevice(&device, token)
                .map_err(|e| format!("ResetDevice failed: {e}"))?;

            Ok(Gpu {
                device,
                context,
                video_device,
                video_context,
                video_context1,
                manager,
                pipeline: RefCell::new(None),
            })
        }
    }

    /// Attach this device to source-reader attributes so decoding happens on
    /// the GPU's video engine.
    pub fn attach_to(&self, attrs: &IMFAttributes) -> Result<(), String> {
        unsafe { attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, &self.manager) }
            .map_err(|e| format!("set D3D manager failed: {e}"))
    }

    /// Convert one decoded NV12 sample surface into tightly packed BGRA bytes
    /// at `(out_w x out_h)`, entirely on the GPU.
    pub fn nv12_sample_to_bgra(
        &self,
        sample: &IMFSample,
        out_w: usize,
        out_h: usize,
    ) -> Result<Vec<u8>, String> {
        // The decoded surface lives in the sample's DXGI buffer; pull the
        // D3D11 texture (and which array slice holds it) out of that buffer.
        let buffer = unsafe { sample.GetBufferByIndex(0).map_err(|e| format!("no buffer: {e}"))? };
        let dxgi_buffer = buffer
            .cast::<IMFDXGIBuffer>()
            .map_err(|e| format!("not a dxgi buffer: {e}"))?;
        let subresource = unsafe {
            dxgi_buffer
                .GetSubresourceIndex()
                .map_err(|e| format!("subresource index failed: {e}"))?
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
            dxgi_buffer
                .GetResource(&ID3D11Texture2D::IID, &mut ptr)
                .map_err(|e| format!("get resource failed: {e}"))?;
            if !ptr.is_null() {
                texture = Some(ID3D11Texture2D::from_raw(ptr));
            }
        }
        let texture = texture.ok_or("no decoded texture")?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { texture.GetDesc(&mut desc) };
        if desc.Format != DXGI_FORMAT_NV12 {
            return Err(format!(
                "decoded surface is not NV12 (format={})",
                desc.Format.0
            ));
        }

        let key = (desc.Width, desc.Height, out_w as u32, out_h as u32);
        let mut pipeline = self.pipeline.borrow_mut();
        if pipeline.as_ref().is_none_or(|p| p.key != key) {
            *pipeline =
                Some(self.build_pipeline(desc.Width, desc.Height, out_w as u32, out_h as u32)?);
        }
        let pipe = pipeline.as_ref().unwrap();

        unsafe {
            let input_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV {
                        MipSlice: 0,
                        ArraySlice: subresource,
                    },
                },
            };
            let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
            self.video_device
                .CreateVideoProcessorInputView(
                    &texture,
                    &pipe.enumerator,
                    &input_desc,
                    Some(&mut input_view),
                )
                .map_err(|e| format!("create input view failed: {e}"))?;
            let input_view = input_view.ok_or("no input view")?;

            let mut stream = D3D11_VIDEO_PROCESSOR_STREAM::default();
            stream.Enable = true.into();
            stream.pInputSurface = std::mem::ManuallyDrop::new(Some(input_view));

            let blt = self.video_context.VideoProcessorBlt(
                &pipe.processor,
                &pipe.output_view,
                0,
                std::slice::from_ref(&stream),
            );
            // Release the view reference we lent to the stream description.
            std::mem::ManuallyDrop::drop(&mut stream.pInputSurface);
            blt.map_err(|e| format!("VideoProcessorBlt failed: {e}"))?;

            // Bring the converted BGRA frame back to CPU memory.
            self.context.CopyResource(&pipe.staging, &pipe.output);
            let mut mapped = windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
            self.context
                .Map(&pipe.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|e| format!("map staging failed: {e}"))?;

            let row_pitch = mapped.RowPitch as usize;
            let row_bytes = out_w * 4;
            let src = std::slice::from_raw_parts(
                mapped.pData.cast(),
                row_pitch.max(1) * out_h,
            );
            let mut out = vec![0u8; row_bytes * out_h];
            for row in 0..out_h {
                let s = row * row_pitch;
                let d = row * row_bytes;
                let n = row_bytes.min(src.len().saturating_sub(s));
                out[d..d + n].copy_from_slice(&src[s..s + n]);
            }
            self.context.Unmap(&pipe.staging, 0);

            Ok(out)
        }
    }

    fn build_pipeline(
        &self,
        src_w: u32,
        src_h: u32,
        out_w: u32,
        out_h: u32,
    ) -> Result<Pipeline, String> {
        unsafe {
            let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
                InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
                InputFrameRate: DXGI_RATIONAL {
                    Numerator: 30,
                    Denominator: 1,
                },
                InputWidth: src_w,
                InputHeight: src_h,
                OutputFrameRate: DXGI_RATIONAL {
                    Numerator: 30,
                    Denominator: 1,
                },
                OutputWidth: out_w,
                OutputHeight: out_h,
                Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
            };
            let enumerator = self
                .video_device
                .CreateVideoProcessorEnumerator(&desc)
                .map_err(|e| format!("create enumerator failed: {e}"))?;
            let processor = self
                .video_device
                .CreateVideoProcessor(&enumerator, 0)
                .map_err(|e| format!("create processor failed: {e}"))?;

            self.video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            if let Some(vc1) = &self.video_context1 {
                vc1.VideoProcessorSetStreamColorSpace1(
                    &processor,
                    0,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                );
                vc1.VideoProcessorSetOutputColorSpace1(
                    &processor,
                    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                );
            }

            let output_desc = D3D11_TEXTURE2D_DESC {
                Width: out_w,
                Height: out_h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut output: Option<ID3D11Texture2D> = None;
            self.device
                .CreateTexture2D(&output_desc, None, Some(&mut output))
                .map_err(|e| format!("create output texture failed: {e}"))?;
            let output = output.ok_or("no output texture")?;

            let output_view_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };
            let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
            self.video_device
                .CreateVideoProcessorOutputView(
                    &output,
                    &enumerator,
                    &output_view_desc,
                    Some(&mut output_view),
                )
                .map_err(|e| format!("create output view failed: {e}"))?;
            let output_view = output_view.ok_or("no output view")?;

            let staging_desc = D3D11_TEXTURE2D_DESC {
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..output_desc
            };
            let mut staging: Option<ID3D11Texture2D> = None;
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|e| format!("create staging texture failed: {e}"))?;
            let staging = staging.ok_or("no staging texture")?;

            Ok(Pipeline {
                key: (src_w, src_h, out_w, out_h),
                enumerator,
                processor,
                output,
                output_view,
                staging,
            })
        }
    }
}
