//! `.cargo/config.toml` 的开发配置（见 docs/fast-dev.md）：依赖开满优化，自己的每个 crate 都单独压回 1。

use std::fs;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("crates/nge-core 在仓库根下两层")
}

/// TOML 里 `[section]` 下（到下一个节为止）`key = value` 的值。
fn value(text: &str, section: &str, key: &str) -> Option<String> {
    let header = format!("[{section}]");
    let mut lines = text.lines().map(str::trim).skip_while(|l| *l != header).skip(1);
    lines.by_ref().take_while(|l| !l.starts_with('[')).find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim() == key).then(|| v.trim().trim_matches('"').to_owned())
    })
}

/// `crates/*`、`examples/*` 里每个 crate 的名字。
fn own_crates() -> Vec<String> {
    let mut names = Vec::new();
    for dir in ["crates", "examples"] {
        for entry in fs::read_dir(root().join(dir)).expect("目录") {
            let manifest = entry.expect("条目").path().join("Cargo.toml");
            let Ok(text) = fs::read_to_string(&manifest) else { continue };
            names.push(value(&text, "package", "name").unwrap_or_else(|| panic!("{} 的 [package] 里没写 name", manifest.display())));
        }
    }
    names.sort();
    names
}

#[test]
fn dependencies_are_optimized_and_own_crates_stay_light() {
    let config = fs::read_to_string(root().join(".cargo/config.toml")).expect("有 .cargo/config.toml");
    let level = |section: &str| value(&config, section, "opt-level");
    assert_eq!(level("profile.dev").as_deref(), Some("1"));
    assert_eq!(level("profile.dev.package.\"*\"").as_deref(), Some("3"));
    let crates = own_crates();
    assert!(crates.len() > 10 && crates.contains(&"platformer".to_owned()), "{crates:?}");
    for name in &crates {
        assert_eq!(level(&format!("profile.dev.package.{name}")).as_deref(), Some("1"), "{name} 在 nsh 的工作区里会被 \"*\" 开满优化，改了重编很慢");
    }
    // 删了的 crate 别留着旧的节
    for line in config.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("[profile.dev.package.").and_then(|l| l.strip_suffix(']'))
            && name != "\"*\""
        {
            assert!(crates.iter().any(|c| c == name), "{name} 已经不是仓库里的 crate");
        }
    }
}

#[test]
fn reads_values_by_section_whatever_the_layout() {
    let text = "[a]\r\ndebug = 1\r\nopt-level = 2\r\n[b]\nopt-level=\"s\"\n";
    assert_eq!(value(text, "a", "opt-level").as_deref(), Some("2"));
    assert_eq!(value(text, "b", "opt-level").as_deref(), Some("s"));
    assert_eq!(value(text, "a", "missing"), None);
    assert_eq!(value(text, "c", "opt-level"), None);
    assert_eq!(value("[a]\n[b]\nopt-level = 3\n", "a", "opt-level"), None, "不读到下一个节里去");
}
