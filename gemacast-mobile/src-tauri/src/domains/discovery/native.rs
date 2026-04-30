#[cfg(target_os = "android")]
pub fn call_native_transport_check(app: &tauri::AppHandle) -> Result<String, String> {
    use std::sync::mpsc;
    use tauri::Manager;

    let (tx, rx) = mpsc::channel();

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "Failed to find main webview window".to_string())?;

    window
        .with_webview(move |webview| {
            #[cfg(target_os = "android")]
            {
                let tx = tx.clone();
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

                    let _ = tx.send(result);
                });
            }
        })
        .map_err(|e| format!("WebView JNI execution failed: {}", e))?;

    rx.recv()
        .map_err(|e| format!("Failed to receive JNI result: {}", e))?
}
