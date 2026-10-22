plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}
android {
    namespace = "org.fm1.android"
    compileSdk = 35
    defaultConfig {
        applicationId = "org.fm1.android"
        minSdk = 26
        targetSdk = 35
        versionCode = 2
        versionName = "0.2.0"
        manifestPlaceholders["appLabel"] = "FM-1"
        testApplicationId = if (providers.gradleProperty("fm1Performance").isPresent)
            "org.fm1.android.benchmark.tests" else "org.fm1.android.tests"
        testInstrumentationRunner = "org.fm1.android.EngineInstrumentation"
        ndk { abiFilters += "arm64-v8a" }
    }
    buildTypes {
        getByName("debug") { signingConfig = null }
        create("benchmark") {
            initWith(getByName("debug"))
            applicationIdSuffix = ".benchmark"
            manifestPlaceholders["appLabel"] = "FM-1 Benchmark"
            matchingFallbacks += "debug"
        }
    }
    testBuildType = if (providers.gradleProperty("fm1Performance").isPresent) "benchmark" else "debug"
    sourceSets.getByName("main") {
        assets.srcDir(rootProject.layout.buildDirectory.dir("assets"))
        jniLibs.srcDir(rootProject.layout.buildDirectory.dir("lib"))
    }
    sourceSets.getByName("androidTest") {
        assets.srcDir(rootProject.layout.buildDirectory.dir("test/assets"))
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    kotlinOptions { jvmTarget = "1.8" }
    packaging { jniLibs.useLegacyPackaging = true }
}
dependencies {
    implementation("com.google.android.material:material:1.13.0")
}

dependencyLocking { lockAllConfigurations() }
