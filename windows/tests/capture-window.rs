//! Read a dashboard fixture's DWM-composed client pixels independently of its renderer.
#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::{mem::size_of, sync::mpsc, time::Duration};
    use windows::{
        Foundation::TypedEventHandler,
        Graphics::{
            Capture::*,
            DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat},
        },
        Win32::{
            Foundation::{HMODULE, HWND, POINT, RECT},
            Graphics::{
                Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
                Direct3D11::*,
                Dwm::*,
                Dxgi::IDXGIDevice,
                Gdi::ClientToScreen,
            },
            System::WinRT::{Direct3D11::*, Graphics::Capture::IGraphicsCaptureItemInterop, *},
            UI::{HiDpi::*, WindowsAndMessaging::GetClientRect},
        },
        core::{Interface, factory},
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Expected a window handle and output PNG".into());
    }
    let hwnd = HWND(args[0].to_str().ok_or("Invalid handle")?.parse::<usize>()? as _);
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { RoUninitialize() };
        }
    }
    let _apartment = Apartment;
    if !GraphicsCaptureSession::IsSupported()? {
        return Err("Windows Graphics Capture is unavailable".into());
    }
    let mut device = None;
    let mut context = None;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let result = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        if result.is_ok() {
            break;
        }
        if driver == D3D_DRIVER_TYPE_WARP {
            result?;
        }
    }
    let device = device.ok_or("Missing capture device")?;
    let context = context.ok_or("Missing graphics context")?;
    let winrt: IDirect3DDevice =
        unsafe { CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)? }.cast()?;
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &winrt,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        item.Size()?,
    )?;
    let session = pool.CreateCaptureSession(&item)?;
    session.SetIsCursorCaptureEnabled(false)?;
    let (send, receive) = mpsc::sync_channel(1);
    let token = pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
        let _ = send.try_send(());
        Ok(())
    }))?;
    session.StartCapture()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(4);
    let (width, height, pixels) = loop {
        if receive
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .is_err()
        {
            return Err("Windows did not present a populated frame for this window".into());
        }
        let frame = pool.TryGetNextFrame()?;
        let content = frame.ContentSize()?;
        let surface: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
        let mut outer = RECT::default();
        let mut client = RECT::default();
        let mut origin = POINT::default();
        let (width, height, pixels) = unsafe {
            DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut outer as *mut RECT).cast(),
                size_of::<RECT>() as u32,
            )?;
            GetClientRect(hwnd, &mut client)?;
            ClientToScreen(hwnd, &mut origin).ok()?;
            let (x, y, w, h) = (
                origin.x - outer.left,
                origin.y - outer.top,
                client.right,
                client.bottom,
            );
            if x < 0
                || y < 0
                || w < 100
                || h < 100
                || x + w > content.Width
                || y + h > content.Height
            {
                return Err(format!(
                    "Client {x},{y} {w}×{h} is outside the composed {}×{} frame",
                    content.Width, content.Height
                )
                .into());
            }
            let texture: ID3D11Texture2D = surface.GetInterface()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if (x + w) as u32 > desc.Width || (y + h) as u32 > desc.Height {
                return Err("Capture texture changed size".into());
            }
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.MiscFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut staging = None;
            device.CreateTexture2D(&desc, None, Some(&mut staging))?;
            let staging = staging.ok_or("Missing capture texture")?;
            context.CopyResource(&staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let mut pixels = Vec::with_capacity(w as usize * h as usize * 3);
            for row in y..y + h {
                let from = mapped
                    .pData
                    .cast::<u8>()
                    .add(row as usize * mapped.RowPitch as usize + x as usize * 4);
                for p in std::slice::from_raw_parts(from, w as usize * 4)
                    .as_chunks::<4>()
                    .0
                {
                    pixels.extend_from_slice(&[p[2], p[1], p[0]]);
                }
            }
            context.Unmap(&staging, 0);
            (w as u32, h as u32, pixels)
        };
        frame.Close()?;
        let rgb = pixels.as_chunks::<3>().0;
        let center = rgb[rgb.len() / 2 + width as usize / 2];
        let content = rgb
            .iter()
            .filter(|p| p.iter().zip(center).any(|(&v, bg)| v.abs_diff(bg) > 16))
            .count();
        // A remote session can publish a white client with just a handful of frame-edge pixels.
        if content > rgb.len() / 500 {
            break (width, height, pixels);
        }
        if std::time::Instant::now() >= deadline {
            return Err("Windows returned a blank compositor surface for this window".into());
        }
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::RedrawWindow(
                Some(hwnd),
                None,
                None,
                windows::Win32::Graphics::Gdi::RDW_INVALIDATE
                    | windows::Win32::Graphics::Gdi::RDW_ALLCHILDREN,
            );
        }
    };
    pool.RemoveFrameArrived(token)?;
    session.Close()?;
    pool.Close()?;
    image::save_buffer(&args[1], &pixels, width, height, image::ColorType::Rgb8)?;
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
