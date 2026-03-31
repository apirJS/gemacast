# Android NDK Cross-Compilation Toolchain for armv7

set(ANDROID_ABI "armeabi-v7a")
set(ANDROID_PLATFORM "android-26")
set(ANDROID_NDK "C:/Users/april/AppData/Local/Android/Sdk/ndk/29.0.13846066")

include("C:/Users/april/AppData/Local/Android/Sdk/ndk/29.0.13846066/build/cmake/android.toolchain.cmake")

set(CMAKE_C_COMPILER_WORKS TRUE)
set(CMAKE_CXX_COMPILER_WORKS TRUE)
