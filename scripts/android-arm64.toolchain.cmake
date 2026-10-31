# The NDK's toolchain set to 64-bit ARM. Build scripts that hand CMake only a
# toolchain file, as whisper-rs-sys does, would otherwise get its 32-bit
# default ABI.
set(ANDROID_ABI arm64-v8a)
set(ANDROID_PLATFORM android-24)
include($ENV{NDK_HOME}/build/cmake/android.toolchain.cmake)
