//! Dependency-free routing benchmark for `Router::route`.
//!
//! Run with: `cargo run --release --example route_bench --locked`

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::hint::black_box;
use std::time::Instant;

use rusty_conduit_engine::router::{RouteRequest, Router};
use rusty_conduit_engine::routing::{Inbound, ProfileConfig, RoutingConfig, RoutingInput};
use serde::Deserialize;

const RULE_COUNTS: [usize; 3] = [100, 1000, 5000];
const REQUEST_COUNT: usize = 10_000;
const LCG_SEED: u64 = 42;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Host,
    Wildcard,
    Mixed,
    Url,
}

impl Kind {
    const ALL: [Self; 4] = [Self::Host, Self::Wildcard, Self::Mixed, Self::Url];

    fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Wildcard => "wildcard",
            Self::Mixed => "mixed",
            Self::Url => "url",
        }
    }
}

#[derive(Deserialize)]
struct BenchFile {
    profiles: BTreeMap<String, ProfileConfig>,
    routing: RoutingConfig,
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }
}

struct SampleRequest {
    host: String,
    url: Option<String>,
}

fn build_config_toml(n: usize, kind: Kind) -> String {
    let mut out = String::with_capacity(n * 64 + 128);
    out.push_str(
        "[profiles.p]\n\
         type = \"http\"\n\
         host = \"127.0.0.1\"\n\
         port = 3128\n\
         \n\
         [routing]\n\
         default = \"direct\"\n",
    );

    for i in 0..n {
        out.push_str("\n[[routing.rules]]\n");
        match kind {
            Kind::Host => {
                let _ = writeln!(out, "host = \"h{i}.example\"");
            }
            Kind::Wildcard => {
                let _ = writeln!(out, "host_wildcard = \"*.w{i}.example\"");
            }
            Kind::Mixed => match i % 4 {
                0 => {
                    let _ = writeln!(out, "host = \"h{i}.example\"");
                }
                1 => {
                    let _ = writeln!(out, "host_wildcard = \"*.w{i}.example\"");
                }
                2 => {
                    let _ = writeln!(out, "host_regex = \"^r{i}\\\\.\"");
                }
                _ => {
                    let a = (i / 256) % 256;
                    let b = i % 256;
                    let _ = writeln!(out, "cidr = \"10.{a}.{b}.0/24\"");
                }
            },
            Kind::Url => {
                let _ = writeln!(out, "url_wildcard = \"*://u{i}.example/*\"");
            }
        }
        out.push_str("target = \"p\"\n");
    }

    out
}

fn build_requests(n: usize, kind: Kind) -> Vec<SampleRequest> {
    let mut rng = Lcg::new(LCG_SEED);
    let mut requests = Vec::with_capacity(REQUEST_COUNT);

    for _ in 0..REQUEST_COUNT {
        let is_hit = (rng.next() >> 63) == 0;
        let k = ((rng.next() >> 32) as usize) % n;

        let (host, url) = if is_hit {
            match kind {
                Kind::Host => (format!("h{k}.example"), None),
                Kind::Wildcard => (format!("x.w{k}.example"), None),
                Kind::Mixed => match k % 4 {
                    0 => (format!("h{k}.example"), None),
                    1 => (format!("x.w{k}.example"), None),
                    2 => (format!("r{k}.example"), None),
                    _ => (format!("10.{}.{}.1", (k / 256) % 256, k % 256), None),
                },
                Kind::Url => (
                    format!("u{k}.example"),
                    Some(format!("http://u{k}.example/path")),
                ),
            }
        } else {
            let host = format!("miss{k}.other");
            let url = if kind == Kind::Url {
                Some(format!("http://{host}/path"))
            } else {
                None
            };
            (host, url)
        };

        requests.push(SampleRequest { host, url });
    }

    requests
}

fn run_pass(router: &Router, requests: &[SampleRequest]) -> usize {
    let mut hits = 0usize;
    for item in requests {
        let mut req = RouteRequest::new(Inbound::Http, &item.host, 80);
        if let Some(url) = item.url.as_deref() {
            req.scheme = Some("http");
            req.url = Some(url);
        }
        let decision = black_box(router.route(&req).expect("route should not be denied"));
        if decision.rule.is_some() {
            hits += 1;
        }
    }
    hits
}

fn main() {
    for n in RULE_COUNTS {
        for kind in Kind::ALL {
            let toml_text = build_config_toml(n, kind);
            let parsed: BenchFile =
                toml::from_str(&toml_text).expect("generated benchmark TOML must parse");
            let input = RoutingInput {
                profiles: parsed.profiles,
                routing: parsed.routing,
                ..RoutingInput::default()
            };

            let compile_start = Instant::now();
            let router = Router::from_input(&input).expect("generated routing config must compile");
            let compile_ms = compile_start.elapsed().as_millis();

            let requests = build_requests(n, kind);

            // Warm-up pass.
            let warmup_hits = run_pass(&router, &requests);

            // Timed pass.
            let start = Instant::now();
            let timed_hits = run_pass(&router, &requests);
            let elapsed_ns = start.elapsed().as_nanos();

            assert_eq!(warmup_hits, timed_hits);
            let ns_per_route = elapsed_ns / (requests.len() as u128);

            println!(
                "N={n} kind={} ns/route={ns_per_route} (rules compiled in {compile_ms} ms)",
                kind.as_str()
            );
        }
    }
}
