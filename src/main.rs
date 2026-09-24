// SPDX-License-Identifier: Apache-2.0

mod device;
mod process;

use device::{Device, Metrics};
use process::Process;
use std::env;
use std::ffi::CStr;
use std::io::{self, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const FIELDS: &[&str] = &[
    "timestamp",
    "index",
    "name",
    "pci.bus_id",
    "display_active",
    "memory.total",
    "memory.used",
    "memory.free",
    "memory.visible.total",
    "memory.visible.used",
    "memory.gtt.total",
    "memory.gtt.used",
    "utilization.gpu",
    "temperature.gpu",
    "clocks.current.graphics",
    "clocks.current.memory",
    "power.draw",
    "power.limit",
    "fan.speed",
    "ecc.errors.uncorrected.volatile.total",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Table,
    List,
    Detail,
    Query,
    HelpQuery,
}

struct Options {
    mode: Mode,
    id: Option<String>,
    fields: Vec<String>,
    noheader: bool,
    nounits: bool,
    interval: Option<Duration>,
}

fn parse_interval(value: &str, millis: bool) -> Result<Duration, String> {
    let amount: u64 = value
        .parse()
        .map_err(|_| format!("invalid interval: {value}"))?;
    if amount == 0 {
        return Err("interval must be greater than zero".to_owned());
    }
    Ok(if millis {
        Duration::from_millis(amount)
    } else {
        Duration::from_secs(amount)
    })
}

fn take_value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        mode: Mode::Table,
        id: None,
        fields: Vec::new(),
        noheader: false,
        nounits: false,
        interval: None,
    };
    let mut format = None;
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-L" | "--list-gpus" => options.mode = Mode::List,
            "-q" | "--query" => options.mode = Mode::Detail,
            "--help-query-gpu" => options.mode = Mode::HelpQuery,
            "-i" | "--id" => options.id = Some(take_value(args, &mut i, arg)?),
            "--query-gpu" => {
                options.mode = Mode::Query;
                options.fields = take_value(args, &mut i, arg)?
                    .split(',')
                    .map(str::to_owned)
                    .collect();
            }
            "--format" => format = Some(take_value(args, &mut i, arg)?),
            "-l" | "--loop" => {
                let value = if args
                    .get(i + 1)
                    .is_some_and(|next| next.chars().all(|c| c.is_ascii_digit()))
                {
                    take_value(args, &mut i, arg)?
                } else {
                    "5".to_owned()
                };
                options.interval = Some(parse_interval(&value, false)?);
            }
            "-lms" | "--loop-ms" => {
                options.interval = Some(parse_interval(&take_value(args, &mut i, arg)?, true)?)
            }
            _ if arg.starts_with("--id=") => options.id = Some(arg[5..].to_owned()),
            _ if arg.starts_with("--query-gpu=") => {
                options.mode = Mode::Query;
                options.fields = arg[12..].split(',').map(str::to_owned).collect();
            }
            _ if arg.starts_with("--format=") => format = Some(arg[9..].to_owned()),
            _ if arg.starts_with("--loop-ms=") => {
                options.interval = Some(parse_interval(&arg[10..], true)?)
            }
            _ if arg.starts_with("--loop=") => {
                options.interval = Some(parse_interval(&arg[7..], false)?)
            }
            _ if arg.starts_with("-lms") && arg.len() > 4 => {
                options.interval = Some(parse_interval(&arg[4..], true)?)
            }
            _ if arg.starts_with("-l") && arg.len() > 2 => {
                options.interval = Some(parse_interval(&arg[2..], false)?)
            }
            _ if arg == "--query-compute-apps" || arg.starts_with("--query-compute-apps=") => {
                return Err(
                    "per-process compute accounting is not available on legacy Radeon GPUs"
                        .to_owned(),
                );
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "--version" => {
                println!("radeon-smi {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            _ => return Err(format!("unknown option: {arg}")),
        }
        i += 1;
    }
    if options.mode == Mode::Query {
        if options.fields.is_empty() || options.fields.iter().any(|s| !FIELDS.contains(&s.as_str()))
        {
            return Err("unknown query field; use --help-query-gpu".to_owned());
        }
        let format = format.ok_or("--query-gpu requires --format=csv".to_owned())?;
        let mut parts = format.split(',');
        if parts.next() != Some("csv") {
            return Err("only --format=csv is supported".to_owned());
        }
        for part in parts {
            match part {
                "noheader" => options.noheader = true,
                "nounits" => options.nounits = true,
                _ => return Err(format!("unknown CSV format option: {part}")),
            }
        }
    } else if format.is_some() {
        return Err("--format requires --query-gpu".to_owned());
    }
    Ok(options)
}

fn print_help() {
    println!(
        "radeon-smi {} — read-only AMD GPU monitoring",
        env!("CARGO_PKG_VERSION")
    );
    println!("Usage: radeon-smi [options]");
    println!("  -L, --list-gpus          List GPUs");
    println!("  -i, --id ID              Select GPU by index or PCI bus ID");
    println!("  -q, --query              Show detailed metrics");
    println!("      --query-gpu=FIELDS   Query comma-separated GPU fields");
    println!("      --format=csv[,noheader][,nounits]");
    println!("      --help-query-gpu     List query fields");
    println!("  -l, --loop[=SEC]         Repeat output (default: 5 seconds)");
    println!("  -lms, --loop-ms=MS       Repeat output in milliseconds");
    println!("  -h, --help               Show this help");
    println!("      --version            Show version");
}

fn timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    let mut buffer = [0_i8; 64];
    // SAFETY: localtime_r writes a valid tm; strftime writes at most buffer.len() bytes.
    let size = unsafe {
        if libc::localtime_r(&seconds, local.as_mut_ptr()).is_null() {
            return seconds.to_string();
        }
        libc::strftime(
            buffer.as_mut_ptr(),
            buffer.len(),
            b"%a %b %d %H:%M:%S %Y\0".as_ptr().cast(),
            local.as_ptr(),
        )
    };
    if size == 0 {
        return seconds.to_string();
    }
    // SAFETY: strftime terminates its output on success.
    unsafe { CStr::from_ptr(buffer.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn mib(bytes: u64) -> u64 {
    bytes.saturating_add(524_288) / 1_048_576
}

fn bus_id(id: &str) -> String {
    if let Some((domain, rest)) = id.split_once(':') {
        if let Ok(domain) = u32::from_str_radix(domain, 16) {
            return format!("{domain:08X}:{}", rest.to_ascii_uppercase());
        }
    }
    id.to_owned()
}

fn value(field: &str, gpu: &Device, m: &Metrics, stamp: &str, nounits: bool) -> String {
    let unit = |number: String, suffix: &str| {
        if nounits {
            number
        } else {
            format!("{number} {suffix}")
        }
    };
    match field {
        "timestamp" => stamp.to_owned(),
        "index" => gpu.index.to_string(),
        "name" => gpu.name.clone(),
        "pci.bus_id" => bus_id(&gpu.bus_id),
        "display_active" => m
            .display_active
            .map(|v| if v { "Enabled" } else { "Disabled" }.to_owned())
            .unwrap_or_else(|| "N/A".to_owned()),
        "memory.total" => m
            .memory_total
            .map(|v| unit(mib(v).to_string(), "MiB"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "memory.used" => m
            .memory_used
            .map(|v| unit(mib(v).to_string(), "MiB"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "memory.free" => m
            .memory_total
            .zip(m.memory_used)
            .map(|(total, used)| unit(mib(total.saturating_sub(used)).to_string(), "MiB"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "memory.visible.total" => m
            .visible_memory_total
            .map(|v| unit(mib(v).to_string(), "MiB"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "memory.visible.used" => m
            .visible_memory_used
            .map(|v| unit(mib(v).to_string(), "MiB"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "memory.gtt.total" => m
            .gtt_total
            .map(|v| unit(mib(v).to_string(), "MiB"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "memory.gtt.used" => m
            .gtt_used
            .map(|v| unit(mib(v).to_string(), "MiB"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "utilization.gpu" => m
            .utilization
            .map(|v| unit(v.to_string(), "%"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "temperature.gpu" => m
            .temperature_c
            .map(|v| unit(v.to_string(), "C"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "clocks.current.graphics" => m
            .graphics_mhz
            .map(|v| unit(v.to_string(), "MHz"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "clocks.current.memory" => m
            .memory_mhz
            .map(|v| unit(v.to_string(), "MHz"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "power.draw" => m
            .power_watts
            .map(|v| unit(format!("{v:.2}"), "W"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "power.limit" => m
            .power_cap_watts
            .map(|v| unit(format!("{v:.2}"), "W"))
            .unwrap_or_else(|| "N/A".to_owned()),
        "fan.speed" => m
            .fan_rpm
            .map(|v| unit(v.to_string(), "RPM"))
            .unwrap_or_else(|| "N/A".to_owned()),
        _ => "N/A".to_owned(),
    }
}

fn shorten(input: &str, width: usize) -> String {
    input
        .chars()
        .filter(|c| !c.is_control())
        .take(width)
        .collect()
}

fn frame(text: &str) -> String {
    format!("| {:<87} |", shorten(text, 87))
}

fn title(left: &str, right: &str) -> String {
    let left = shorten(left, 43);
    let right = shorten(right, 44);
    format!("| {left}{:>width$} |", right, width = 87 - left.len())
}

fn cells(left: &str, middle: &str, right: &str) -> String {
    format!("|{left}|{middle}|{right}|")
}

fn gpu_name(gpu: &str, name: &str) -> String {
    format!(" {:<4}{:>35} ", shorten(gpu, 4), shorten(name, 35))
}

fn bus_display(bus: &str, display: &str) -> String {
    format!(" {:<16}{:>6} ", shorten(bus, 16), shorten(display, 6))
}

fn temp_power_clocks(temp: &str, power: &str, clocks: &str) -> String {
    format!(
        " {:<8}{:<12}{:>19} ",
        shorten(temp, 8),
        shorten(power, 12),
        shorten(clocks, 19)
    )
}

fn memory_cell(value: &str) -> String {
    format!("{:>23} ", shorten(value, 23))
}

fn right_cell(value: &str) -> String {
    format!("{:>21} ", shorten(value, 21))
}

fn table(devices: &[(Device, Metrics)], processes: &[Process], stamp: &str) {
    let border = format!("+{}+", "-".repeat(89));
    let split = format!("+{}+{}+{}+", "-".repeat(41), "-".repeat(24), "-".repeat(22));
    println!("{stamp}\n{border}");
    println!(
        "{}",
        title(
            &format!("RADEON-SMI {}", env!("CARGO_PKG_VERSION")),
            &format!("Driver: {}", devices[0].0.driver)
        )
    );
    println!("{split}");
    println!(
        "{}",
        cells(
            &gpu_name("GPU", "Name"),
            &bus_display("Bus-Id", "Disp.A"),
            &right_cell("Volatile Uncorr. ECC")
        )
    );
    println!(
        "{}",
        cells(
            &temp_power_clocks("Temp", "Power", "Clocks GFX/MEM"),
            &memory_cell("Memory-Usage"),
            &right_cell("GPU-Util")
        )
    );
    println!("{}", split.replace('-', "="));
    for (gpu, m) in devices {
        let display = m
            .display_active
            .map(|v| if v { "On" } else { "Off" })
            .unwrap_or("N/A");
        let temp = m
            .temperature_c
            .map(|v| format!("{v}C"))
            .unwrap_or_else(|| "N/A".to_owned());
        let memory = m
            .memory_used
            .zip(m.memory_total)
            .map(|(used, total)| format!("{}MiB / {}MiB", mib(used), mib(total)))
            .unwrap_or_else(|| "N/A".to_owned());
        let util = m
            .utilization
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "N/A".to_owned());
        let power = m
            .power_watts
            .zip(m.power_cap_watts)
            .map(|(draw, cap)| format!("{draw:.0}W / {cap:.0}W"))
            .unwrap_or_else(|| "N/A".to_owned());
        let gfx = m
            .graphics_mhz
            .map(|v| format!("{v}MHz"))
            .unwrap_or_else(|| "N/A".to_owned());
        let mem = m
            .memory_mhz
            .map(|v| format!("{v}MHz"))
            .unwrap_or_else(|| "N/A".to_owned());
        println!(
            "{}",
            cells(
                &gpu_name(&gpu.index.to_string(), &gpu.name),
                &bus_display(&bus_id(&gpu.bus_id), display),
                &right_cell("N/A")
            )
        );
        println!(
            "{}",
            cells(
                &temp_power_clocks(&temp, &power, &format!("{gfx}/{mem}")),
                &memory_cell(&memory),
                &right_cell(&util)
            )
        );
        if let Some(issue) = &m.issue {
            println!("{}", frame(&format!("Telemetry: {issue}")));
        }
        println!("{split}");
    }
    println!();
    println!("{border}");
    println!("{}", frame("Processes: GPU device users"));
    println!("{border}");
    println!(
        "| {:<3}  {:<7}  {:<60} {:>12} |",
        "GPU", "PID", "Process name", "GPU Memory"
    );
    println!("{border}");
    if processes.is_empty() {
        println!("{}", frame("No visible GPU processes"));
    } else {
        for p in processes {
            println!(
                "| {:<3}  {:<7}  {:<60} {:>12} |",
                p.gpu,
                p.pid,
                shorten(&p.name, 60),
                p.memory
                    .map(|v| format!("{}MiB", mib(v)))
                    .unwrap_or_else(|| "N/A".to_owned())
            );
        }
    }
    println!(
        "{}",
        frame("Visible processes only (/proc permissions); VRAM uses DRM fdinfo when available")
    );
    println!("{border}");
}

fn detail(devices: &[(Device, Metrics)], stamp: &str) {
    for (gpu, m) in devices {
        println!("GPU {}: {}", gpu.index, gpu.name);
        for field in FIELDS {
            println!("    {field}: {}", value(field, gpu, m, stamp, false));
        }
        if let Some(issue) = &m.issue {
            println!("    telemetry_error: {issue}");
        }
        println!();
    }
}

fn csv_escape(input: &str) -> String {
    if input.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", input.replace('"', "\"\""))
    } else {
        input.to_owned()
    }
}

fn query(devices: &[(Device, Metrics)], stamp: &str, options: &Options, first: bool) {
    if first && !options.noheader {
        println!("{}", options.fields.join(", "));
    }
    for (gpu, m) in devices {
        let row: Vec<_> = options
            .fields
            .iter()
            .map(|field| csv_escape(&value(field, gpu, m, stamp, options.nounits)))
            .collect();
        println!("{}", row.join(", "));
    }
}

fn selected(devices: Vec<Device>, id: &Option<String>) -> Result<Vec<Device>, String> {
    let Some(id) = id else { return Ok(devices) };
    let matches: Vec<_> = devices
        .into_iter()
        .filter(|gpu| {
            gpu.index.to_string() == *id
                || gpu.bus_id.eq_ignore_ascii_case(id)
                || bus_id(&gpu.bus_id).eq_ignore_ascii_case(id)
        })
        .collect();
    if matches.is_empty() {
        Err(format!("GPU not found: {id}"))
    } else {
        Ok(matches)
    }
}

fn run(options: Options) -> Result<(), String> {
    if options.mode == Mode::HelpQuery {
        println!("Supported --query-gpu fields (unsupported hardware values are N/A):");
        for field in FIELDS {
            println!("  {field}");
        }
        return Ok(());
    }
    let mut first = true;
    loop {
        let start = Instant::now();
        let devices = selected(
            device::discover().map_err(|e| format!("cannot discover DRM devices: {e}"))?,
            &options.id,
        )?;
        if devices.is_empty() {
            return Err("no Radeon DRM GPUs found (radeon or amdgpu)".to_owned());
        }
        if options.mode == Mode::List {
            for gpu in &devices {
                println!(
                    "GPU {}: {} (PCI {})",
                    gpu.index,
                    gpu.name,
                    bus_id(&gpu.bus_id)
                );
            }
        } else {
            let processes = if options.mode == Mode::Table {
                process::discover(&devices)
            } else {
                Vec::new()
            };
            let devices: Vec<_> = devices
                .into_iter()
                .map(|gpu| {
                    let m = device::collect(&gpu);
                    (gpu, m)
                })
                .collect();
            let stamp = timestamp();
            match options.mode {
                Mode::Table => table(&devices, &processes, &stamp),
                Mode::Detail => detail(&devices, &stamp),
                Mode::Query => query(&devices, &stamp, &options, first),
                _ => unreachable!(),
            }
        }
        io::stdout().flush().map_err(|e| e.to_string())?;
        first = false;
        let Some(interval) = options.interval else {
            break;
        };
        if options.mode != Mode::Query {
            println!();
        }
        std::thread::sleep(interval.saturating_sub(start.elapsed()));
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = parse(&args).and_then(run);
    if let Err(err) = result {
        eprintln!("radeon-smi: {err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_needs_csv_format() {
        let args = vec!["--query-gpu=memory.used".to_owned()];
        assert!(parse(&args).is_err());
        let args = vec![
            "--query-gpu=memory.used".to_owned(),
            "--format=csv,noheader,nounits".to_owned(),
        ];
        let options = parse(&args).unwrap();
        assert!(options.noheader && options.nounits);
        assert!(parse(&[
            "--query-gpu=driver_version".to_owned(),
            "--format=csv".to_owned()
        ])
        .is_err());
    }

    #[test]
    fn csv_escapes_names() {
        assert_eq!(csv_escape("A, \"B\""), "\"A, \"\"B\"\"\"");
    }

    #[test]
    fn invalid_intervals_fail() {
        assert!(parse_interval("0", false).is_err());
        assert!(parse_interval("abc", true).is_err());
    }

    #[test]
    fn table_headers_and_values_share_column_positions() {
        let header = cells(
            &gpu_name("GPU", "Name"),
            &bus_display("Bus-Id", "Disp.A"),
            &right_cell("Volatile Uncorr. ECC"),
        );
        let data = cells(
            &gpu_name("0", "Radeon R7 250"),
            &bus_display("00000000:01:00.0", "Off"),
            &right_cell("N/A"),
        );
        assert_eq!(header.len(), 91);
        assert_eq!(data.len(), 91);
        assert_eq!(
            header.find("Name").unwrap() + 4,
            data.find("Radeon R7 250").unwrap() + 13
        );
        assert_eq!(header.find("Bus-Id"), data.find("00000000:01:00.0"));
        assert_eq!(
            header.find("Disp.A").unwrap() + 6,
            data.find("Off").unwrap() + 3
        );
        assert_eq!(
            header.find("ECC").unwrap() + 3,
            data.find("N/A").unwrap() + 3
        );

        let header = cells(
            &temp_power_clocks("Temp", "Power", "Clocks GFX/MEM"),
            &memory_cell("Memory-Usage"),
            &right_cell("GPU-Util"),
        );
        let data = cells(
            &temp_power_clocks("30C", "N/A", "300MHz/300MHz"),
            &memory_cell("361MiB / 2048MiB"),
            &right_cell("0%"),
        );
        assert_eq!(header.len(), 91);
        assert_eq!(data.len(), 91);
        assert_eq!(header.find("Temp"), data.find("30C"));
        assert_eq!(header.find("Power"), data.find("N/A"));
        assert_eq!(
            header.find("GFX/MEM").unwrap() + 7,
            data.find("300MHz/300MHz").unwrap() + 13
        );
        assert_eq!(
            header.find("Memory-Usage").unwrap() + 12,
            data.find("361MiB / 2048MiB").unwrap() + "361MiB / 2048MiB".len()
        );
        assert_eq!(
            header.find("GPU-Util").unwrap() + 8,
            data.find("0%").unwrap() + 2
        );
    }
}
