# Keep Tauri 2 native JNI / plugin classes & Kotlin reflection commands
-keep class io.github.vidgrab.** { *; }
-keep class app.tauri.** { *; }

# Keep youtubedl-android & embedded ffmpeg wrappers
-keep class com.yausername.youtubedl_android.** { *; }
-keep class com.yausername.ffmpeg.** { *; }

-keepclassmembers class * {
    @app.tauri.annotation.Command <methods>;
}
