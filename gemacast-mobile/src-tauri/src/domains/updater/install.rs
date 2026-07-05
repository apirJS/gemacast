/// Android-specific APK installation via a Kotlin helper on the Activity.
///
/// Delegates to `MainActivity.installApk(path)` which handles the
/// `FileProvider.getUriForFile()` → `ACTION_VIEW` intent flow entirely in
/// Kotlin.
///
/// **Why Kotlin instead of pure JNI?**
/// `with_webview` / `jni_handle().exec()` runs the closure on a native thread
/// whose class loader is the **boot class loader**. The boot class loader can
/// only see Android framework classes (`android.*`, `java.*`), NOT application-
/// level classes like `androidx.core.content.FileProvider`. Calling
/// `env.find_class("androidx/core/content/FileProvider")` from this context
/// throws `java.lang.NoClassDefFoundError`. By keeping all AndroidX usage in
/// Kotlin, the app's own class loader resolves the class normally.
///
/// **Important**: `with_webview` dispatches the JNI closure asynchronously on
/// the WebView thread. We use a `Condvar` to block this function until the
/// closure has finished, ensuring errors are properly propagated and the APK
/// file isn't cleaned up prematurely.
#[cfg(target_os = "android")]
pub fn install_apk_android(app: &tauri::AppHandle, path: &str) -> Result<(), String> {
    use std::sync::{Arc, Condvar, Mutex};
    use tauri::Manager;

    let webview_window = app.get_webview_window("main").ok_or("No main webview")?;

    // Shared state: the Mutex holds `Option<Result<(), String>>`.
    // - `None`  = the JNI closure hasn't finished yet.
    // - `Some(result)` = the JNI closure has finished with this result.
    let pair = Arc::new((Mutex::new(None::<Result<(), String>>), Condvar::new()));
    let pair_inner = pair.clone();
    let path_owned = path.to_string();

    webview_window
        .with_webview(move |webview| {
            webview.jni_handle().exec(move |env, activity, _webview| {
                let result = (|| -> Result<(), String> {
                    let j_path = env
                        .new_string(&path_owned)
                        .map_err(|e| format!("Failed to create Java string for APK path: {e}"))?;

                    // Call MainActivity.installApk(path) — all FileProvider/Intent
                    // logic lives in Kotlin where the app class loader works.
                    env.call_method(
                        activity,
                        "installApk",
                        "(Ljava/lang/String;)V",
                        &[(&j_path).into()],
                    )
                    .map_err(|e| format!("installApk failed: {e}"))?;

                    Ok(())
                })();

                // Signal completion to the waiting thread.
                let (lock, cvar) = &*pair_inner;
                *lock.lock().unwrap() = Some(result);
                cvar.notify_one();
            });
        })
        .map_err(|e| format!("Failed to access webview: {e:?}"))?;

    // Block until the JNI closure has finished executing on the WebView thread.
    let (lock, cvar) = &*pair;
    let mut guard = lock.lock().unwrap();
    while guard.is_none() {
        guard = cvar.wait(guard).unwrap();
    }

    // Unwrap the result from the JNI closure.
    guard.take().unwrap()
}
