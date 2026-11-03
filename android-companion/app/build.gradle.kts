import java.util.Properties

val signing = Properties().apply {
    val config = rootProject.file("signing/signing.properties")
    if (config.isFile) config.inputStream().use { load(it) }
}
fun signingValue(name: String, environment: String): String? = providers.environmentVariable(environment).orNull ?: signing.getProperty(name)

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.linktolinux.companion"
    compileSdk = 34

    defaultConfig {
        applicationId = "dev.linktolinux.companion"
        minSdk = 24
        targetSdk = 34
        versionCode = 7
        versionName = "1.0.0"
    }

    signingConfigs {
        create("release") {
            val location = signingValue("storeFile", "LINKTOLINUX_KEYSTORE")
            if (location != null) storeFile = rootProject.file(location)
            storePassword = signingValue("storePassword", "LINKTOLINUX_KEYSTORE_PASSWORD")
            keyAlias = signingValue("keyAlias", "LINKTOLINUX_KEY_ALIAS")
            keyPassword = signingValue("keyPassword", "LINKTOLINUX_KEY_PASSWORD")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            signingConfig = signingConfigs.getByName("release")
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
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
    implementation("androidx.core:core:1.13.1")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}
