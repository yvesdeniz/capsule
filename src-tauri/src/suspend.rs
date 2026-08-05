#[cfg(target_os = "windows")]
mod imp {
    use tauri::WebviewWindow;
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
    use webview2_com::TrySuspendCompletedHandler;
    use windows::core::Interface;

    pub fn suspend(window: &WebviewWindow) {
        let label = window.label().to_string();
        let _ = window.with_webview(move |webview| unsafe {
            let controller = webview.controller();
            let Ok(core) = controller.CoreWebView2() else {
                return;
            };
            let Ok(core3) = core.cast::<ICoreWebView2_3>() else {
                tracing::debug!(window = %label, "runtime predates ICoreWebView2_3");
                return;
            };
            if controller.SetIsVisible(false).is_err() {
                return;
            }
            let handler = TrySuspendCompletedHandler::create(Box::new(move |result, ok| {
                match result {
                    Ok(()) => tracing::debug!(window = %label, suspended = ok, "webview suspend"),
                    Err(e) => tracing::warn!(window = %label, error = %e, "webview suspend"),
                }
                Ok(())
            }));
            if core3.TrySuspend(&handler).is_err() {
                let _ = controller.SetIsVisible(true);
            }
        });
    }

    pub fn resume(window: &WebviewWindow) {
        let _ = window.with_webview(|webview| unsafe {
            let controller = webview.controller();
            if let Ok(core) = controller.CoreWebView2() {
                if let Ok(core3) = core.cast::<ICoreWebView2_3>() {
                    let _ = core3.Resume();
                }
            }
            let _ = controller.SetIsVisible(true);
        });
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use tauri::WebviewWindow;

    pub fn suspend(_window: &WebviewWindow) {}
    pub fn resume(_window: &WebviewWindow) {}
}

pub use imp::{resume, suspend};
