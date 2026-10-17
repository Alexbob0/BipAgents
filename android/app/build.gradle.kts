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
        // Store builds: yy + day of the year + hour and minute, passed by scripts/play-upload.py (a date computed here would
        // be kept by Gradle's configuration cache). Local builds: 1.
        versionCode = providers.gradleProperty("versionCode").orNull?.toInt() ?: 1
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

    // The Play Store upload key: local.properties (upload.storeFile, upload.storePassword, upload.keyAlias,
    // upload.keyPassword), never in the repo. Google re-signs the app with its own key (Play App Signing).
    val local = Properties().apply { rootProject.file("local.properties").takeIf { it.exists() }?.inputStream()?.use(::load) }
    local.getProperty("upload.storeFile")?.let { store ->
        signingConfigs.create("upload") {
            storeFile = file(store)
            storePassword = local.getProperty("upload.storePassword")
            keyAlias = local.getProperty("upload.keyAlias")
            keyPassword = local.getProperty("upload.keyPassword")
        }
    }

    buildTypes {
        release {
            signingConfigs.findByName("upload")?.let { signingConfig = it }
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
        // `-PminifyDebug`: a debug build shrunk as the release is, to try it against the development servers (plain HTTP,
        // debug hooks) and catch what R8 removes before Play does.
        debug {
            if (providers.gradleProperty("minifyDebug").isPresent) {
                isMinifyEnabled = true
                proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            }
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
    implementation(libs.biometric)
    implementation(libs.fragment.ktx)
    implementation(libs.media3.exoplayer)
    implementation(libs.media3.session)
    implementation(libs.media3.ui)
    implementation(libs.glance.appwidget)
    implementation(libs.glance.material3)
    implementation(libs.docscan)
    implementation(libs.zxing)
    implementation(platform(libs.firebase.bom))
    implementation(libs.firebase.messaging)
    implementation(libs.coroutines.play.services)
    debugImplementation(libs.compose.ui.tooling)
    testImplementation(libs.junit)
    testImplementation(libs.okhttp.mockwebserver)
}
