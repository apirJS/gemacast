/// Android-specific APK installation using JNI.
///
/// Uses `ACTION_VIEW` with MIME type `application/vnd.android.package-archive`
/// (the modern replacement for the deprecated `ACTION_INSTALL_PACKAGE`) and a
/// `content://` URI from the app's `FileProvider`. The user will see the system
/// install prompt.
#[cfg(target_os = "android")]
pub fn install_apk_android(app: &tauri::AppHandle, path: &str) -> Result<(), String> {
    use jni::objects::{JObject, JValue};
    use std::sync::{Arc, Mutex};
    use tauri::Manager;

    let webview_window = app.get_webview_window("main").ok_or("No main webview")?;

    let path_owned = path.to_string();

    // We use a shared error slot to propagate errors out of the `with_webview`
    // closure, since the closure cannot return a `Result` directly.
    let error_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let error_slot_inner = error_slot.clone();
    let path_owned = path.to_string();

    activity
        .with_webview(move |webview| {
            let result = (|| -> Result<(), String> {
                let env = webview.jni_env();
                let activity = webview.activity();

                // Steps:
                //   1. Create a java.io.File from the path
                //   2. Get a content:// URI via FileProvider.getUriForFile()
                //   3. Create an ACTION_VIEW intent with setDataAndType()
                //   4. Add FLAG_GRANT_READ_URI_PERMISSION | FLAG_ACTIVITY_NEW_TASK
                //   5. Start the activity

                let j_path = env
                    .new_string(&path_owned)
                    .map_err(|e| format!("Failed to create Java string for APK path: {e}"))?;

                // new java.io.File(path)
                let file_class = env
                    .find_class("java/io/File")
                    .map_err(|e| format!("File class not found: {e}"))?;
                let file_obj = env
                    .new_object(
                        file_class,
                        "(Ljava/lang/String;)V",
                        &[JValue::Object(&j_path)],
                    )
                    .map_err(|e| format!("Failed to create File object: {e}"))?;

                // Get the application context
                let get_app_context = env
                    .call_method(
                        &activity,
                        "getApplicationContext",
                        "()Landroid/content/Context;",
                        &[],
                    )
                    .map_err(|e| format!("getApplicationContext failed: {e}"))?
                    .l()
                    .map_err(|e| format!("getApplicationContext returned non-object: {e}"))?;

                // Get authority string: "<package>.fileprovider"
                let get_package = env
                    .call_method(
                        &get_app_context,
                        "getPackageName",
                        "()Ljava/lang/String;",
                        &[],
                    )
                    .map_err(|e| format!("getPackageName failed: {e}"))?
                    .l()
                    .map_err(|e| format!("getPackageName returned non-object: {e}"))?;

                let package_name: String = env
                    .get_string((&get_package).into())
                    .map_err(|e| format!("Failed to convert package name: {e}"))?
                    .into();

                let authority = format!("{package_name}.fileprovider");
                let j_authority = env
                    .new_string(&authority)
                    .map_err(|e| format!("Failed to create authority string: {e}"))?;

                // FileProvider.getUriForFile(context, authority, file)
                let fp_class = env
                    .find_class("androidx/core/content/FileProvider")
                    .map_err(|e| format!("FileProvider class not found: {e}"))?;
                let content_uri = env
                    .call_static_method(
                        fp_class,
                        "getUriForFile",
                        "(Landroid/content/Context;Ljava/lang/String;Ljava/io/File;)Landroid/net/Uri;",
                        &[
                            JValue::Object(&get_app_context),
                            JValue::Object(&j_authority),
                            JValue::Object(&file_obj),
                        ],
                    )
                    .map_err(|e| format!("getUriForFile failed: {e}"))?
                    .l()
                    .map_err(|e| format!("getUriForFile returned non-object: {e}"))?;

                // Create an ACTION_VIEW intent (modern replacement for deprecated ACTION_INSTALL_PACKAGE)
                let intent_class = env
                    .find_class("android/content/Intent")
                    .map_err(|e| format!("Intent class not found: {e}"))?;
                let action = env
                    .new_string("android.intent.action.VIEW")
                    .map_err(|e| format!("Failed to create action string: {e}"))?;
                let intent = env
                    .new_object(
                        intent_class,
                        "(Ljava/lang/String;)V",
                        &[JValue::Object(&action)],
                    )
                    .map_err(|e| format!("Failed to create Intent: {e}"))?;

                // intent.setDataAndType(contentUri, "application/vnd.android.package-archive")
                let mime_type = env
                    .new_string("application/vnd.android.package-archive")
                    .map_err(|e| format!("Failed to create MIME type string: {e}"))?;
                let _ = env
                    .call_method(
                        &intent,
                        "setDataAndType",
                        "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;",
                        &[
                            JValue::Object(&content_uri),
                            JValue::Object(&mime_type),
                        ],
                    )
                    .map_err(|e| format!("setDataAndType failed: {e}"))?;

                // intent.addFlags(FLAG_GRANT_READ_URI_PERMISSION | FLAG_ACTIVITY_NEW_TASK)
                // FLAG_GRANT_READ_URI_PERMISSION = 1
                // FLAG_ACTIVITY_NEW_TASK = 0x10000000
                let flags: i32 = 1 | 0x10000000;
                let _ = env
                    .call_method(
                        &intent,
                        "addFlags",
                        "(I)Landroid/content/Intent;",
                        &[JValue::Int(flags)],
                    )
                    .map_err(|e| format!("addFlags failed: {e}"))?;

                // context.startActivity(intent)
                env.call_method(
                    &activity,
                    "startActivity",
                    "(Landroid/content/Intent;)V",
                    &[JValue::Object(&intent)],
                )
                .map_err(|e| format!("startActivity failed: {e}"))?;

                Ok(())
            })();

            if let Err(e) = result {
                *error_slot_inner.lock().unwrap() = Some(e);
            }
        })
        .map_err(|e| format!("Failed to access webview: {e:?}"))?;

    // Check if the JNI closure reported an error.
    if let Some(err) = error_slot.lock().unwrap().take() {
        return Err(err);
    }

    Ok(())
}
