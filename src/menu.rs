use std::io::{self, BufRead, Write};
use std::path::Path;

use crate::settings::Settings;

pub enum Choice {
    Dashboard,
    Console,
}

/// `None` when the console has no more input, for example when started without a window.
fn prompt(text: &str) -> Option<String> {
    print!("{text}");
    let _ = io::stdout().flush();
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        Ok(n) if n > 0 => Some(line.trim().to_owned()),
        _ => None,
    }
}

pub fn choose(settings_path: &Path) -> Choice {
    loop {
        println!("\nZippy {}", crate::update::current_version());
        println!("  1) Open the dashboard in your browser (default)");
        println!("  2) Run here in this window");
        println!("  3) Settings");
        let Some(answer) = prompt("Choose 1, 2 or 3, then press Enter: ") else { return Choice::Dashboard };
        match answer.as_str() {
            "" | "1" => return Choice::Dashboard,
            "2" => return Choice::Console,
            "3" => edit(settings_path),
            _ => println!("Type 1, 2 or 3."),
        }
    }
}

fn hint(key: &str) -> &'static str {
    match key {
        "mode" => "hunt, battle or both",
        "gem1" => "51 to 57",
        "gem3" => "65 to 71",
        "gem4" => "72 to 78",
        _ => "on or off",
    }
}

fn edit(path: &Path) {
    loop {
        let settings = match Settings::load_or_create(path) {
            Ok((settings, _)) => settings,
            Err(e) => {
                println!("[!] {e}");
                println!("    Showing the defaults; saving replaces the file.");
                Settings::default()
            }
        };
        let values = settings.values();
        println!("\nSettings ({})", path.display());
        for (i, (key, value)) in values.iter().enumerate() {
            println!("  {:>2}) {key:<13} {value:<7} ({})", i + 1, hint(key));
        }
        let Some(answer) = prompt("Number to change, or Enter to go back: ") else { return };
        if answer.is_empty() {
            return;
        }
        let Some((key, _)) = answer.parse::<usize>().ok().and_then(|n| values.get(n.wrapping_sub(1))) else {
            println!("Type a number from the list.");
            continue;
        };
        let Some(value) = prompt(&format!("New value for {key} ({}): ", hint(key))) else { return };
        match settings.with(key, &value).and_then(|changed| changed.save(path)) {
            Ok(()) => println!("[OK] {key} = {value}"),
            Err(e) => println!("[!] {e}"),
        }
    }
}
