# Keep Tauri 2 native JNI / plugin classes & Kotlin reflection commands
-keep class io.github.vidgrab.** { *; }
-keep class app.tauri.** { *; }

# Keep youtubedl-android, ffmpeg, common and dependencies
-keep class com.yausername.** { *; }
-keep class org.apache.commons.io.** { *; }
-keep class org.apache.commons.compress.** { *; }
-keep class com.fasterxml.jackson.** { *; }
-keep class kotlin.** { *; }
-keep class kotlinx.** { *; }

-keepclassmembers class * {
    @app.tauri.annotation.Command <methods>;
}

-dontwarn org.apache.commons.io.**
-dontwarn org.apache.commons.compress.**
-dontwarn com.fasterxml.jackson.**
-dontwarn com.yausername.**
