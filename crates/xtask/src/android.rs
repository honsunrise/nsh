//! Android runtime builds: Rust (`libnsh.so`) with the NDK, Java with
//! `javac` + `d8` from the SDK. No Gradle.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, bail};
use sha2::{Digest as _, Sha256};

use crate::term;

/// ABIs built by default: phones' 64-bit and 32-bit processes, and the
/// emulators of x86-64 hosts.
pub(crate) const ABIS: [&str; 3] = ["arm64-v8a", "armeabi-v7a", "x86_64"];
/// Minimum Android API level (Android 8.0). Android 8.x gets what API 28
/// added another way: weights from Roboto's named families, text measured
/// without `PrecomputedText`, shadows drawn in software
/// (docs/dev/pitfalls.md, "Android 8").
pub(crate) const MIN_SDK: u32 = 26;
/// The SDK platform compiled against.
pub(crate) const COMPILE_SDK: u32 = 35;
/// The Java runtime's shape (sources, dependencies and what `classes.jar`
/// holds): a cache from another shape is not reused.
const JAVA_FORMAT: &str = "thin-aar+androidx-webkit-1.15.0+okhttp-5.3.2";

/// Locations of the Android SDK and NDK.
pub(crate) struct Sdk {
    pub(crate) root: PathBuf,
    pub(crate) ndk: PathBuf,
    pub(crate) build_tools: PathBuf,
}

impl Sdk {
    pub(crate) fn find() -> anyhow::Result<Sdk> {
        let root = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
            .iter()
            .find_map(env::var_os)
            .map(PathBuf::from)
            // Android Studio's default place.
            .or_else(|| {
                if cfg!(windows) {
                    env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Android/Sdk"))
                } else {
                    env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Android/sdk"))
                }
            })
            .filter(|p| p.exists())
            .context("Android SDK not found: set ANDROID_HOME")?;
        let ndk = match env::var_os("ANDROID_NDK_HOME") {
            Some(ndk) => PathBuf::from(ndk),
            None => {
                newest(&root.join("ndk")).context("no NDK in the SDK: install `ndk;<version>`")?
            }
        };
        let build_tools = newest(&root.join("build-tools")).context("no SDK build-tools")?;
        Ok(Sdk {
            root,
            ndk,
            build_tools,
        })
    }

    pub(crate) fn android_jar(&self) -> PathBuf {
        self.root
            .join(format!("platforms/android-{COMPILE_SDK}/android.jar"))
    }

    fn toolchain(&self) -> PathBuf {
        // The NDK's "darwin-x86_64" toolchain is universal.
        let host = if cfg!(target_os = "macos") {
            "darwin-x86_64"
        } else if cfg!(windows) {
            "windows-x86_64"
        } else {
            "linux-x86_64"
        };
        self.ndk.join("toolchains/llvm/prebuilt").join(host)
    }
}

/// The highest version-named subdirectory of `dir`.
fn newest(dir: &Path) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    let key = |p: &PathBuf| -> Vec<u64> {
        p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .split(['.', '-'])
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    versions.sort_by_key(key);
    versions.pop()
}

fn triple(abi: &str) -> anyhow::Result<&'static str> {
    Ok(match abi {
        "arm64-v8a" => "aarch64-linux-android",
        "armeabi-v7a" => "armv7-linux-androideabi",
        "x86_64" => "x86_64-linux-android",
        other => bail!("unsupported ABI {other} (arm64-v8a, armeabi-v7a, x86_64)"),
    })
}

/// Repository root (this crate lives in `crates/xtask`).
pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Builds `libnsh.so` for `abis` and `classes.dex` into
/// `target/android/<profile>[-dev]`, returning that directory.
///
/// Release libraries go into `lib/<abi>` without their symbol tables and
/// debug info, which APKs store uncompressed (a quarter of each library);
/// `symbols/<abi>` keeps them whole, for `ndk-stack`, Play Console and the
/// AAR (Gradle strips what it packages and keeps the symbols itself).
pub(crate) fn build(release: bool, dev: bool, abis: &[String]) -> anyhow::Result<PathBuf> {
    let sdk = Sdk::find()?;
    let profile = if release { "release" } else { "debug" };
    let out = root().join("target/android").join(if dev {
        format!("{profile}-dev")
    } else {
        profile.to_owned()
    });
    for abi in abis {
        let triple = triple(abi)?;
        let so = cargo_lib(&sdk, triple, abi, release, dev)?;
        check_alignment(&sdk, &so)?;
        let dest = out.join("lib").join(abi);
        fs::create_dir_all(&dest)?;
        // Unchanged files keep their times: `nsh run` rebuilds and
        // reinstalls the development host when they change.
        let dest = dest.join("libnsh.so");
        let symbols = out.join("symbols").join(abi).join("libnsh.so");
        if up_to_date(&dest, std::slice::from_ref(&so)) && (!release || symbols.exists()) {
            continue;
        }
        if release {
            fs::create_dir_all(symbols.parent().context("symbols directory")?)?;
            fs::copy(&so, &symbols)?;
            strip(&sdk, &so, &dest)?;
            check_alignment(&sdk, &dest)?;
        } else {
            fs::copy(&so, &dest)?;
        }
    }
    let java_dir = root().join("crates/nsh-platform-android/java");
    let mut sources = Vec::new();
    collect(&java_dir, &mut sources)?;
    // The directories too: removing a source changes its directory.
    let mut inputs = sources.clone();
    collect_dirs(&java_dir, &mut inputs)?;
    let java_deps = dependencies(&root())?;
    inputs.extend(java_deps.iter().map(|dependency| dependency.path.clone()));
    let format = out.join("java.format");
    if fs::read_to_string(&format).ok().as_deref() != Some(JAVA_FORMAT) {
        // A runtime from another shape (a fat AAR's classes, other
        // dependencies) is rebuilt, not reused.
        let _ = fs::remove_file(out.join("classes.dex"));
    }
    if !up_to_date(&out.join("classes.dex"), &inputs) || !out.join("classes.jar").exists() {
        let step = term::Step::start("compiling the Java runtime");
        java(&sdk, &out, &sources, &java_deps)?;
        fs::write(&format, JAVA_FORMAT)?;
        step.done("compiled the Java runtime");
    }
    term::note(out.strip_prefix(root()).unwrap_or(&out).display());
    Ok(out)
}

/// Writes `library` to `to` without the symbols nothing links against: the
/// dynamic ones (`JNI_OnLoad`, the `Java_*` natives) stay.
fn strip(sdk: &Sdk, library: &Path, to: &Path) -> anyhow::Result<()> {
    term::run(
        Command::new(sdk.toolchain().join("bin/llvm-strip"))
            .arg("--strip-unneeded")
            .arg("-o")
            .arg(to)
            .arg(library),
    )
}

/// Check the linked ELF, before it is copied into a runtime or an AAR.
fn check_alignment(sdk: &Sdk, library: &Path) -> anyhow::Result<()> {
    let output = Command::new(sdk.toolchain().join("bin/llvm-objdump"))
        .arg("-p")
        .arg(library)
        .output()
        .with_context(|| format!("inspect ELF alignment of {}", library.display()))?;
    if !output.status.success() {
        bail!(
            "llvm-objdump failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    check_load_alignment(&String::from_utf8(output.stdout)?)
        .with_context(|| format!("{} is not 16 KB aligned", library.display()))
}

fn check_load_alignment(headers: &str) -> anyhow::Result<()> {
    let mut loads = 0;
    for line in headers
        .lines()
        .filter(|line| line.split_whitespace().next() == Some("LOAD"))
    {
        let (_, exponent) = line
            .rsplit_once("align 2**")
            .context("missing LOAD alignment")?;
        let exponent: u32 = exponent.trim().parse().context("invalid LOAD alignment")?;
        if exponent < 14 {
            bail!("LOAD alignment is 2**{exponent}; expected at least 2**14");
        }
        loads += 1;
    }
    if loads == 0 {
        bail!("no ELF LOAD segments found");
    }
    Ok(())
}

/// The NDK's name of `triple` in its compilers' names: ARMv7's is
/// `armv7a-linux-androideabi`, the others are Rust's.
fn clang_target(triple: &str) -> &str {
    match triple {
        "armv7-linux-androideabi" => "armv7a-linux-androideabi",
        other => other,
    }
}

/// A cargo command set up to cross-compile for `triple` with the NDK.
fn ndk_cargo(sdk: &Sdk, triple: &str) -> Command {
    let toolchain = sdk.toolchain();
    let bin = toolchain.join("bin");
    // On Windows the NDK's per-target compilers are batch files.
    let script = if cfg!(windows) { ".cmd" } else { "" };
    let clang = bin.join(format!("{}{MIN_SDK}-clang{script}", clang_target(triple)));
    let env_triple = triple.replace('-', "_");
    // New NDKs keep the C standard headers their clang carries (stdbool.h
    // &c.) out of the sysroot: whatever libclang bindgen of the C
    // dependencies loads, give it that directory to search as well. Last
    // (`-idirafter`), not first: ahead of the libclang's own headers, the
    // NDK's `stdint.h` reaches the libclang's through `#include_next`, which
    // its include guard empties, and the sysroot's `stdint.h` is never read
    // (NDK 27 with a system libclang: `unknown type name 'uint32_t'`).
    let clang_include = fs::read_dir(toolchain.join("lib/clang"))
        .ok()
        .and_then(|versions| {
            versions
                .filter_map(|version| version.ok().map(|version| version.path()))
                .map(|version| version.join("include"))
                .find(|include| include.is_dir())
        });
    let mut extra = format!(
        "--target={}{MIN_SDK} --sysroot=\"{}\"",
        clang_target(triple),
        toolchain
            .join("sysroot")
            .display()
            .to_string()
            .replace('\\', "/")
    );
    if let Some(include) = clang_include {
        let include = include.display().to_string().replace('\\', "/");
        extra.push_str(format!(" -idirafter \"{include}\"").as_str());
    }
    let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(root())
        .env(format!("CC_{env_triple}"), &clang)
        .env(format!("AR_{env_triple}"), bin.join("llvm-ar"))
        .env(
            format!("CARGO_TARGET_{}_LINKER", env_triple.to_uppercase()),
            &clang,
        );
    // The C dependencies' bindgen runs against the NDK's own libclang,
    // rather than against whichever libclang the machine answers with
    // first — but newer NDKs ship no libclang at all (r27 dropped it),
    // and setting LIBCLANG_PATH cuts clang-sys's own discovery off. Only
    // point it here when this NDK actually carries the library.
    let ndk_libclang = bin.join(if cfg!(windows) {
        "libclang.dll"
    } else {
        "libclang.so"
    });
    if ndk_libclang.exists() {
        cmd.env("LIBCLANG_PATH", &bin);
    }
    // bindgen splits this variable like a shell: quoted, with forward slashes,
    // so that the backslashes and spaces of a Windows path survive.
    cmd.env(format!("BINDGEN_EXTRA_CLANG_ARGS_{env_triple}"), extra);
    cmd
}

/// Clippy on the Android crate (it is empty on other targets): with every
/// feature, and as a Rust app's library (no JS).
pub(crate) fn clippy() -> anyhow::Result<()> {
    let sdk = Sdk::find()?;
    for features in ["--all-features", "--no-default-features"] {
        let status = ndk_cargo(&sdk, "aarch64-linux-android")
            .args([
                "clippy",
                "-p",
                "nsh-platform-android",
                "--target",
                "aarch64-linux-android",
                features,
                "--",
                "-D",
                "warnings",
            ])
            .status()?;
        if !status.success() {
            bail!("clippy found problems ({features})");
        }
    }
    Ok(())
}

fn cargo_lib(
    sdk: &Sdk,
    triple: &str,
    abi: &str,
    release: bool,
    dev: bool,
) -> anyhow::Result<PathBuf> {
    let mut cmd = ndk_cargo(sdk, triple);
    cmd.args(["build", "-p", "nsh-platform-android", "--target", triple]);
    if release {
        cmd.arg("--release");
    }
    if dev {
        cmd.args(["--features", "dev"]);
    }
    term::cargo(&mut cmd, &format!("libnsh.so for {abi}"))?;
    let profile = if release { "release" } else { "debug" };
    Ok(root()
        .join("target")
        .join(triple)
        .join(profile)
        .join("libnsh.so"))
}

/// Whether `file` exists and is newer than each of `sources`.
fn up_to_date(file: &Path, sources: &[PathBuf]) -> bool {
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    let Some(built) = modified(file) else {
        return false;
    };
    sources
        .iter()
        .all(|s| modified(s).is_some_and(|m| m <= built))
}

/// A Java dependency of the standalone runtime, fetched from its published
/// repository and checked against a pinned SHA-256.
///
/// The runtime's `classes.dex` is NSH's own classes plus these real
/// artifacts: `androidx.webkit` supplies `COOKIE_INTERCEPT` (1.15.0) for
/// the request's own cookies in `shouldInterceptRequest`, and OkHttp's
/// Android artifact (5.3.2) is the download transport. The library AAR
/// stays thin (its Gradle dependencies are declared by the SDK's POM and
/// example), so an embedding app resolves the same artifacts itself.
struct Dependency {
    /// The file the artifact is cached as.
    file: &'static str,
    /// The published artifact.
    url: &'static str,
    /// The artifact's SHA-256, refusing anything else.
    sha256: &'static str,
}

/// `androidx.webkit:webkit:1.15.0`,
/// `com.squareup.okhttp3:okhttp-android:5.3.2` and their resolved runtime
/// closures.
///
/// `annotation` is the Kotlin multiplatform module, whose JVM classes are in
/// `annotation-jvm`; its POM pulls `kotlin-stdlib` and
/// `org.jetbrains:annotations`. `kotlin-stdlib-common` is metadata only and
/// carries no classes. `androidx.core` is used by
/// `WebViewFeatureInternal`'s WebView-version check, but the whole artifact
/// is used (not hand-picked classes): its nested implementations and
/// interfaces are easy to miss.
///
/// OkHttp 5 publishes Kotlin Multiplatform metadata under `okhttp`; on
/// Android the classes are in the `okhttp-android` AAR (the JVM artifact
/// would select the desktop platform). Its POM requires `okio-jvm`,
/// `kotlin-stdlib` 2.2.21, `androidx.annotation` 1.9.1 and AndroidX
/// Startup; the `PlatformInitializer` class needs Startup's `Initializer`
/// interface to dex, and Startup's own initializer references
/// `androidx.tracing`. These are present even though the standalone dex
/// does not merge the AAR's startup provider: the classes must resolve, and
/// an embedding Gradle app merges them itself.
const DEPENDENCIES: [Dependency; 16] = [
    Dependency {
        file: "webkit-1.15.0.aar",
        url: "https://dl.google.com/dl/android/maven2/androidx/webkit/webkit/1.15.0/webkit-1.15.0.aar",
        sha256: "241c90aac937b6562592976da5cc73100538bf255000f490ed37296da1773960",
    },
    Dependency {
        file: "annotation-jvm-1.9.1.jar",
        url: "https://dl.google.com/dl/android/maven2/androidx/annotation/annotation-jvm/1.9.1/annotation-jvm-1.9.1.jar",
        sha256: "1e343917ebf27ba96fe4dc52b1cad7fd32b738fbc6355bb6cd5b3b305d7212d0",
    },
    Dependency {
        file: "annotation-experimental-1.3.0.aar",
        url: "https://dl.google.com/dl/android/maven2/androidx/annotation/annotation-experimental/1.3.0/annotation-experimental-1.3.0.aar",
        sha256: "abfd29c8556e5bd0325a9f769ab9e9d154ff4a5515c476cdd5a2a8285b1b19dc",
    },
    Dependency {
        file: "jspecify-1.0.0.jar",
        url: "https://repo1.maven.org/maven2/org/jspecify/jspecify/1.0.0/jspecify-1.0.0.jar",
        sha256: "1fad6e6be7557781e4d33729d49ae1cdc8fdda6fe477bb0cc68ce351eafdfbab",
    },
    Dependency {
        file: "core-1.1.0.aar",
        url: "https://dl.google.com/dl/android/maven2/androidx/core/core/1.1.0/core-1.1.0.aar",
        sha256: "76c7cfbe596fe3c09a6983bf1c89e889299c08ac9a3b52ce5182a088d056647e",
    },
    // `androidx.core`'s own compile/runtime dependencies (the closure
    // Gradle would resolve), so the standalone dex has no missing
    // supertypes or interfaces.
    Dependency {
        file: "lifecycle-runtime-2.0.0.aar",
        url: "https://dl.google.com/dl/android/maven2/androidx/lifecycle/lifecycle-runtime/2.0.0/lifecycle-runtime-2.0.0.aar",
        sha256: "e4afc9e636183f6f3e0edf1cf46121a492ffd2c673075bb07f55c7a99dd43cfb",
    },
    Dependency {
        file: "lifecycle-common-2.0.0.jar",
        url: "https://dl.google.com/dl/android/maven2/androidx/lifecycle/lifecycle-common/2.0.0/lifecycle-common-2.0.0.jar",
        sha256: "7bad7a188804adea6fa1f35d5ef99b705f20bd93ecadde484760ff86b535fefc",
    },
    Dependency {
        file: "core-common-2.0.0.jar",
        url: "https://dl.google.com/dl/android/maven2/androidx/arch/core/core-common/2.0.0/core-common-2.0.0.jar",
        sha256: "4b80b337779b526e64b0ee0ca9e0df43b808344d145f8e9b1c42a134dac57ad8",
    },
    Dependency {
        file: "versionedparcelable-1.1.0.aar",
        url: "https://dl.google.com/dl/android/maven2/androidx/versionedparcelable/versionedparcelable/1.1.0/versionedparcelable-1.1.0.aar",
        sha256: "9a1d77140ac222b7866b5054ee7d159bc1800987ed2d46dd6afdd145abb710c1",
    },
    Dependency {
        file: "collection-1.0.0.jar",
        url: "https://dl.google.com/dl/android/maven2/androidx/collection/collection/1.0.0/collection-1.0.0.jar",
        sha256: "9c8d117b5c2bc120a1cdfeb857e05b495b16c36013570372a708f7827e3ac9f9",
    },
    // The annotation artifacts and OkHttp are Kotlin: their POMs require
    // the Kotlin standard library (and its `org.jetbrains:annotations`).
    // Dexing them without it would leave references unresolved. 2.2.21 is
    // what okhttp-android 5.3.2 and okio-jvm 3.16.4 require; AndroidX
    // annotation 1.9.1 resolves against it too. 5.4.0 and later Android
    // artifacts declare minCompileSdk 36/37; this build compiles against 35.
    Dependency {
        file: "okhttp-android-5.3.2.aar",
        url: "https://repo1.maven.org/maven2/com/squareup/okhttp3/okhttp-android/5.3.2/okhttp-android-5.3.2.aar",
        sha256: "f1a78812ebeef4d45eee0dbeb4f27347210f564d2b8ee411f012ce8573b0a658",
    },
    Dependency {
        file: "okio-jvm-3.16.4.jar",
        url: "https://repo1.maven.org/maven2/com/squareup/okio/okio-jvm/3.16.4/okio-jvm-3.16.4.jar",
        sha256: "2196b993cd34dbbd919e7e01f57a4781b58bee80f86106163e287c20343a96a7",
    },
    Dependency {
        file: "kotlin-stdlib-2.2.21.jar",
        url: "https://repo1.maven.org/maven2/org/jetbrains/kotlin/kotlin-stdlib/2.2.21/kotlin-stdlib-2.2.21.jar",
        sha256: "6558a3d233da56a20934b32159f9db5f86ed5816ef098f78a2c223dc6abb79dd",
    },
    // `PlatformInitializer` implements Startup's `Initializer`, and
    // Startup's `AppInitializer` references `androidx.tracing.Trace`:
    // included so the dex has no missing interfaces or classes.
    Dependency {
        file: "startup-runtime-1.2.0.aar",
        url: "https://dl.google.com/dl/android/maven2/androidx/startup/startup-runtime/1.2.0/startup-runtime-1.2.0.aar",
        sha256: "f51d361d17b30b09cfb9e2aebf301d113f99437cbf462a066092795fb60eddd2",
    },
    Dependency {
        file: "tracing-1.0.0.aar",
        url: "https://dl.google.com/dl/android/maven2/androidx/tracing/tracing/1.0.0/tracing-1.0.0.aar",
        sha256: "07b8b6139665b884a162eccf97891ca50f7f56831233bf25168ae04f7b568612",
    },
    Dependency {
        file: "annotations-13.0.jar",
        url: "https://repo1.maven.org/maven2/org/jetbrains/annotations/13.0/annotations-13.0.jar",
        sha256: "ace2a10dc8e2d5fd34925ecac03e4988b2c0f851650c94b8cef49ba1bd111478",
    },
];

/// A dependency's classes, ready for `javac` and for `d8`.
struct DependencyJar {
    path: PathBuf,
}

/// Downloads, verifies and unpacks [`DEPENDENCIES`] under `target/android/deps`.
fn dependencies(root: &Path) -> anyhow::Result<Vec<DependencyJar>> {
    let dir = root.join("target/android/deps");
    fs::create_dir_all(&dir)?;
    let mut jars = Vec::new();
    for dependency in &DEPENDENCIES {
        let artifact = fetch(dependency, &dir)?;
        let path = if dependency.file.ends_with(".aar") {
            let classes = dir.join(format!("{}.classes.jar", dependency.file));
            if !up_to_date(&classes, std::slice::from_ref(&artifact)) {
                extract_classes(&artifact, &classes)?;
            }
            classes
        } else {
            artifact
        };
        jars.push(DependencyJar { path });
    }
    Ok(jars)
}

/// `dependency`'s artifact in `dir`, downloaded when missing (or when the
/// cached copy is not the pinned one).
fn fetch(dependency: &Dependency, dir: &Path) -> anyhow::Result<PathBuf> {
    let path = dir.join(dependency.file);
    if path.exists() && sha256(&path)? == dependency.sha256 {
        return Ok(path);
    }
    let part = dir.join(format!("{}.part", dependency.file));
    let output = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "--connect-timeout", "30", "-o"])
        .arg(&part)
        .arg(dependency.url)
        .output()
        .context("run curl")?;
    if !output.status.success() {
        let _ = fs::remove_file(&part);
        bail!(
            "download {}: {}",
            dependency.url,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let hash = sha256(&part)?;
    if hash != dependency.sha256 {
        let _ = fs::remove_file(&part);
        bail!(
            "{} has SHA-256 {hash}, not {}: refusing to build with it",
            dependency.url,
            dependency.sha256
        );
    }
    fs::rename(&part, &path)?;
    Ok(path)
}

/// The SHA-256 of `path`, in lower-case hexadecimal.
fn sha256(path: &Path) -> anyhow::Result<String> {
    let mut file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = io::Read::read(&mut file, &mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut hex = String::with_capacity(64);
    for byte in hasher.finalize() {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    Ok(hex)
}

/// `aar`'s `classes.jar`, beside it.
fn extract_classes(aar: &Path, to: &Path) -> anyhow::Result<()> {
    let file = fs::File::open(aar).with_context(|| format!("open {}", aar.display()))?;
    let mut archive =
        zip::ZipArchive::new(file).with_context(|| format!("read {}", aar.display()))?;
    let mut entry = archive
        .by_name("classes.jar")
        .with_context(|| format!("{} has no classes.jar", aar.display()))?;
    let mut out = fs::File::create(to).with_context(|| format!("create {}", to.display()))?;
    io::copy(&mut entry, &mut out)?;
    Ok(())
}

/// Compiles the Java runtime (`sources`) to `out/classes.dex`.
fn java(
    sdk: &Sdk,
    out: &Path,
    sources: &[PathBuf],
    dependencies: &[DependencyJar],
) -> anyhow::Result<()> {
    let classes = out.join("classes");
    if classes.exists() {
        fs::remove_dir_all(&classes)?;
    }
    fs::create_dir_all(&classes)?;
    let mut classpath = vec![sdk.android_jar()];
    classpath.extend(
        dependencies
            .iter()
            .map(|dependency| dependency.path.clone()),
    );
    let classpath = env::join_paths(&classpath).context("join the Java classpath")?;
    // Java 17 bytecode: d8 desugars it for every supported API level.
    term::run(
        jdk("javac")
            .args([
                "--release",
                "17",
                "-Xlint:-options",
                "-encoding",
                "UTF-8",
                "-classpath",
            ])
            .arg(classpath)
            .arg("-d")
            .arg(&classes)
            .args(sources),
    )?;
    // One jar for d8 rather than every class file: on Windows `d8.bat` runs
    // through cmd.exe, whose command line ends at 8191 characters. It is
    // also the AAR's `classes.jar` (Gradle projects dex it themselves).
    let jar = out.join("classes.jar");
    let _ = fs::remove_file(&jar);
    term::run(
        jdk("jar")
            .arg("cf")
            .arg(&jar)
            .arg("-C")
            .arg(&classes)
            .arg("."),
    )?;
    let mut d8 = Command::new(
        sdk.build_tools
            .join(if cfg!(windows) { "d8.bat" } else { "d8" }),
    );
    // The library AAR stays thin (NSH's classes only): an embedding app
    // gets the dependencies from Gradle, declared by the SDK's POM. The
    // standalone runtime has no Gradle, so its dex is NSH's classes and the
    // real artifacts together. An AAR's `classes.jar` refers to its
    // generated `R` class, which Gradle makes and d8 here reports as a
    // missing class for code paths this runtime never loads.
    d8.args(["--min-api", &MIN_SDK.to_string(), "--lib"])
        .arg(sdk.android_jar())
        .arg("--output")
        .arg(out)
        .arg(&jar);
    for dependency in dependencies {
        d8.arg(&dependency.path);
    }
    // `d8.bat` runs the `java` it finds on the PATH: the JDK's bin first.
    if let Some(home) = env::var_os("JAVA_HOME").map(PathBuf::from) {
        let mut paths = vec![home.join("bin")];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap_or_default()));
        if let Ok(path) = env::join_paths(paths) {
            d8.env("PATH", path);
        }
    }
    term::run(&mut d8)
}

/// A JDK tool (`javac`, `jar`): JAVA_HOME's rather than the PATH's, where a
/// machine that builds Android apps usually keeps a JDK.
fn jdk(command: &str) -> Command {
    let exe = if cfg!(windows) {
        format!("{command}.exe")
    } else {
        command.to_owned()
    };
    let from_home = env::var_os("JAVA_HOME")
        .map(PathBuf::from)
        .map(|home| home.join("bin").join(&exe))
        .filter(|tool| tool.exists());
    Command::new(from_home.unwrap_or_else(|| PathBuf::from(&exe)))
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    collect_ext(dir, "java", out)
}

/// `dir` and the directories in it.
fn collect_dirs(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    out.push(dir.to_owned());
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_dirs(&path, out)?;
        }
    }
    Ok(())
}

fn collect_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            collect_ext(&path, ext, out)?;
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::check_load_alignment;

    #[test]
    fn load_segments_support_16_kb_pages() {
        let headers = "libnsh.so: file format elf64-littleaarch64\n\
            LOAD off 0x0000 vaddr 0x0000 paddr 0x0000 align 2**14\n\
            LOAD off 0x4000 vaddr 0x4000 paddr 0x4000 align 2**16\n\
            DYNAMIC off 0x4010 vaddr 0x4010 paddr 0x4010 align 2**3\n";
        check_load_alignment(headers).unwrap();
        for exponent in [12, 13] {
            let headers = headers.replace("align 2**16", &format!("align 2**{exponent}"));
            let error = check_load_alignment(&headers).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(&format!("LOAD alignment is 2**{exponent}"))
            );
        }
    }

    #[test]
    fn missing_or_invalid_load_headers_fail_the_check() {
        for headers in ["", "LOAD off 0x0000", "LOAD off 0x0000 align 2**invalid"] {
            assert!(check_load_alignment(headers).is_err(), "{headers}");
        }
    }
}
