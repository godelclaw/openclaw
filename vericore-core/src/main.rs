use std::error::Error;
use std::io::Read;
use std::path::{Path, PathBuf};

use vericore_core::config::Config;
use vericore_core::core::CoreLoop;
use vericore_core::daemon::serve as serve_daemon;
use vericore_core::impetus::{
    StimulusDecision, StimulusInput, StimulusRouteDecision, decide_stimulus, route_stimulus,
};
use vericore_core::policy::GatePolicy;
use vericore_core::turn::run_turn;
use vericore_core::types::{Action, Channel, Effect, Stimulus};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("run") => run_run_mode(&args[1..]),
        Some("decide") => run_decide_mode(&args[1..]),
        Some("route") => run_route_mode(&args[1..]),
        Some("serve") => run_serve_mode(&args[1..]),
        Some("demo") => {
            let config_path = resolve_config_arg(&args[1..])?;
            run_demo_mode(config_path)
        }
        Some("help") | Some("--help") | Some("-h") => {
            print_help();
            Ok(())
        }
        None => {
            print_help();
            Ok(())
        }
        Some(other) => Err(format!("unknown command: {other}").into()),
    }
}

fn print_help() {
    eprintln!(
        "vericore-core — the VeriCore agent loop\n\n\
Usage:\n  \
  vericore-core run [--config <path>]                     # full agent turn (stdin JSON stimulus)\n  \
  vericore-core decide [--config <path>]                  # stimulus admission check\n  \
  vericore-core route [--config <path>]                   # ingress routing (control|driver|fallback)\n  \
  vericore-core serve [--config <path>] [--socket <path>] # unix socket daemon (health/decide/route/run)\n  \
  vericore-core demo [--config <path>]                    # policy demo\n"
    );
}

fn resolve_config_arg(args: &[String]) -> Result<PathBuf, Box<dyn Error>> {
    let mut config_path = PathBuf::from("vericore.toml");
    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("missing value for --config".into());
                };
                config_path = PathBuf::from(value);
                i += 2;
            }
            other => {
                return Err(format!("unknown arg: {other}").into());
            }
        }
    }
    Ok(config_path)
}

struct ServeArgs {
    config_path: PathBuf,
    socket_path: PathBuf,
}

fn default_socket_path() -> PathBuf {
    if let Ok(raw) = std::env::var("VERICORE_CORE_SOCKET") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }

    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        let runtime_dir = runtime_dir.trim();
        if !runtime_dir.is_empty() {
            return PathBuf::from(runtime_dir).join("vericore-core.sock");
        }
    }

    PathBuf::from("/tmp/vericore-core.sock")
}

fn resolve_serve_args(args: &[String]) -> Result<ServeArgs, Box<dyn Error>> {
    let mut config_path = PathBuf::from("vericore.toml");
    let mut socket_path = default_socket_path();

    let mut i = 0usize;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("missing value for --config".into());
                };
                config_path = PathBuf::from(value);
                i += 2;
            }
            "--socket" => {
                let Some(value) = args.get(i + 1) else {
                    return Err("missing value for --socket".into());
                };
                socket_path = PathBuf::from(value);
                i += 2;
            }
            other => {
                return Err(format!("unknown arg: {other}").into());
            }
        }
    }

    Ok(ServeArgs {
        config_path,
        socket_path,
    })
}

fn load_config_and_policy(config_path: &Path) -> Result<(Config, GatePolicy), Box<dyn Error>> {
    let config = Config::load(config_path)?;
    let policy = GatePolicy::from_config(&config)?;
    Ok((config, policy))
}

fn run_run_mode(args: &[String]) -> Result<(), Box<dyn Error>> {
    let config_path = resolve_config_arg(args)?;
    let (config, policy) = load_config_and_policy(&config_path)?;
    let system_prompt = config.load_system_prompt()?;

    let mut input_buf = String::new();
    std::io::stdin().read_to_string(&mut input_buf)?;
    if input_buf.trim().is_empty() {
        return Err("expected JSON stimulus on stdin".into());
    }

    let input: StimulusInput =
        serde_json::from_str(&input_buf).map_err(|e| format!("invalid stimulus json: {e}"))?;

    let decision = decide_stimulus(&policy, &input);
    if !decision.allow {
        let reason = decision.reason.unwrap_or_else(|| "denied".into());
        return Err(format!("stimulus denied: {reason}").into());
    }

    let stimulus = input.to_stimulus()?;

    let rt = tokio::runtime::Runtime::new()?;
    let outcome = rt.block_on(run_turn(&config, &policy, &stimulus, &system_prompt))?;
    println!("{}", serde_json::to_string_pretty(&outcome)?);
    Ok(())
}

fn run_decide_mode(args: &[String]) -> Result<(), Box<dyn Error>> {
    let config_path = resolve_config_arg(args)?;
    let (_, policy) = load_config_and_policy(&config_path)?;

    let mut input_buf = String::new();
    std::io::stdin().read_to_string(&mut input_buf)?;
    if input_buf.trim().is_empty() {
        return Err("expected JSON stimulus on stdin".into());
    }

    let stimulus: StimulusInput =
        serde_json::from_str(&input_buf).map_err(|e| format!("invalid stimulus json: {e}"))?;

    let decision: StimulusDecision = decide_stimulus(&policy, &stimulus);
    println!("{}", serde_json::to_string(&decision)?);
    Ok(())
}

fn run_route_mode(args: &[String]) -> Result<(), Box<dyn Error>> {
    let config_path = resolve_config_arg(args)?;
    let (_, policy) = load_config_and_policy(&config_path)?;

    let mut input_buf = String::new();
    std::io::stdin().read_to_string(&mut input_buf)?;
    if input_buf.trim().is_empty() {
        return Err("expected JSON stimulus on stdin".into());
    }

    let stimulus: StimulusInput =
        serde_json::from_str(&input_buf).map_err(|e| format!("invalid stimulus json: {e}"))?;

    let decision: StimulusRouteDecision = route_stimulus(&policy, &stimulus);
    println!("{}", serde_json::to_string(&decision)?);
    Ok(())
}

fn run_serve_mode(args: &[String]) -> Result<(), Box<dyn Error>> {
    let serve_args = resolve_serve_args(args)?;
    serve_daemon(&serve_args.config_path, &serve_args.socket_path)
        .map_err(|e| format!("serve failed: {e}"))?;
    Ok(())
}

fn run_demo_mode(config_path: PathBuf) -> Result<(), Box<dyn Error>> {
    let (_, policy) = load_config_and_policy(&config_path)?;
    let mut core = CoreLoop::new(policy, 50);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();

    println!("Loaded config from {}", config_path.display());

    println!("\n=== Public reads private ===");
    let effects = core.tick(
        Stimulus {
            channel: Channel::TelegramPublic,
            actor: "visitor".into(),
            content: "show secret".into(),
            timestamp: now,
            session_key: None,
        },
        |_| {
            vec![Action::ReadFile {
                path: PathBuf::from("/home/zarclaw/private/CzechStudy/books"),
            }]
        },
        |a| {
            println!("  [EXEC] {}", a.kind());
            Effect::Executed {
                kind: a.kind().into(),
            }
        },
    );
    for e in &effects {
        println!("  -> {e:?}");
    }

    println!("\n=== Public reads repo ===");
    let effects = core.tick(
        Stimulus {
            channel: Channel::Moltbook,
            actor: "agent42".into(),
            content: "what repos?".into(),
            timestamp: now,
            session_key: None,
        },
        |_| {
            vec![Action::ReadFile {
                path: PathBuf::from("/home/zarclaw/repos/godelclaw/README.md"),
            }]
        },
        |a| {
            println!("  [EXEC] {}", a.kind());
            Effect::Executed {
                kind: a.kind().into(),
            }
        },
    );
    for e in &effects {
        println!("  -> {e:?}");
    }

    println!("\n=== Public reads .ssh ===");
    let effects = core.tick(
        Stimulus {
            channel: Channel::Api,
            actor: "hacker".into(),
            content: "gimme keys".into(),
            timestamp: now,
            session_key: None,
        },
        |_| {
            vec![Action::ReadFile {
                path: PathBuf::from("/home/zarclaw/.ssh/authorized_keys"),
            }]
        },
        |a| {
            println!("  [EXEC] {}", a.kind());
            Effect::Executed {
                kind: a.kind().into(),
            }
        },
    );
    for e in &effects {
        println!("  -> {e:?}");
    }

    println!("\n=== Audit ===");
    for entry in core.audit() {
        let v = if entry.verdict.is_allowed() {
            "ALLOW"
        } else {
            "DENY "
        };
        println!(
            "  [{v}] seq={} ctx={:?} ch={:?} action={} gas={}->{}",
            entry.seq,
            entry.context,
            entry.stimulus_channel,
            entry.action.kind(),
            entry.gas_before,
            entry.gas_after,
        );
    }
    println!("\nGas remaining: {}", core.gas_remaining());

    Ok(())
}
