plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

// Cross-compiles renderer-android (../../renderer/android) via cargo-ndk
// and points AGP's jniLibs source set at its output, so a plain
// `./gradlew assembleDebug` does the whole cross-compile + Android build
// in one shot — no separate manual cargo step to forget. Unlike bridge/'s
// CMake (which imports an already-built library rather than driving
// cargo itself, since CMake has no built-in Rust awareness), this is a
// direct Exec task rather than the `rust-android-gradle` community
// plugin: that plugin's own copy-to-jniLibs step didn't fire for reasons
// not worth chasing further into its bundled source, so this uses the
// exact cargo-ndk invocation already verified to work standalone.
val rustJniLibsDir = layout.buildDirectory.dir("rustJniLibs/android")

val cargoBuildAndroid =
    tasks.register<Exec>("cargoBuildAndroid") {
        workingDir = file("../../renderer")
        commandLine(
            "cargo",
            "ndk",
            "-t",
            "arm64-v8a",
            // Matches minSdk below.
            "-P",
            "26",
            "-o",
            rustJniLibsDir.get().asFile.absolutePath,
            "build",
            "-p",
            "renderer-android",
        )
    }

android {
    namespace = "dev.slangplayground.app"
    // NOTE: package/applicationId are placeholders — rename freely once
    // there's a real decision on app identity.
    compileSdk = 34

    defaultConfig {
        applicationId = "dev.slangplayground.app"
        // Vulkan support on Android is meaningfully consistent from API
        // 26 (Android 8.0) onward; this is a renderer-focused app, so
        // there's little reason to support pre-Vulkan devices.
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    sourceSets.getByName("main") {
        jniLibs.srcDir(rustJniLibsDir)
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation(platform("androidx.compose:compose-bom:2024.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
}

tasks.matching { it.name == "preBuild" }.configureEach {
    dependsOn(cargoBuildAndroid)
}
