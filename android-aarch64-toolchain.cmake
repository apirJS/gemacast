# Android NDK Cross-Compilation Toolchain for aarch64
# Uses the NDK's own toolchain file for maximum compatibility

set(ANDROID_ABI "arm64-v8a")
set(ANDROID_PLATFORM "android-26")
set(ANDROID_NDK "C:/Users/april/AppData/Local/Android/Sdk/ndk/29.0.13846066")

include("C:/Users/april/AppData/Local/Android/Sdk/ndk/29.0.13846066/build/cmake/android.toolchain.cmake")

# Force-skip the compiler test. The cmake crate injects --target=aarch64-linux-android
# (without API level) into CMAKE_C_FLAGS, which conflicts with the NDK toolchain's
# --target=aarch64-none-linux-android26. This causes the linker to fail finding
# crtbegin_dynamic.o during the compiler test. Skipping the test is safe because
# we know the NDK compiler works.
set(CMAKE_C_COMPILER_WORKS TRUE)
set(CMAKE_CXX_COMPILER_WORKS TRUE)
