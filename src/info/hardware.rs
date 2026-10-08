//! Modules about the hardware: host model, CPU, GPU, memory, disk and battery.

use super::{
    Ctx,
    value::{Batteries, Battery, Cpu, Gpus, Name, Usage, Value},
};
use crate::wsl::{self, Wsl};

const DMI_PLACEHOLDERS: &[&str] = &[
    "To be filled by O.E.M.",
    "System Product Name",
    "System Version",
    "Default string",
    "Not Applicable",
    "None",
    "OEM",
    "O.E.M.",
    "Type1ProductConfigId",
    "Undefined",
];

pub fn host(ctx: &Ctx) -> Option<Value> {
    host_name(ctx).map(|name| Value::Host(Name::new(name)))
}

fn host_name(ctx: &Ctx) -> Option<String> {
    if ctx.wsl.is_some()
        && let Some(host) = wsl::host(ctx)
    {
        return Some(host);
    }
    let dmi = |f: &str| {
        ctx.read(format!("/sys/devices/virtual/dmi/id/{f}"))
            .filter(|v| !DMI_PLACEHOLDERS.iter().any(|p| v.eq_ignore_ascii_case(p)))
    };
    if let Some(name) = dmi("product_name") {
        return Some(match dmi("product_version") {
            Some(version) if !name.contains(&version) => format!("{name} {version}"),
            _ => name,
        });
    }
    if let Some(model) = ctx.read("/sys/firmware/devicetree/base/model") {
        return Some(model.trim_end_matches('\0').to_string());
    }
    // WSL without a working `wslinfo`.
    ctx.wsl.map(|version| match version {
        Wsl::V2 => "Windows Subsystem for Linux (WSL2)".into(),
        Wsl::V1 => "Windows Subsystem for Linux".into(),
    })
}

pub fn cpu(ctx: &Ctx) -> Option<Value> {
    let info = ctx.cpuinfo()?;
    let field = |key: &str| {
        info.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            (k.trim() == key).then(|| v.trim().to_string())
        })
    };
    let threads = info.lines().filter(|l| l.starts_with("processor")).count();
    let raw = field("model name")
        .or_else(|| field("Hardware"))
        .or_else(|| field("cpu model"))?;

    // Strip the marketing noise, e.g. "Intel(R) Core(TM) i7 CPU @ 2.50GHz", "8-Core Processor".
    let (name, nominal) = match raw.split_once(" @ ") {
        Some((name, freq)) => (
            name,
            freq.trim().strip_suffix("GHz").and_then(|f| f.parse().ok()),
        ),
        None => (raw.as_str(), None),
    };
    let mut name = ["(R)", "(r)", "(TM)", "(tm)", " CPU", " Processor"]
        .iter()
        .fold(name.to_string(), |n, junk| n.replace(junk, ""));
    if let Some(i) = name.find(" with Radeon") {
        name.truncate(i);
    }
    let name = name
        .split_whitespace()
        .filter(|w| !w.ends_with("-Core"))
        .collect::<Vec<_>>()
        .join(" ");

    let max_khz: Option<f64> = ctx
        .read("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq")
        .and_then(|s| s.parse().ok());
    let ghz = max_khz
        .map(|khz| khz / 1e6)
        .or(nominal)
        .or_else(|| field("cpu MHz")?.parse::<f64>().ok().map(|mhz| mhz / 1e3));
    Some(Value::Cpu(Cpu { name, threads, ghz }))
}

pub fn gpu(ctx: &Ctx) -> Option<Value> {
    gpu_names(ctx).map(|names| Value::Gpu(Gpus { names }))
}

fn gpu_names(ctx: &Ctx) -> Option<Vec<String>> {
    // On WSL the PCI scan only finds the virtual "Microsoft Basic Render Driver".
    if ctx.wsl.is_some()
        && let Some(gpus) = ctx.cached("gpu", || wsl::gpus(ctx))
    {
        return Some(gpus.lines().map(str::to_string).collect());
    }
    let mut pci_ids: Option<Option<String>> = None;
    let mut gpus: Vec<String> = Vec::new();
    for dev in ctx.sorted_dir("/sys/bus/pci/devices") {
        // PCI class 0x03xxxx = display controller.
        if !ctx
            .read(dev.join("class"))
            .is_some_and(|c| c.starts_with("0x03"))
        {
            continue;
        }
        let id = |f: &str| {
            ctx.read(dev.join(f))
                .map(|s| s.trim_start_matches("0x").to_lowercase())
        };
        let (Some(vendor), Some(device)) = (id("vendor"), id("device")) else {
            continue;
        };
        let ids = pci_ids.get_or_insert_with(|| {
            [
                "/usr/share/hwdata/pci.ids",
                "/usr/share/misc/pci.ids",
                "/usr/share/pci.ids",
            ]
            .iter()
            .find_map(|p| ctx.read_full(p))
        });
        let name = gpu_name(ids.as_deref(), &vendor, &device);
        if !gpus.contains(&name) {
            gpus.push(name);
        }
    }
    (!gpus.is_empty()).then_some(gpus)
}

fn gpu_name(ids: Option<&str>, vendor: &str, device: &str) -> String {
    let (vendor_name, device_name) = ids
        .map(|ids| pci_lookup(ids, vendor, device))
        .unwrap_or_default();
    let vendor = match vendor {
        "10de" => "NVIDIA",
        "1002" => "AMD",
        "8086" => "Intel",
        "1414" => "Microsoft",
        "15ad" => "VMware",
        "80ee" => "VirtualBox",
        "1af4" => "Red Hat",
        "1234" => "QEMU",
        _ => vendor_name.as_deref().unwrap_or(vendor),
    };
    // "GA102 [GeForce RTX 3080]" -> "GeForce RTX 3080"
    let model = match device_name {
        Some(d) => match (d.rfind('['), d.rfind(']')) {
            (Some(a), Some(b)) if a < b => d[a + 1..b].to_string(),
            _ => d,
        },
        None => format!("Device {device}"),
    };
    format!("{vendor} {model}")
}

/// Looks up vendor and device names in a pci.ids database.
fn pci_lookup(ids: &str, vendor: &str, device: &str) -> (Option<String>, Option<String>) {
    let vendor_name = |l: &str| {
        l.strip_prefix(vendor)?
            .strip_prefix("  ")
            .map(str::to_string)
    };
    let mut lines = ids.lines().skip_while(|l| vendor_name(l).is_none());
    let Some(vendor_line) = lines.next() else {
        return (None, None);
    };
    let device_name = lines
        .take_while(|l| l.starts_with('\t') || l.starts_with('#'))
        .find_map(|l| {
            Some(
                l.strip_prefix('\t')?
                    .strip_prefix(device)?
                    .strip_prefix("  ")?
                    .to_string(),
            )
        });
    (vendor_name(vendor_line), device_name)
}

pub fn memory(ctx: &Ctx) -> Option<Value> {
    let info = ctx.meminfo()?;
    let kib = |key: &str| {
        info.lines().find_map(|l| {
            l.strip_prefix(key)?
                .strip_prefix(':')?
                .trim()
                .strip_suffix(" kB")?
                .parse::<u64>()
                .ok()
        })
    };
    let total = kib("MemTotal")?;
    // Same formula as neofetch (and htop).
    let free = kib("MemFree")?
        + kib("Buffers").unwrap_or(0)
        + kib("Cached").unwrap_or(0)
        + kib("SReclaimable").unwrap_or(0);
    let used = (total + kib("Shmem").unwrap_or(0)).saturating_sub(free);
    Some(Value::Memory(Usage::new(
        used * 1024,
        total * 1024,
        total * 1024,
    )))
}

pub fn disk(ctx: &Ctx) -> Option<Value> {
    let st = ctx.statvfs("/")?;
    let block = st.frsize;
    let total = st.blocks * block;
    let used = (st.blocks - st.bfree) * block;
    // Like df: percentage of the space available to unprivileged users.
    Some(Value::Disk(Usage::new(
        used,
        total,
        used + st.bavail * block,
    )))
}

pub fn battery(ctx: &Ctx) -> Option<Value> {
    let batteries: Vec<Battery> = ctx
        .sorted_dir("/sys/class/power_supply")
        .into_iter()
        .filter(|p| ctx.read(p.join("type")).as_deref() == Some("Battery"))
        .filter_map(|p| {
            Some(Battery {
                pct: ctx.read(p.join("capacity"))?.parse().ok()?,
                status: ctx.read(p.join("status")),
            })
        })
        .collect();
    (!batteries.is_empty()).then_some(Value::Battery(Batteries { batteries }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: &str = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t164e  Raphael
10de  NVIDIA Corporation
# a comment between devices
\t2684  AD102 [GeForce RTX 4090]
\t\t1043 889d  ROG Strix
1af4  Red Hat, Inc.
\t2684  Not NVIDIA
";

    #[test]
    fn pci_lookup_finds_vendor_and_device() {
        let lookup = |v, d| pci_lookup(IDS, v, d);
        assert_eq!(
            lookup("10de", "2684"),
            (
                Some("NVIDIA Corporation".into()),
                Some("AD102 [GeForce RTX 4090]".into())
            )
        );
        assert_eq!(
            lookup("10de", "ffff"),
            (Some("NVIDIA Corporation".into()), None)
        );
        assert_eq!(lookup("abcd", "2684"), (None, None));
    }

    #[test]
    fn gpu_names_from_pci_ids() {
        assert_eq!(
            gpu_name(Some(IDS), "10de", "2684"),
            "NVIDIA GeForce RTX 4090"
        );
        assert_eq!(gpu_name(Some(IDS), "1002", "164e"), "AMD Raphael");
        assert_eq!(gpu_name(Some(IDS), "1002", "1234"), "AMD Device 1234");
        assert_eq!(gpu_name(None, "abcd", "0001"), "abcd Device 0001");
    }
}
