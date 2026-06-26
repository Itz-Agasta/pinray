use std::sync::{Arc, Condvar, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_foundation::NSError;
use objc2_screen_capture_kit::SCShareableContent;

use pinray_core::{CaptureSource, DisplaySource, PinrayError, Result, SourceId, WindowSource};

/// Synchronously retrieve `SCShareableContent` by blocking on the async completion handler.
pub fn get_shareable_content() -> Result<Retained<SCShareableContent>> {
    let slot: Arc<Mutex<Option<Result<Retained<SCShareableContent>>>>> = Arc::new(Mutex::new(None));
    let cv = Arc::new(Condvar::new());

    let slot2 = Arc::clone(&slot);
    let cv2 = Arc::clone(&cv);

    // RcBlock::new itself is safe; the ObjC call below is unsafe.
    let block = RcBlock::new(
        move |content_ptr: *mut SCShareableContent, error_ptr: *mut NSError| {
            let outcome = if !error_ptr.is_null() {
                let msg = unsafe { &*error_ptr }.localizedDescription().to_string();
                Err(PinrayError::Platform(format!(
                    "SCShareableContent failed: {msg}"
                )))
            } else if !content_ptr.is_null() {
                unsafe {
                    Retained::retain(content_ptr).ok_or_else(|| {
                        PinrayError::Platform("SCShareableContent retain returned nil".into())
                    })
                }
            } else {
                Err(PinrayError::Platform("SCShareableContent is nil".into()))
            };

            *slot2.lock().unwrap() = Some(outcome);
            cv2.notify_one();
        },
    );

    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };

    let mut guard = cv
        .wait_while(slot.lock().unwrap(), |v| v.is_none())
        .unwrap();
    guard.take().unwrap()
}

/// Enumerate capture sources via `SCShareableContent`.
pub fn enumerate_sources() -> Result<Vec<CaptureSource>> {
    let content = get_shareable_content()?;
    let mut sources = Vec::new();

    let displays = unsafe { content.displays() };
    for display in displays.iter() {
        let id = unsafe { display.displayID() };
        let width = unsafe { display.width() } as u32;
        let height = unsafe { display.height() } as u32;

        sources.push(CaptureSource::Display(DisplaySource {
            id: SourceId::new(id.to_string()),
            name: format!("Display {id}"),
            width,
            height,
            scale_factor_milli: 1000,
            is_primary: false,
        }));
    }

    let windows = unsafe { content.windows() };
    for window in windows.iter() {
        let win_id = unsafe { window.windowID() };
        let title = unsafe { window.title() }
            .map(|s| s.to_string())
            .unwrap_or_default();
        let app_name = unsafe { window.owningApplication() }
            .map(|app| unsafe { app.applicationName() }.to_string());

        if title.is_empty() && app_name.is_none() {
            continue;
        }

        sources.push(CaptureSource::Window(WindowSource {
            id: SourceId::new(win_id.to_string()),
            title,
            app_name,
        }));
    }

    Ok(sources)
}
