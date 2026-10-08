//! WSL detection, and facts about the Windows side, found by running Windows
//! programs through WSL interop.

use std::{path::Path, process::Command};

use crate::info::{Ctx, System};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wsl {
    V1,
    V2,
}

/// Where WSL maps in the Windows NVIDIA driver's `nvidia-smi`.
const NVIDIA_SMI: &str = "/usr/lib/wsl/lib/nvidia-smi";
/// The first Windows 11 build; Windows 11 still reports version 10.0.
const WINDOWS_11_BUILD: u32 = 22000;
/// Set by WSL to the distribution's name, e.g. "Ubuntu".
const DISTRO: &str = "WSL_DISTRO_NAME";

/// The WSL version of `sys`, or `None` outside WSL.
pub fn detect(sys: &dyn System) -> Option<Wsl> {
    let release = sys
        .read(Path::new("/proc/sys/kernel/osrelease"))
        .unwrap_or_default();
    classify(&release, sys.env(DISTRO).is_some_and(|d| !d.is_empty()))
}

/// WSL kernels say so in their release ("6.6.87.2-microsoft-standard-WSL2"),
/// and WSL sets `WSL_DISTRO_NAME` even with a custom kernel.
fn classify(release: &str, distro_set: bool) -> Option<Wsl> {
    let release = release.to_lowercase();
    (release.contains("microsoft") || distro_set).then(|| match release.contains("wsl2") {
        true => Wsl::V2,
        false => Wsl::V1,
    })
}

/// "Windows Subsystem for Linux 2.6.3.0 (Ubuntu)", or `None` without a
/// working `wslinfo`. Fast enough (~1 ms) not to need caching.
pub fn host(ctx: &Ctx) -> Option<String> {
    let mut wslinfo = Command::new("wslinfo");
    wslinfo.arg("--version");
    let out = ctx.run(wslinfo).filter(|o| o.status.success())?;
    let out = String::from_utf8_lossy(&out.stdout);
    let distro = ctx.env_nonempty(DISTRO);
    Some(host_name(wslinfo_version(&out)?, distro.as_deref()))
}

fn host_name(version: &str, distro: Option<&str>) -> String {
    match distro {
        Some(distro) => format!("Windows Subsystem for Linux {version} ({distro})"),
        None => format!("Windows Subsystem for Linux {version}"),
    }
}

/// The version in `wslinfo --version` output ("2.6.3.0").
fn wslinfo_version(out: &str) -> Option<&str> {
    let is_version = |word: &&str| {
        let mut parts = word.split('.');
        let numeric = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        word.contains('.') && parts.all(numeric)
    };
    out.lines().next()?.split_whitespace().find(is_version)
}

/// "Windows 11 (build 26300)", from `cmd.exe /c ver` (~130 ms).
pub fn windows(ctx: &Ctx) -> Option<String> {
    let mut cmd = Command::new("cmd.exe");
    // Started from a Linux directory, cmd.exe warns that UNC paths are not supported.
    cmd.args(["/c", "ver"]).current_dir("/mnt/c");
    let out = ctx.run(cmd).filter(|o| o.status.success())?;
    windows_version(&String::from_utf8_lossy(&out.stdout))
}

/// Parses "Microsoft Windows [Version 10.0.26300.9457]".
fn windows_version(ver: &str) -> Option<String> {
    // "Version" is localized, so only the number in the brackets is used.
    let (_, rest) = ver.split_once('[')?;
    let (inside, _) = rest.split_once(']')?;
    let number = inside.split_whitespace().last()?;
    let parts: Vec<u32> = number
        .split('.')
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let &[_major, _minor, build, ..] = parts.as_slice() else {
        return None;
    };
    let name = match build >= WINDOWS_11_BUILD {
        true => "Windows 11",
        false => "Windows 10",
    };
    Some(format!("{name} (build {build})"))
}

/// The physical GPUs, one per line. WSL's own PCI bus only has a virtual
/// adapter, so ask the NVIDIA driver (~120 ms), or else Windows (~700 ms).
pub fn gpus(ctx: &Ctx) -> Option<String> {
    let mut nvidia_smi = Command::new(NVIDIA_SMI);
    nvidia_smi.args(["--query-gpu=name", "--format=csv,noheader"]);
    let mut powershell = Command::new("powershell.exe");
    powershell.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "(Get-CimInstance Win32_VideoController).Name",
    ]);
    [nvidia_smi, powershell].into_iter().find_map(|cmd| {
        let out = ctx.run(cmd).filter(|o| o.status.success())?;
        gpu_names(&String::from_utf8_lossy(&out.stdout))
    })
}

/// One name per output line; Windows programs end them with CRLF.
fn gpu_names(out: &str) -> Option<String> {
    let mut names: Vec<&str> = Vec::new();
    for name in out.lines().map(str::trim).filter(|n| !n.is_empty()) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    (!names.is_empty()).then(|| names.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wsl_from_the_kernel_release() {
        let cases = [
            ("6.6.87.2-microsoft-standard-WSL2", false, Some(Wsl::V2)),
            ("6.6.87.2-microsoft-standard-WSL2\n", false, Some(Wsl::V2)),
            ("5.15.90.1-Microsoft-standard-WSL2", false, Some(Wsl::V2)),
            ("4.4.0-19041-Microsoft", false, Some(Wsl::V1)),
            ("4.4.0-22621-microsoft", false, Some(Wsl::V1)),
            ("6.1.21-custom-WSL2", true, Some(Wsl::V2)),
            ("6.1.21-custom", true, Some(Wsl::V1)),
            ("6.8.0-45-generic", false, None),
            ("6.10.10-arch1-1", false, None),
            ("", false, None),
        ];
        for (release, distro_set, expected) in cases {
            assert_eq!(
                classify(release, distro_set),
                expected,
                "{release:?}, {distro_set}"
            );
        }
    }

    #[test]
    fn host_name_format() {
        assert_eq!(
            host_name("2.6.3.0", Some("Ubuntu")),
            "Windows Subsystem for Linux 2.6.3.0 (Ubuntu)"
        );
        assert_eq!(
            host_name("2.6.3.0", None),
            "Windows Subsystem for Linux 2.6.3.0"
        );
    }

    #[test]
    fn parses_wslinfo_version() {
        assert_eq!(wslinfo_version("2.6.3.0\n"), Some("2.6.3.0"));
        assert_eq!(wslinfo_version("2.6.3.0\r\n"), Some("2.6.3.0"));
        assert_eq!(wslinfo_version("WSL version: 2.6.3.0\n"), Some("2.6.3.0"));
        for junk in [
            "",
            "\n",
            "Invalid command line argument: --version\n",
            "Usage: wslinfo [--networking-mode] [-n]\n",
            "2.\n",
            "2..3\n",
            "v2.6.3\n",
            "\n2.6.3.0\n",
        ] {
            assert_eq!(wslinfo_version(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn parses_windows_ver() {
        let ver = |s: &str| windows_version(s);
        assert_eq!(
            ver("\r\nMicrosoft Windows [Version 10.0.26300.9457]\r\n").as_deref(),
            Some("Windows 11 (build 26300)")
        );
        assert_eq!(
            ver("Microsoft Windows [Version 10.0.22000.194]").as_deref(),
            Some("Windows 11 (build 22000)")
        );
        assert_eq!(
            ver("\r\nMicrosoft Windows [Version 10.0.19045.5247]\r\n").as_deref(),
            Some("Windows 10 (build 19045)")
        );
        // The word "Version" is localized.
        assert_eq!(
            ver("Microsoft Windows [Versi\u{fffd}n 10.0.19045.3803]").as_deref(),
            Some("Windows 10 (build 19045)")
        );
        for junk in [
            "",
            "\r\n",
            "'cmd.exe' is not recognized",
            "Microsoft Windows [Version]",
            "Microsoft Windows [Version 10.0]",
            "Microsoft Windows [Version 10.0.abc.1]",
            "Microsoft Windows [Version x.0.19045.1]",
            "Microsoft Windows [Version 10.0.19045.5247",
        ] {
            assert_eq!(ver(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn parses_gpu_lists() {
        assert_eq!(
            gpu_names("NVIDIA T1200 Laptop GPU\n").as_deref(),
            Some("NVIDIA T1200 Laptop GPU")
        );
        assert_eq!(
            gpu_names("NVIDIA GeForce RTX 4090\nNVIDIA RTX A2000\n").as_deref(),
            Some("NVIDIA GeForce RTX 4090\nNVIDIA RTX A2000")
        );
        // PowerShell: CRLF, and the same model listed twice.
        assert_eq!(
            gpu_names(
                "NVIDIA T1200 Laptop GPU\r\nIntel(R) UHD Graphics\r\nIntel(R) UHD Graphics\r\n\r\n"
            )
            .as_deref(),
            Some("NVIDIA T1200 Laptop GPU\nIntel(R) UHD Graphics")
        );
        for empty in ["", "\n", " \r\n\r\n"] {
            assert_eq!(gpu_names(empty), None, "{empty:?}");
        }
    }
}
