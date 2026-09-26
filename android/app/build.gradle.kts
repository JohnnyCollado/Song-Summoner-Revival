/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * Parts of this file are derived from SDL 2's Android project template, which
 * has a different license. Please see vendor/SDL/LICENSE.txt for details.
 */
import javax.inject.Inject
import org.gradle.nativeplatform.platform.internal.DefaultNativePlatform
import org.gradle.process.ExecOperations

plugins {
    id("com.android.application") version("8.13.0")
    id("org.jetbrains.kotlin.android") version("2.0.21")
}

// Builds the Rust side with `cargo ndk` and copies the resulting libraries
// into src/main/jniLibs/<abi>/, where AGP packages them.
//
// This replaces the com.github.willir.rust.cargo-ndk-android plugin, which
// is unmaintained (last release 0.3.4, February 2021) and runs cargo through
// Project.exec, deprecated for removal in Gradle 9. The command line, copy
// locations and task names (buildCargoNdk<Variant>) match the plugin's.
abstract class CargoNdkBuildTask : DefaultTask() {
    @get:Inject
    abstract val execOps: ExecOperations

    // Directory holding the workspace Cargo.toml.
    @get:Input
    abstract val cargoDir: Property<String>
    @get:Input
    abstract val rustTarget: Property<String>
    @get:Input
    abstract val abi: Property<String>
    @get:Input
    abstract val platform: Property<Int>
    // Arguments after `build`, e.g. --release and feature flags.
    @get:Input
    abstract val cargoArgs: ListProperty<String>
    @get:Input
    abstract val cargoEnv: MapProperty<String, String>
    // Files copied from target/<triple>/release/.
    @get:Input
    abstract val libraries: ListProperty<String>
    @get:Input
    abstract val jniLibsDir: Property<String>

    @TaskAction
    fun build() {
        val cargoRoot = File(cargoDir.get())
        execOps.exec {
            workingDir = cargoRoot
            commandLine(
                listOf(
                    "cargo", "ndk",
                    "--target", rustTarget.get(),
                    "--platform", platform.get().toString(),
                    "--", "build",
                ) + cargoArgs.get()
            )
            environment(cargoEnv.get())
        }
        val from = cargoRoot.resolve("target/${rustTarget.get()}/release")
        val to = File(jniLibsDir.get(), abi.get())
        to.mkdirs()
        for (lib in libraries.get()) {
            from.resolve(lib).copyTo(to.resolve(lib), overwrite = true)
            logger.info("Copied ${from.resolve(lib)} -> ${to.resolve(lib)}")
        }
    }
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

// The plain workspace version from <repo>/Cargo.toml (e.g. "0.2.3"), used to
// name the APKs copied out of the build tree. Unlike getTouchHLEVersionName
// it carries no git suffix, so rebuilding a version replaces its APK.
fun getCargoVersion(): String {
    val toml = file("${rootDir.parentFile}/Cargo.toml").readText()
    val section = toml.substringAfter("[workspace.package]")
    return Regex("""(?m)^version\s*=\s*"([^"]+)"""")
        .find(section)?.groupValues?.get(1)
        ?: throw GradleException("No [workspace.package] version in Cargo.toml")
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
        // game-wrapper build. In wrapper mode the IPA is NOT bundled in
        // the APK; it lives in WRAPPER_USER_DATA_DIR on the device's
        // public storage, which is also where touchHLE's user data is
        // redirected to live.
        buildConfigField("boolean", "WRAPPER_AUTO_LAUNCH", "false")
        buildConfigField("String",  "WRAPPER_IPA_FILENAME", "\"\"")
        buildConfigField("String",  "WRAPPER_USER_DATA_DIR", "\"\"")

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
                // Make sure this matches the buildCargoNdk task below.
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
    //   - `songsummoner` -- a standalone wrapper that auto-launches the
    //                       Song Summoner IPA from /sdcard/SongSummoner/.
    //                       The IPA is NOT bundled in the APK; the user
    //                       drops it into that folder themselves. All
    //                       touchHLE user data (options, sandbox, music
    //                       library, log) also lives in that folder.
    // The Kotlin side branches on BuildConfig.WRAPPER_AUTO_LAUNCH.
    flavorDimensions += "distribution"
    productFlavors {
        create("touchhle") {
            dimension = "distribution"
            isDefault = true
        }
        create("songsummoner") {
            dimension = "distribution"
            // Own package id, outside touchHLE's org.touchhle.android
            // namespace, so it never clashes with a real touchHLE install.
            // Only the installed id changes: `namespace` (R, BuildConfig and
            // the Kotlin package) stays org.touchhle.android, and the
            // DocumentsProvider authority follows via ${applicationId}.
            // CI builds would still get defaultConfig's branding suffix.
            applicationId = "com.sqefam.songsummoner"
            versionNameSuffix = "-songsummoner"
            // App name shown under the launcher icon.
            resValue("string", "app_name", "Song Summoner")
            buildConfigField("String",  "APP_NAME", "\"Song Summoner\"")
            // Wrapper behaviour switches.
            buildConfigField("boolean", "WRAPPER_AUTO_LAUNCH", "true")
            // Filename the wrapper expects under WRAPPER_USER_DATA_DIR.
            // The .ipa extension matters because touchHLE's bundle
            // loader sniffs it.
            buildConfigField("String",  "WRAPPER_IPA_FILENAME",
                "\"Song Summoner The Unsung Heroes Encore.ipa\"")
            // Public-storage folder that holds both the IPA and all
            // touchHLE user data for this flavor. MainActivity sets the
            // TOUCHHLE_USER_DATA_BASE_PATH env var to this before SDL
            // starts the native thread, and paths.rs honours it.
            buildConfigField("String",  "WRAPPER_USER_DATA_DIR",
                "\"/sdcard/SongSummoner\"")
        }
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
        // Rust build. Every variant, debug included, builds Rust in release
        // mode, as the old plugin did: a debug build of the emulator core is
        // far too slow to play on.
        val buildCargoNdk = tasks.register<CargoNdkBuildTask>("buildCargoNdk${variantName}") {
            group = "build"
            description = "Builds the Rust library for variant $variantName"
            cargoDir.set(rootDir.parentFile.path)
            // Make sure these match the android abiFilters above.
            rustTarget.set("aarch64-linux-android")
            abi.set("arm64-v8a")
            platform.set(android.defaultConfig.minSdk ?: 21)
            // The default feature, "static", makes us use static linking for
            // SDL2 and OpenAL Soft. For Android, we need dynamic linking for
            // SDL2, but static linking for OpenAL Soft.
            cargoArgs.set(listOf(
                "--release",
                "--lib",
                "--no-default-features",
                "--features",
                "touchHLE_openal_soft_wrapper/static,sdl2/bundled",
            ))
            cargoEnv.set(cargoNdkEnv())
            libraries.set(listOf("libtouchHLE.so", "libSDL2.so", "libc++_shared.so"))
            jniLibsDir.set("$projectDir/src/main/jniLibs")
        }
        // Same hook points the plugin used.
        tasks.matching {
            it.name == "compile${variantName}Sources" ||
                it.name == "merge${variantName}JniLibFolders"
        }.configureEach { dependsOn(buildCargoNdk) }
        // Copy the finished APK out of the build tree after every assemble:
        // release builds to <repo-root>/dist/, debug builds to
        // <repo-root>/debug/, named <flavor>-<version>.apk
        // (e.g. song-summoner-0.2.3.apk).
        val outDir = if (buildType.name == "release") "dist" else "debug"
        val apkPrefix = if (flavorName == "songsummoner") "song-summoner" else "touchhle"
        val apkName = "$apkPrefix-${getCargoVersion()}.apk"
        val copyApk = tasks.register<Copy>("copy${variantName}Apk") {
            from(packageApplicationProvider.flatMap { it.outputDirectory }) {
                include("*.apk")
                rename { apkName }
            }
            into("${rootDir.parentFile}/$outDir")
        }
        assembleProvider.configure { finalizedBy(copyApk) }
    }

    sourceSets {
        getByName("main") {
            java.srcDir("${rootDir.parentFile}/vendor/SDL/android-project/app/src/main/java")
            // Ship the virtual-cursor sprite PNGs that the user drops at
            // <project-root>/res/cursor_*.png. The copy task below stages
            // them under build/generated/cursor_assets/res/ so AAPT packs
            // them at assets/res/cursor_*.png in the APK, where the Rust
            // side's ResourceFile loader will find them.
            assets.srcDir(
                layout.buildDirectory.dir("generated/cursor_assets")
            )
        }
    }

    // Copy the cursor sprite PNGs from the project root into the generated
    // assets directory the sourceSets entry above points at. Idempotent and
    // tolerant of missing files (the build still succeeds with no sprites;
    // the renderer falls back to the legacy black-dot cursor for whichever
    // states have no PNG).
    val copyCursorSprites = tasks.register<Copy>("copyCursorSprites") {
        from("${rootDir.parentFile}/res") {
            // Match both the README's underscore aliases and the
            // capitalised spaced names the user actually saves them as.
            include("cursor_*.png", "Cursor *.png")
            into("res")
        }
        into(layout.buildDirectory.dir("generated/cursor_assets"))
    }
    tasks.matching { it.name.startsWith("merge") && it.name.endsWith("Assets") }
        .configureEach { dependsOn(copyCursorSprites) }

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

// Environment for the `cargo ndk` build in CargoNdkBuildTask.
fun cargoNdkEnv(): Map<String, String> {
    val env = mutableMapOf(
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

        env.putAll(
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
    return env
}

// The only Java left in this module is SDL's vendored Android glue
// (vendor/SDL/android-project), which uses some deprecated Android APIs.
// We don't patch third-party code, so hide javac's "uses or overrides a
// deprecated API" note instead. Our own code is Kotlin and still warns.
tasks.withType<JavaCompile>().configureEach {
    options.compilerArgs.add("-XDsuppressNotes")
}

dependencies {
    implementation(fileTree("libs") {
        include("*.jar")
    })
}
