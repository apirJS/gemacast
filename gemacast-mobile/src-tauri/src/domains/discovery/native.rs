#[cfg(target_os = "android")]
/// Calls the Android Activity's `getTransportType()` method via JNI.
///
/// Returns a pipe-delimited string like `"WIFI|ADB_ON"` indicating the
/// active network transports and ADB status.
pub fn call_native_transport_check(app: &tauri::AppHandle) -> Result<String, String> {
    use std::sync::mpsc;
    use tauri::Manager;

    let (transport_info_tx, transport_info_rx) = mpsc::channel();

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;

    window
        .with_webview(move |webview| {
            #[cfg(target_os = "android")]
            {
                let transport_info_tx = transport_info_tx.clone();
                let _ = webview.jni_handle().exec(move |env, context, _webview| {
                    let result = (|| -> Result<String, String> {
                        let _class = env
                            .get_object_class(&context)
                            .map_err(|e| format!("Failed to get Activity class: {}", e))?;

                        let transport_obj = env
                            .call_method(&context, "getTransportType", "()Ljava/lang/String;", &[])
                            .map_err(|e| {
                                format!("Failed to call getTransportType on Activity: {}", e)
                            })?;

                        let transport_jstr = transport_obj
                            .l()
                            .map_err(|e| format!("Failed to get transport string object: {}", e))?;

                        let transport: String = env
                            .get_string(&transport_jstr.into())
                            .map_err(|e| format!("Failed to extract string from JNI: {}", e))?
                            .into();

                        Ok(transport)
                    })();

                    let _ = transport_info_tx.send(result);
                });
            }
        })
        .map_err(|e| format!("WebView JNI execution failed: {}", e))?;

    transport_info_rx
        .recv()
        .map_err(|e| format!("Failed to receive JNI result: {}", e))?
}

#[cfg(target_os = "android")]
/// Calls the Android Activity's `syncServiceState()` method via JNI.
pub fn call_native_sync_service(
    app: &tauri::AppHandle,
    action: &str,
    is_exclusive: bool,
) -> Result<(), String> {
    use tauri::Manager;

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;

    let action_str = action.to_string();

    window
        .with_webview(move |webview| {
            #[cfg(target_os = "android")]
            {
                let _ = webview.jni_handle().exec(move |env, context, _webview| {
                    let action_jstr = env.new_string(&action_str).unwrap();
                    let _ = env.call_method(
                        &context,
                        "syncServiceState",
                        "(Ljava/lang/String;Z)V",
                        &[
                            jni::objects::JValue::from(&action_jstr),
                            jni::objects::JValue::from(is_exclusive),
                        ],
                    );
                });
            }
        })
        .map_err(|e| format!("WebView JNI execution failed: {}", e))?;

    Ok(())
}
