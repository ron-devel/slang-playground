// Top-level build file; per-module configuration lives in each module's
// own build.gradle.kts (currently just app/).
plugins {
    id("com.android.application") version "8.6.0" apply false
    id("org.jetbrains.kotlin.android") version "2.0.20" apply false
    // Since Kotlin 2.0, the Compose compiler is a separate Gradle plugin
    // rather than a version pinned via composeOptions.kotlinCompilerExtensionVersion.
    id("org.jetbrains.kotlin.plugin.compose") version "2.0.20" apply false
}
