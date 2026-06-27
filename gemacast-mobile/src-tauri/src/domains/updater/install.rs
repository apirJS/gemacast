/// Android-specific APK installation using JNI.
///
/// This calls the Android `Intent.ACTION_INSTALL_PACKAGE` with a content URI
/// from the app's `FileProvider`. The user will see the system install prompt.
#[cfg(target_os = "android")]
pub fn install_apk_android(app: &tauri::AppHandle, path: &str) -> Result<(), String> {
    use jni::objects::JValue;
    use tauri::Manager;

    let webview_window = app.get_webview_window("main").ok_or("No main webview")?;

    // Run on the Android activity's JNI environment.
    webview_window
        .with_webview(move |webview| {
            webview
                .jni_handle()
                .exec(move |env, activity, _webview| {
                    let path_str = path.to_string();

                    // We need to call Java code to trigger the install intent.
                    // Steps:
                    //   1. Create a java.io.File from the path
                    //   2. Get a content:// URI via FileProvider.getUriForFile()
                    //   3. Create an ACTION_INSTALL_PACKAGE intent
                    //   4. Set data and type, add FLAG_GRANT_READ_URI_PERMISSION
                    //   5. Start the activity

                    let j_path = env
                        .new_string(&path_str)
                        .expect("Failed to create Java string for APK path");

                    // new java.io.File(path)
                    let file_class = env
                        .find_class("java/io/File")
                        .expect("File class not found");
                    let file_obj = env
                        .new_object(
                            file_class,
                            "(Ljava/lang/String;)V",
                            &[JValue::Object(&j_path)],
                        )
                        .expect("Failed to create File object");

                    // Get the application context
                    let context_class = env
                        .find_class("android/content/Context")
                        .expect("Context class not found");
                    let get_app_context = env
                        .call_method(
                            &activity,
                            "getApplicationContext",
                            "()Landroid/content/Context;",
                            &[],
                        )
                        .expect("getApplicationContext failed")
                        .l()
                        .expect("getApplicationContext returned non-object");

                    // Get authority string: "<package>.fileprovider"
                    let get_package = env
                        .call_method(
                            &get_app_context,
                            "getPackageName",
                            "()Ljava/lang/String;",
                            &[],
                        )
                        .expect("getPackageName failed")
                        .l()
                        .expect("getPackageName returned non-object");

                    let package_name: String = env
                        .get_string((&get_package).into())
                        .expect("Failed to convert package name")
                        .into();

                    let authority = format!("{package_name}.fileprovider");
                    let j_authority = env
                        .new_string(&authority)
                        .expect("Failed to create authority string");

                    // FileProvider.getUriForFile(context, authority, file)
                    let fp_class = env
                        .find_class("androidx/core/content/FileProvider")
                        .expect("FileProvider class not found");
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
                .expect("getUriForFile failed")
                .l()
                .expect("getUriForFile returned non-object");

                    // Create install intent
                    let intent_class = env
                        .find_class("android/content/Intent")
                        .expect("Intent class not found");
                    let action = env
                        .new_string("android.intent.action.INSTALL_PACKAGE")
                        .expect("Failed to create action string");
                    let intent = env
                        .new_object(
                            intent_class,
                            "(Ljava/lang/String;)V",
                            &[JValue::Object(&action)],
                        )
                        .expect("Failed to create Intent");

                    // intent.setData(contentUri)
                    let _ = env
                        .call_method(
                            &intent,
                            "setData",
                            "(Landroid/net/Uri;)Landroid/content/Intent;",
                            &[JValue::Object(&content_uri)],
                        )
                        .expect("setData failed");

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
                        .expect("addFlags failed");

                    // intent.putExtra(Intent.EXTRA_NOT_UNKNOWN_SOURCE, true)
                    let extra_key = env
                        .new_string("android.intent.extra.NOT_UNKNOWN_SOURCE")
                        .expect("Failed to create extra key");
                    let _ = env
                        .call_method(
                            &intent,
                            "putExtra",
                            "(Ljava/lang/String;Z)Landroid/content/Intent;",
                            &[JValue::Object(&extra_key), JValue::Bool(1)],
                        )
                        .expect("putExtra failed");

                    // context.startActivity(intent)
                    let _ = env
                        .call_method(
                            &activity,
                            "startActivity",
                            "(Landroid/content/Intent;)V",
                            &[JValue::Object(&intent)],
                        )
                        .expect("startActivity failed");
                })
                .map_err(|e| format!("JNI error: {e:?}"))
        })
        .map_err(|e| e.to_string())??;

    Ok(())
}
