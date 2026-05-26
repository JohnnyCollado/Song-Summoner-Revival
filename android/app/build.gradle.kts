/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
import org.gradle.nativeplatform.platform.internal.DefaultNativePlatform

plugins {
    id("com.android.application") version("8.10.1")
    id("com.github.willir.rust.cargo-ndk-android") version("0.3.4")
    id("org.jetbrains.kotlin.android") version("2.0.21")
}

fun runTouchHLEVersionTool(wantBranding: Boolean): String {
    val output = providers.exec {
        commandLine("cargo", "run", "--package", "touchHLE_version")
        if (wantBranding) {
            args("--", "--branding")
        }
    }.standardOutput.asText.get().trim()

    return output
}

fun getTouchHLEBranding(): String {
    return runTouchHLEVersionTool(/* wantBranding: */ true)
}

fun getTouchHLEVersionName(): String {
    return runTouchHLEVersionTool(/* wantBranding: */ false)
}

fun join(prefix: String, separator: String, branding: String): String {
    return if (branding.isEmpty()) prefix else prefix + separator + branding
}

android {
    ndkVersion = "30.0.14904198"
    compileSdk = 31
    buildFeatures {
        buildConfig = true
    }
    defaultConfig {
        val branding = getTouchHLEBranding()
        applicationId = "org.touchhle.android"
        if (!branding.isEmpty()) {
            applicationIdSuffix = branding.lowercase()
        }
        resValue("string", "app_name", join("touchHLE", " ", branding))
        buildConfigField("String", "APP_NAME", "\"${join("touchHLE", " ", branding)}\"")
        manifestPlaceholders["icon"] = join("@drawable/icon", "_", branding.lowercase())
        buildConfigField("int", "APP_ICON", join("R.drawable.icon", "_", branding.lowercase()))
        versionName = join(getTouchHLEVersionName(), " ", branding)

        // Default-flavor BuildConfig fields — overridden in the
        // `songsummoner` flavor so the same MainActivity code can branch
        // on whether it's the generic emulator build or the standalone
        // game-wrapper build.
        buildConfigField("boolean", "WRAPPER_AUTO_LAUNCH", "false")
        buildConfigField("String",  "WRAPPER_IPA_ASSET",   "\"\"")
        buildConfigField("String",  "WRAPPER_IPA_FILENAME", "\"\"")

        minSdk = 21 // first version with AArch64
        targetSdk = 31
        externalNativeBuild {
            ndkBuild {
                arguments("APP_PLATFORM=android-21")
                // abiFilters 'armeabi-v7a', 'arm64-v8a', 'x86', 'x86_64'
                // Only 'arm64-v8a' and 'x86_64' are supported by dynarmic
                // and hence touchHLE. The 'x86_64' build works, but the main
                // use for that would be the emulator in Android Studio, and
                // its OpenGL ES implementations don't seem to work properly
                // with touchHLE, so we disable it to reduce build time and
                // avoid shipping stuff we haven't meaningfully tested.
                // Make sure this matches the cargoNdk targets below.
                abiFilters("arm64-v8a")
            }
        }
    }
    // The target JVM version must be the same for Java and Kotlin.
    compileOptions {
        sourceCompatibility(JavaVersion.VERSION_11)
        targetCompatibility(JavaVersion.VERSION_11)
    }
    kotlinOptions {
        jvmTarget = "11"
    }
    // Product flavors let us ship two artifacts from one codebase:
    //   - `touchhle`     -- the generic emulator with an app picker
    //   - `songsummoner` -- a standalone wrapper that bundles the
    //                       Song Summoner IPA in assets/ and skips the
    //                       picker, jumping straight into the game.
    // The Java side branches on BuildConfig.WRAPPER_AUTO_LAUNCH.
    flavorDimensions += "distribution"
    productFlavors {
        create("touchhle") {
            dimension = "distribution"
            isDefault = true
        }
        create("songsummoner") {
            dimension = "distribution"
            // Distinct package id so both flavors can coexist on one device.
            applicationIdSuffix = ".songsummoner"
            versionNameSuffix = "-songsummoner"
            // App name shown under the launcher icon.
            resValue("string", "app_name", "Song Summoner")
            buildConfigField("String",  "APP_NAME", "\"Song Summoner\"")
            // Wrapper behaviour switches.
            buildConfigField("boolean", "WRAPPER_AUTO_LAUNCH", "true")
            buildConfigField("String",  "WRAPPER_IPA_ASSET",   "\"song_summoner.ipa\"")
            // Filename used when copying to internal storage; the .ipa
            // extension matters because touchHLE's bundle loader sniffs it.
            buildConfigField("String",  "WRAPPER_IPA_FILENAME",
                "\"Song Summoner The Unsung Heroes Encore.ipa\"")
        }
    }

    // The IPA is already a compressed zip; AAPT compressing it again
    // would double-process ~260 MB for no gain and slow down install /
    // first-launch extraction. Mark it noCompress.
    androidResources {
        noCompress.add("ipa")
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.getByName("debug")
            isMinifyEnabled = false
            isDebuggable = true // allow use of ADB to manage files, etc
        }
        debug {
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("**/*.so")
            }
            isDebuggable = true
            isJniDebuggable = true
        }
    }

    applicationVariants.all {
        val variantName = name.replaceFirstChar { char ->
            if (char.isLowerCase()) char.titlecase() else char.toString()
        }
        tasks.named("merge${variantName}Assets").configure {
            dependsOn("externalNativeBuild${variantName}")
        }
    }

    sourceSets {
        getByName("main") {
            java.srcDir("${rootDir.parentFile}/vendor/SDL/android-project/app/src/main/java")
        }
    }

    if (!project.hasProperty("EXCLUDE_NATIVE_LIBS")) {
        sourceSets {
            getByName("main") {
                jniLibs.srcDir("${projectDir}/jniLibs")
            }
        }
        externalNativeBuild {
            ndkBuild {
                path("jni/Android.mk")
            }
        }
    }

    lint {
        abortOnError = false
    }
    namespace = "org.touchhle.android"
}

cargoNdk {
    // Make sure this matches the android abiFilters above.
    targets = arrayListOf("arm64")
    module = ".."
    librariesNames = arrayListOf("libtouchHLE.so", "libSDL2.so", "libc++_shared.so")
    extraCargoEnv = mapOf(
        "ANDROID_NDK" to android.ndkDirectory.toString(),
        "ANDROID_NDK_HOME" to android.ndkDirectory.toString(),
    )

    if (DefaultNativePlatform.host().operatingSystem.isWindows) {
        val binPath =
            android.ndkDirectory.toPath().resolve("toolchains/llvm/prebuilt/windows-x86_64/bin")
        val clangPath = binPath.resolve("clang.exe")
        val clangXXPath = binPath.resolve("clang++.exe")

        if (!clangPath.toFile().exists()) {
            throw GradleException("NDK clang compiler not found at expected location: $clangPath")
        }
        if (!clangXXPath.toFile().exists()) {
            throw GradleException("NDK clang++ compiler not found at expected location: $clangXXPath")
        }

        extraCargoEnv.putAll(
            mapOf(
                "CC" to clangPath.toString(),
                "CXX" to clangXXPath.toString(),
                // The default generator on Windows (Visual Studio) does not respect
                // the CC and CXX environment variables. Using Ninja ensures that
                // the specified compilers are used
                "CMAKE_GENERATOR" to "Ninja",
            )
        )
    }
    // The default feature, "static", makes us use static linking for SDL2 and OpenAL Soft.
    // For Android, we need dynamic linking for SDL2, but static linking for OpenAL Soft.
    extraCargoBuildArguments = arrayListOf(
        "--lib",
        "--no-default-features",
        "--features",
        "touchHLE_openal_soft_wrapper/static,sdl2/bundled"
    )
}

dependencies {
    implementation(fileTree("libs") {
        include("*.jar")
    })
}
