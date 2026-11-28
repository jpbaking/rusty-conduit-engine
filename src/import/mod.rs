//! AirSwitch / ZeroOmega / SwitchyOmega `.bak` importer, behind the opt-in `import` feature.
//!
//! [`convert`] maps a `.bak` backup onto routing profiles and rules; [`render_toml`] writes
//! them as a commented TOML configuration.

pub mod airswitch;

use std::fmt::Write as _;

pub use airswitch::{
    convert, slugify, ImportError, ImportOptions, Imported, ImportedProfile, ImportedRule,
};

use crate::routing::{OneOrMany, PortSpec, ProfileConfig, RuleConfig};

/// Quote `s` as a TOML basic string.
pub fn toml_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A TOML key: bare when it only uses `A-Za-z0-9_-`, quoted otherwise.
pub fn toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_string()
    } else {
        toml_string(k)
    }
}

fn string_list(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| toml_string(s)).collect();
    format!("[{}]", inner.join(", "))
}

fn one_or_many(v: &OneOrMany<String>) -> String {
    match v {
        OneOrMany::One(s) => toml_string(s),
        OneOrMany::Many(items) => string_list(items),
    }
}

/// Comment text on one line (newlines in imported names would otherwise end the comment).
fn comment(text: &str) -> String {
    format!("# {}\n", text.replace(['\n', '\r'], " "))
}

fn render_profile(out: &mut String, p: &ImportedProfile) {
    for c in &p.comments {
        out.push_str(&comment(c));
    }
    let _ = writeln!(out, "[profiles.{}]", toml_key(&p.key));
    // Exhaustive, so a new `ProfileConfig` field fails to compile until it is rendered.
    let ProfileConfig {
        kind,
        host,
        port,
        username,
        password,
        password_env,
        password_file,
        bypass,
        tls_insecure_skip_verify,
        http_proxy,
        https_proxy,
        fallback_proxy,
    } = &p.config;
    let _ = writeln!(out, "type = {}", toml_string(kind.as_str()));
    if let Some(h) = host {
        let _ = writeln!(out, "host = {}", toml_string(h));
    }
    if let Some(port) = port {
        let _ = writeln!(out, "port = {port}");
    }
    if let Some(u) = username {
        let _ = writeln!(out, "username = {}", toml_string(u));
    }
    if let Some(pw) = password {
        let _ = writeln!(out, "password = {}", toml_string(pw));
    }
    if let Some(env) = password_env {
        let _ = writeln!(out, "password_env = {}", toml_string(env));
    }
    if let Some(file) = password_file {
        let _ = writeln!(
            out,
            "password_file = {}",
            toml_string(&file.display().to_string())
        );
    }
    if *tls_insecure_skip_verify {
        let _ = writeln!(out, "tls_insecure_skip_verify = true");
    }
    for (key, v) in [
        ("http_proxy", http_proxy),
        ("https_proxy", https_proxy),
        ("fallback_proxy", fallback_proxy),
    ] {
        if let Some(v) = v {
            let _ = writeln!(out, "{key} = {}", toml_string(v));
        }
    }
    if !bypass.is_empty() {
        let _ = writeln!(out, "bypass = {}", string_list(bypass));
    }
    out.push('\n');
}

/// The `key = value` lines of one rule (without the table header).
fn rule_lines(r: &RuleConfig) -> Vec<String> {
    // Exhaustive, so a new `RuleConfig` field fails to compile until it is rendered.
    let RuleConfig {
        name,
        inbound,
        host,
        host_wildcard,
        host_regex,
        cidr,
        ip,
        port,
        host_levels,
        bypass,
        weekday,
        time,
        url_wildcard,
        url_regex,
        keyword,
        match_all,
        disabled,
        resolve_dns,
        bypass_mitm,
        target,
    } = r;
    let mut lines = Vec::new();
    if let Some(n) = name {
        lines.push(format!("name = {}", toml_string(n)));
    }
    if let Some(i) = inbound {
        lines.push(format!("inbound = {}", toml_string(&i.to_string())));
    }
    for (key, v) in [
        ("host", host),
        ("host_wildcard", host_wildcard),
        ("host_regex", host_regex),
        ("cidr", cidr),
        ("ip", ip),
    ] {
        if let Some(v) = v {
            lines.push(format!("{key} = {}", one_or_many(v)));
        }
    }
    if let Some(p) = port {
        let spec = |s: &PortSpec| match s {
            PortSpec::Port(n) => n.to_string(),
            PortSpec::Range(r) => toml_string(r),
        };
        let v = match p {
            OneOrMany::One(s) => spec(s),
            OneOrMany::Many(v) => {
                format!("[{}]", v.iter().map(spec).collect::<Vec<_>>().join(", "))
            }
        };
        lines.push(format!("port = {v}"));
    }
    if let Some(l) = host_levels {
        let mut parts = Vec::new();
        if let Some(min) = l.min {
            parts.push(format!("min = {min}"));
        }
        if let Some(max) = l.max {
            parts.push(format!("max = {max}"));
        }
        lines.push(format!("host_levels = {{ {} }}", parts.join(", ")));
    }
    for (key, v) in [
        ("bypass", bypass),
        ("weekday", weekday),
        ("time", time),
        ("url_wildcard", url_wildcard),
        ("url_regex", url_regex),
        ("keyword", keyword),
    ] {
        if let Some(v) = v {
            lines.push(format!("{key} = {}", one_or_many(v)));
        }
    }
    if *match_all {
        lines.push("match_all = true".into());
    }
    if *disabled {
        lines.push("disabled = true".into());
    }
    if *bypass_mitm {
        lines.push("bypass_mitm = true".into());
    }
    if let Some(d) = resolve_dns {
        lines.push(format!("resolve_dns = {d}"));
    }
    lines.push(format!("target = {}", one_or_many(target)));
    lines
}

fn render_rule(out: &mut String, r: &ImportedRule) {
    for c in &r.comments {
        out.push_str(&comment(c));
    }
    match &r.skipped {
        None => {
            out.push_str("[[routing.rules]]\n");
            for l in rule_lines(&r.rule) {
                out.push_str(&l);
                out.push('\n');
            }
        }
        Some(reason) => {
            out.push_str(&comment(&format!("SKIPPED: {reason}")));
            out.push_str("# [[routing.rules]]\n");
            let name = r.rule.name.as_deref().unwrap_or("");
            out.push_str(&comment(&format!("name = {}", toml_string(name))));
            out.push_str(&comment(&format!(
                "target = {}",
                one_or_many(&r.rule.target)
            )));
        }
    }
    out.push('\n');
}

/// Render an import as a commented TOML configuration. `source_name` is shown in the
/// header (pass a file name, not a full path).
pub fn render_toml(imp: &Imported, source_name: &str) -> String {
    let mut out = String::new();
    out.push_str(&comment(&format!(
        "Generated by rusty-conduit-switch import-airswitch from {source_name:?}"
    )));
    match &imp.switch_profile {
        Some(s) => out.push_str(&comment(&format!("Routing imported from profile {s:?}"))),
        None => out.push_str("# No SwitchProfile was imported; [routing] has no rules.\n"),
    }
    out.push_str(
        "# Review this file, then check it with:\n\
         #   rusty-conduit-switch validate --config <this file>\n\
         # Server settings (bind, ports, MITM, logging) use their defaults; see docs/USER-GUIDE-ADVANCED.md.\n\
         # Built-in profiles \"direct\" and \"block\" always exist and are not declared here.\n",
    );
    if !imp.warnings.is_empty() {
        out.push_str("#\n# Import warnings:\n");
        for w in &imp.warnings {
            out.push_str(&comment(&format!("  - {w}")));
        }
    }
    out.push('\n');

    for p in &imp.profiles {
        render_profile(&mut out, p);
    }

    if let Some(s) = &imp.switch_profile {
        out.push_str(&comment(&format!(
            "Imported from AirSwitch SwitchProfile {s:?} (rules are evaluated top to bottom)"
        )));
    }
    out.push_str("[routing]\n");
    let default = if imp.default.len() == 1 {
        OneOrMany::One(imp.default[0].clone())
    } else {
        OneOrMany::Many(imp.default.clone())
    };
    let _ = writeln!(out, "default = {}", one_or_many(&default));
    out.push('\n');
    for r in &imp.rules {
        render_rule(&mut out, r);
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;

    use super::*;
    use crate::routing::{RoutingConfig, RoutingInput};

    const BAK: &str = r#"{
      "-startupProfileName": "auto switch",
      "schemaVersion": 2,
      "+Work Proxy": {"name": "Work Proxy", "profileType": "FixedProfile",
        "fallbackProxy": {"scheme": "http", "host": "10.0.0.2", "port": 3128},
        "auth": {"fallbackProxy": {"username": "alice", "password": "pa\"ss"}},
        "bypassList": [{"conditionType": "BypassCondition", "pattern": "<local>"},
                       {"conditionType": "BypassCondition", "pattern": "[::1]"}]},
      "+auto switch": {"name": "auto switch", "profileType": "SwitchProfile",
        "defaultProfileName": "direct",
        "rules": [
          {"condition": {"conditionType": "HostWildcardCondition", "pattern": "*.corp.example"},
           "profileName": "Work Proxy", "note": "corp \"hosts\""},
          {"condition": {"conditionType": "HostLevelsCondition", "minValue": 0, "maxValue": 0},
           "profileName": "Work Proxy"},
          {"condition": {"conditionType": "HostRegexCondition", "pattern": "(?<=a)b"},
           "profileName": "Work Proxy"}
        ]}
    }"#;

    #[test]
    fn toml_string_escapes() {
        assert_eq!(toml_string("a\"b\\c\n"), r#""a\"b\\c\n""#);
        assert_eq!(toml_string("\u{1}"), r#""\u0001""#);
        assert_eq!(toml_key("work_proxy"), "work_proxy");
        assert_eq!(toml_key("a b"), "\"a b\"");
    }

    #[test]
    fn rendered_toml_parses_and_validates() {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            profiles: BTreeMap<String, ProfileConfig>,
            routing: RoutingConfig,
        }

        let imp = convert(BAK, &ImportOptions::default()).unwrap();
        let text = render_toml(&imp, "Options.bak");
        assert!(text.contains("# Imported from AirSwitch profile \"Work Proxy\""));
        assert!(text.contains("# SKIPPED:"), "{text}");
        let file: File = toml::from_str(&text).unwrap();
        let input = RoutingInput {
            profiles: file.profiles,
            routing: file.routing,
            ..RoutingInput::default()
        };
        crate::router::Router::from_input(&input).unwrap();
        assert_eq!(input.routing.rules.len(), 2);
        assert_eq!(
            input.profiles["work_proxy"].password.as_deref(),
            Some("pa\"ss")
        );
        assert_eq!(
            input.routing.rules[0].name.as_deref(),
            Some("corp \"hosts\"")
        );
    }

    #[test]
    fn renderer_writes_every_profile_and_rule_field() {
        // B-08: `tls_insecure_skip_verify` and `bypass_mitm` (and the password sources) were
        // dropped by the renderer.
        use crate::routing::ProfileType;
        let config = ProfileConfig {
            kind: ProfileType::Https,
            host: Some("proxy.example".into()),
            port: Some(443),
            username: Some("alice".into()),
            password: None,
            password_env: Some("CORP_PW".into()),
            password_file: Some("/run/secrets/pw".into()),
            bypass: vec!["<local>".into()],
            tls_insecure_skip_verify: true,
            http_proxy: None,
            https_proxy: None,
            fallback_proxy: None,
        };
        let mut rule: RuleConfig = toml::from_str("target = \"corp\"").unwrap();
        rule.host = Some(OneOrMany::One("a.example".into()));
        rule.bypass_mitm = true;
        let imp = Imported {
            profiles: vec![ImportedProfile {
                key: "corp".into(),
                comments: Vec::new(),
                config: config.clone(),
            }],
            rules: vec![ImportedRule {
                rule: rule.clone(),
                comments: Vec::new(),
                skipped: None,
            }],
            default: vec!["direct".into()],
            switch_profile: Some("auto".into()),
            warnings: Vec::new(),
        };
        let text = render_toml(&imp, "x.bak");

        #[derive(Deserialize)]
        struct File {
            profiles: BTreeMap<String, ProfileConfig>,
            routing: RoutingConfig,
        }
        let file: File = toml::from_str(&text).unwrap();
        assert_eq!(file.profiles["corp"], config, "{text}");
        let back = &file.routing.rules[0];
        assert!(back.bypass_mitm, "{text}");
        assert_eq!(back.host, rule.host);
    }

    #[test]
    fn rendering_is_deterministic() {
        let a = render_toml(&convert(BAK, &ImportOptions::default()).unwrap(), "x");
        let b = render_toml(&convert(BAK, &ImportOptions::default()).unwrap(), "x");
        assert_eq!(a, b);
    }
}
