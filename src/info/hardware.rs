//! Modules about the hardware and how busy it is: host model, CPU, CPU
//! temperature, load, GPU, memory, disk, battery and local IP address.

use std::{
    net::{IpAddr, Ipv4Addr},
    path::Path,
};

use super::{
    Ctx,
    ctx::IfAddr,
    value::{
        Batteries, Battery, Cpu, Gpu, GpuSource, Gpus, Load, LocalIp, Name, Temp, Usage, Value,
    },
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
    let threads = cpu_threads(info);
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

/// Logical CPUs: the entries in /proc/cpuinfo.
fn cpu_threads(cpuinfo: &str) -> usize {
    cpuinfo
        .lines()
        .filter(|l| l.starts_with("processor"))
        .count()
}

/// hwmon drivers whose first sensor (`temp1`) is the CPU package or die.
const CPU_SENSORS: &[&str] = &["coretemp", "k10temp", "zenpower"];

pub fn temp(ctx: &Ctx) -> Option<Value> {
    // Both report millidegrees Celsius.
    let millidegrees = |path: &Path| ctx.read(path)?.parse::<f64>().ok();
    let hwmon = || {
        ctx.sorted_dir("/sys/class/hwmon")
            .into_iter()
            .filter(|p| {
                ctx.read(p.join("name"))
                    .is_some_and(|n| CPU_SENSORS.contains(&n.as_str()))
            })
            .find_map(|p| millidegrees(&p.join("temp1_input")))
    };
    // Intel CPUs without the coretemp driver, e.g. in some VMs.
    let thermal_zone = || {
        ctx.sorted_dir("/sys/class/thermal")
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("thermal_zone"))
            })
            .filter(|p| ctx.read(p.join("type")).as_deref() == Some("x86_pkg_temp"))
            .find_map(|p| millidegrees(&p.join("temp")))
    };
    let millidegrees = hwmon().or_else(thermal_zone)?;
    Some(Value::Temp(Temp {
        celsius: millidegrees / 1000.0,
    }))
}

pub fn load(ctx: &Ctx) -> Option<Value> {
    let [load1, load5, load15] = loadavg(&ctx.read("/proc/loadavg")?)?;
    Some(Value::Load(Load {
        load1,
        load5,
        load15,
        threads: ctx.cpuinfo().map_or(0, cpu_threads),
    }))
}

/// The three averages in /proc/loadavg: "0.14 0.15 0.08 1/612 4242".
fn loadavg(text: &str) -> Option<[f64; 3]> {
    let mut words = text.split_whitespace().map(|w| w.parse().ok());
    Some([words.next()??, words.next()??, words.next()??])
}

pub fn gpu(ctx: &Ctx) -> Option<Value> {
    // On WSL the PCI scan only finds the virtual "Microsoft Basic Render Driver".
    let wsl_gpus = || {
        let fact = ctx.cached(wsl::GPU_FACT, || wsl::gpus(ctx))?;
        wsl::parse_gpus(&fact)
    };
    let gpus = ctx.wsl.and_then(|_| wsl_gpus()).or_else(|| pci_gpus(ctx))?;
    Some(Value::Gpu(Gpus { gpus }))
}

fn pci_gpus(ctx: &Ctx) -> Option<Vec<Gpu>> {
    let mut pci_ids: Option<Option<String>> = None;
    let mut gpus: Vec<Gpu> = Vec::new();
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
        let gpu = pci_gpu(ids.as_deref(), &vendor, &device);
        if !gpus.contains(&gpu) {
            gpus.push(gpu);
        }
    }
    (!gpus.is_empty()).then_some(gpus)
}

/// The GPU with these PCI IDs. Without a vendor name (from the short list
/// below or pci.ids), the vendor ID is shown as part of the name.
fn pci_gpu(ids: Option<&str>, vendor: &str, device: &str) -> Gpu {
    let (vendor_name, device_name) = ids
        .map(|ids| pci_lookup(ids, vendor, device))
        .unwrap_or_default();
    let known = match vendor {
        "10de" => Some("NVIDIA"),
        "1002" => Some("AMD"),
        "8086" => Some("Intel"),
        "1414" => Some("Microsoft"),
        "15ad" => Some("VMware"),
        "80ee" => Some("VirtualBox"),
        "1af4" => Some("Red Hat"),
        "1234" => Some("QEMU"),
        _ => None,
    };
    // "GA102 [GeForce RTX 3080]" -> "GeForce RTX 3080"
    let model = match device_name {
        Some(d) => match (d.rfind('['), d.rfind(']')) {
            (Some(a), Some(b)) if a < b => d[a + 1..b].to_string(),
            _ => d,
        },
        None => format!("Device {device}"),
    };
    match known.map(str::to_string).or(vendor_name) {
        Some(vendor) => Gpu {
            vendor: Some(vendor),
            name: model,
            source: GpuSource::Pci,
        },
        None => Gpu {
            vendor: None,
            name: format!("{vendor} {model}"),
            source: GpuSource::Pci,
        },
    }
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

pub fn ip(ctx: &Ctx) -> Option<Value> {
    let (interface, address) = local_ipv4(ctx.interfaces())?;
    Some(Value::Ip(LocalIp { address, interface }))
}

/// The first IPv4 address that isn't loopback, on an interface that is up and
/// connected.
fn local_ipv4(addrs: Vec<IfAddr>) -> Option<(String, Ipv4Addr)> {
    addrs.into_iter().find_map(|a| match a.addr {
        IpAddr::V4(v4) if a.up && a.running && !a.loopback && !v4.is_loopback() => {
            Some((a.name, v4))
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::info::fixture::Tree;

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
        let gpu = |ids, vendor, device| {
            let gpu = pci_gpu(ids, vendor, device);
            assert_eq!(gpu.source, GpuSource::Pci);
            (gpu.vendor.clone(), gpu.name.clone(), gpu.full_name())
        };
        let named = |vendor: &str, name: &str| {
            (
                Some(vendor.to_string()),
                name.to_string(),
                format!("{vendor} {name}"),
            )
        };
        assert_eq!(
            gpu(Some(IDS), "10de", "2684"),
            named("NVIDIA", "GeForce RTX 4090")
        );
        assert_eq!(gpu(Some(IDS), "1002", "164e"), named("AMD", "Raphael"));
        assert_eq!(gpu(Some(IDS), "1002", "1234"), named("AMD", "Device 1234"));
        // A vendor only pci.ids knows.
        let ids = "1a03  ASPEED Technology, Inc.\n\t2000  ASPEED Graphics Family\n";
        assert_eq!(
            gpu(Some(ids), "1a03", "2000"),
            named("ASPEED Technology, Inc.", "ASPEED Graphics Family")
        );
        // No name for the vendor at all.
        let unknown = "abcd Device 0001".to_string();
        assert_eq!(gpu(None, "abcd", "0001"), (None, unknown.clone(), unknown));
    }

    #[test]
    fn load_averages() {
        assert_eq!(
            loadavg("0.14 0.15 0.08 1/612 4242\n"),
            Some([0.14, 0.15, 0.08])
        );
        assert_eq!(loadavg("12.50 7.00 3.25"), Some([12.5, 7.0, 3.25]));
        assert_eq!(loadavg("0.14 0.15"), None);
        assert_eq!(loadavg("0.14 x 0.08 1/612 4242"), None);
        assert_eq!(
            cpu_threads("processor\t: 0\nmodel name\t: x\n\nprocessor\t: 1\n"),
            2
        );
    }

    /// The CPU temperature found in a machine with only these files.
    fn celsius_in(test: &str, files: &[(&str, &str)]) -> Option<f64> {
        match temp(&Tree::new(test, files).ctx()) {
            Some(Value::Temp(t)) => Some(t.celsius),
            other => {
                assert_eq!(other, None);
                None
            }
        }
    }

    #[test]
    fn temperature_selection() {
        let hwmon = |n: u32, name: &'static str, temp: &'static str| {
            let dir = format!("/sys/class/hwmon/hwmon{n}");
            [
                (format!("{dir}/name"), format!("{name}\n")),
                (format!("{dir}/temp1_input"), format!("{temp}\n")),
            ]
        };
        let zone = |n: u32, kind: &'static str, temp: &'static str| {
            let dir = format!("/sys/class/thermal/thermal_zone{n}");
            [
                (format!("{dir}/type"), format!("{kind}\n")),
                (format!("{dir}/temp"), format!("{temp}\n")),
            ]
        };
        let celsius = |test, files: Vec<[(String, String); 2]>| {
            let files: Vec<(String, String)> = files.into_iter().flatten().collect();
            let files: Vec<(&str, &str)> = files
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            celsius_in(test, &files)
        };

        // Other sensors (a disk, a GPU) are skipped, whatever their order.
        let amd = vec![
            hwmon(0, "nvme", "38850"),
            hwmon(1, "k10temp", "54125"),
            hwmon(2, "amdgpu", "47000"),
        ];
        assert_eq!(celsius("temp-k10temp", amd), Some(54.125));
        let intel = vec![hwmon(0, "acpitz", "27800"), hwmon(1, "coretemp", "61000")];
        assert_eq!(celsius("temp-coretemp", intel), Some(61.0));
        assert_eq!(
            celsius("temp-zenpower", vec![hwmon(3, "zenpower", "48500")]),
            Some(48.5)
        );
        // hwmon comes first; thermal zones are the fallback, by type.
        let both = vec![
            hwmon(0, "coretemp", "61000"),
            zone(0, "x86_pkg_temp", "70000"),
        ];
        assert_eq!(celsius("temp-both", both), Some(61.0));
        let zones = vec![
            zone(0, "acpitz", "27800"),
            zone(1, "x86_pkg_temp", "57000"),
            hwmon(0, "BAT1", "30000"),
        ];
        assert_eq!(celsius("temp-zone", zones), Some(57.0));
        // A CPU sensor that can't be read falls through to the next source.
        let unreadable = vec![hwmon(0, "k10temp", "N/A"), zone(0, "x86_pkg_temp", "52000")];
        assert_eq!(celsius("temp-unreadable", unreadable), Some(52.0));
        // WSL: a battery and an AC adapter, and no thermal zones.
        let wsl = vec![hwmon(0, "AC1", "0"), hwmon(1, "BAT1", "0")];
        assert_eq!(celsius("temp-wsl", wsl), None);
        assert_eq!(celsius("temp-none", vec![]), None);
    }

    #[test]
    fn local_ip_selection() {
        let addr = |name: &str, addr: &str, flags: &str| IfAddr {
            name: name.into(),
            addr: addr.parse().unwrap(),
            up: flags.contains('u'),
            running: flags.contains('r'),
            loopback: flags.contains('l'),
        };
        let pick = |addrs: Vec<IfAddr>| local_ipv4(addrs).map(|(n, a)| format!("{a} ({n})"));
        let wsl = vec![
            addr("lo", "127.0.0.1", "url"),
            // WSL's DNS tunnel puts a private address on the loopback.
            addr("lo", "10.255.255.254", "url"),
            addr("eth0", "fe80::215:5dff:fe12:3456", "ur"),
            addr("eth0", "172.20.254.26", "ur"),
        ];
        assert_eq!(pick(wsl).as_deref(), Some("172.20.254.26 (eth0)"));
        let desktop = vec![
            addr("lo", "127.0.0.1", "url"),
            // Switched off, and switched on but unplugged.
            addr("enp4s0", "10.0.0.7", "r"),
            addr("virbr0", "192.168.122.1", "u"),
            addr("wlan0", "192.168.1.20", "ur"),
            addr("docker0", "172.17.0.1", "ur"),
        ];
        assert_eq!(pick(desktop).as_deref(), Some("192.168.1.20 (wlan0)"));
        assert_eq!(pick(vec![addr("lo", "127.0.0.1", "url")]), None);
        assert_eq!(pick(vec![]), None);
    }
}
