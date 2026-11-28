plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

// The app's version, which lives only in src-tauri/Cargo.toml. The Tauri CLI
// fills tauri.properties only from a version in tauri.conf.json (there is
// none), so the APK reads Cargo.toml itself, and every update gets a higher
// version code: major * 1000000 + minor * 1000 + patch, as Tauri makes it.
val appVersion: String = Regex("""(?m)^version\s*=\s*"([^"]+)"""")
    .find(rootProject.file("../../Cargo.toml").readText())
    ?.groupValues?.get(1)
    ?: error("No version in src-tauri/Cargo.toml")
val appVersionCode: Int = appVersion.substringBefore('-').split('.').map { it.toInt() }
    .let { (major, minor, patch) -> major * 1000000 + minor * 1000 + patch }

android {
    compileSdk = 36
    namespace = "com.olegg90.bookreader"
    defaultConfig {
        // Catalog servers on the home network often speak plain HTTP (the app
        // warns when that crosses the internet).
        manifestPlaceholders["usesCleartextTraffic"] = "true"
        applicationId = "com.olegg90.bookreader"
        minSdk = 30
        targetSdk = 36
        versionCode = appVersionCode
        versionName = appVersion
    }
    // The release key (the same for every version, so updates install over the
    // previous one) comes from CI: ANDROID_KEYSTORE_FILE, alias "book-reader",
    // one password for the store and the key. A local release build without it
    // is left unsigned.
    val releaseKeystore = System.getenv("ANDROID_KEYSTORE_FILE")
    val releasePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = file(releaseKeystore)
                storePassword = releasePassword
                keyAlias = "book-reader"
                keyPassword = releasePassword
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            if (releaseKeystore != null) signingConfig = signingConfigs.getByName("release")
            isMinifyEnabled = true
            proguardFiles(
                *fileTree(".") { include("**/*.pro") }
                    .plus(getDefaultProguardFile("proguard-android-optimize.txt"))
                    .toList().toTypedArray()
            )
        }
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    buildFeatures {
        buildConfig = true
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = "tauri.build.gradle.kts")