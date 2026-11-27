# TODO

- **Publishing to crates.io.** Consumers depend on the engine through a pinned git tag
  (`tag = "vX.Y.Z"`). Publishing to crates.io would replace those git dependencies; decide
  whether to publish or keep git tags.
- **Breaking cleanup (next major / `v0.6.0` + CLI `v3.0.0`):**
  - Drop the unused `socks_block_web: bool` parameter from `EngineHandlers::with_mitm` (every
    caller already passes `false` and uses `with_socks_block_web()` directly).
  - Optionally rename the `blocked_web_in_mitm` SOCKS access-log status (`web_guard::BLOCKED_STATUS`)
    to `blocked_web` since SOCKS web blocking is now standalone; breaking for log parsers, so
    pair with the next major CLI release (`v3.0.0`).
