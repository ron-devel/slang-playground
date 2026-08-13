plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
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
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation(platform("androidx.compose:compose-bom:2024.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
}
