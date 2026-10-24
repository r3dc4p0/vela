//! Vela — a modern, fast terminal text editor.
//!
//! This binary is deliberately thin: it parses arguments, sets up logging and
//! the terminal, then hands control to [`vela::app::App`].

use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use vela::app::App;
use vela::config::{Config, Enhancement, UiStyle};
use vela::logging;
use vela::platform;
use vela::terminal::TerminalGuard;

const USAGE: &str = "\
vela — a modern terminal text editor

USAGE:
    vela [OPTIONS] [FILE...]

OPTIONS:
    -h, --help              Print this help and exit
    -V, --version           Print version information and exit
    -c, --config <PATH>     Use an alternative configuration file
        --ui <STYLE>        Drawing style: pixel (hairlines) or cell (blocks)
        --no-log            Disable file logging
        --print-config      Print the effective configuration and exit
        --list-keys         Print the effective key bindings and exit
        --smoke             Start, draw one frame and exit (startup check)
    -v, --verbose           Verbose logging (debug level)

KEY SHORTCUTS:
    Ctrl+S      Save                     Ctrl+F      Find
    Ctrl+Shift+S Save as                 Ctrl+H      Find and replace
    Ctrl+O      Open file                Ctrl+G      Go to line
    Ctrl+N      New buffer               Ctrl+P      Quick open
    Ctrl+Z      Undo                     Ctrl+Shift+P Command palette
    Ctrl+Y      Redo                     Ctrl+W      Close buffer
    Ctrl+C/X/V  Copy / cut / paste       Ctrl+Tab    Next buffer
    Ctrl+A      Select all               F1          Help
    Ctrl+Q      Quit

    Ctrl+C copies the selection (or the current line) and never quits.
    Esc closes dialogs; Ctrl+Q quits.
";

struct Args {
    files: Vec<PathBuf>,
    config: Option<PathBuf>,
    ui: Option<UiStyle>,
    verbose: bool,
    no_log: bool,
    print_config: bool,
    list_keys: bool,
    smoke: bool,
    help: bool,
    version: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        files: Vec::new(),
        config: None,
        ui: None,
        verbose: false,
        no_log: false,
        print_config: false,
        list_keys: false,
        smoke: false,
        help: false,
        version: false,
    };

    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => args.help = true,
            "-V" | "--version" => args.version = true,
            "-v" | "--verbose" => args.verbose = true,
            "--no-log" => args.no_log = true,
            "--print-config" => args.print_config = true,
            "--list-keys" => args.list_keys = true,
            "--smoke" => args.smoke = true,
            "--ui" => {
                let value = it.next().ok_or("--ui requires a style")?;
                args.ui = Some(
                    UiStyle::parse(&value)
                        .ok_or_else(|| format!("--ui expects `pixel` or `cell`, got `{value}`"))?,
                );
            }
            other if other.starts_with("--ui=") => {
                let value = &other["--ui=".len()..];
                args.ui = Some(
                    UiStyle::parse(value)
                        .ok_or_else(|| format!("--ui expects `pixel` or `cell`, got `{value}`"))?,
                );
            }
            "-c" | "--config" => {
                let value = it.next().ok_or("--config requires a path")?;
                args.config = Some(PathBuf::from(value));
            }
            other if other.starts_with("--config=") => {
                args.config = Some(PathBuf::from(&other["--config=".len()..]));
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option: {other}"));
            }
            other => args.files.push(PathBuf::from(other)),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("vela: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    if args.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!(
            "vela {} ({} {}, {})",
            vela::VERSION,
            platform::os_name(),
            std::env::consts::ARCH,
            platform::term_name()
        );
        return ExitCode::SUCCESS;
    }

    let loaded = Config::load_or_default(args.config.as_deref());
    let mut config = loaded.config;
    let warnings = loaded.warnings.clone();
    // The command line wins over the file, and it has to do so before the
    // normalisation pass so `--print-config` reports what will be drawn.
    if let Some(style) = args.ui {
        config.ui.style = style.name().to_string();
    }
    config.normalize();

    if args.print_config {
        match config.to_toml() {
            Ok(text) => println!("{text}"),
            Err(e) => eprintln!("vela: {e}"),
        }
        return ExitCode::SUCCESS;
    }
    if args.list_keys {
        let mut keymap = vela::keymap::Keymap::defaults();
        keymap.apply_overrides(&config.keys);
        for (chord, cmd) in keymap.all() {
            println!("{:<20} {}", chord.to_config_string(), cmd.id());
        }
        return ExitCode::SUCCESS;
    }

    let _log = logging::init(&platform::log_dir(), args.verbose, args.no_log);

    if !std::io::stdout().is_terminal() {
        eprintln!("vela: stdout is not a terminal");
        return ExitCode::from(1);
    }

    let enhancement =
        Enhancement::parse(&config.terminal.keyboard_enhancement).unwrap_or(Enhancement::Auto);

    let mut guard = match TerminalGuard::enter(config.ui.mouse, enhancement) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("vela: cannot initialise terminal: {e}");
            return ExitCode::from(1);
        }
    };
    vela::terminal::install_signal_handlers();

    let mut terminal = match guard.terminal() {
        Ok(t) => t,
        Err(e) => {
            guard.restore();
            eprintln!("vela: cannot create terminal backend: {e}");
            return ExitCode::from(1);
        }
    };

    let mut app = App::new(
        config,
        loaded.path,
        warnings,
        guard.enhanced_keys(),
        args.files,
    );

    let result = if args.smoke {
        app.run_once(&mut terminal)
    } else {
        app.run(&mut terminal)
    };
    guard.restore();

    match result {
        Ok(()) => ExitCode::from(app.exit_code as u8),
        Err(e) => {
            eprintln!("vela: {e}");
            ExitCode::from(1)
        }
    }
}
