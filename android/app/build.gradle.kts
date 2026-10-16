import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

android {
    namespace = "io.github.bipagents"
    compileSdk = 37

    defaultConfig {
        applicationId = "io.github.bipagents"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1"
        // Notifications: the publisher's Firebase project (local.properties or ~/.gradle/gradle.properties:
        // firebase.appId, firebase.apiKey, firebase.projectId, firebase.senderId). Without them, no notifications.
        val local = Properties().apply { rootProject.file("local.properties").takeIf { it.exists() }?.inputStream()?.use(::load) }
        fun setting(name: String) = (local.getProperty(name) ?: providers.gradleProperty(name).orNull ?: "").replace("\"", "")
        buildConfigField("String", "FIREBASE_APP_ID", "\"${setting("firebase.appId")}\"")
        buildConfigField("String", "FIREBASE_API_KEY", "\"${setting("firebase.apiKey")}\"")
        buildConfigField("String", "FIREBASE_PROJECT_ID", "\"${setting("firebase.projectId")}\"")
        buildConfigField("String", "FIREBASE_SENDER_ID", "\"${setting("firebase.senderId")}\"")
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    testOptions { unitTests.isReturnDefaultValues = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { jvmToolchain(17) }

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    implementation(libs.activity.compose)
    implementation(libs.lifecycle.viewmodel.compose)
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.navigation.compose)
    implementation(libs.okhttp)
    implementation(libs.serialization.json)
    implementation(libs.camera.camera2)
    implementation(libs.camera.lifecycle)
    implementation(libs.camera.view)
    implementation(libs.mlkit.barcode)
    implementation(libs.coil.compose)
    implementation(libs.core.ktx)
    implementation(platform(libs.firebase.bom))
    implementation(libs.firebase.messaging)
    implementation(libs.coroutines.play.services)
    debugImplementation(libs.compose.ui.tooling)
    testImplementation(libs.junit)
    testImplementation(libs.okhttp.mockwebserver)
}
