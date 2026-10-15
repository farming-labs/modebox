# Benchmarks

Measured against Vercel Run, the closest package with the same model (QuickJS on wasm with host functions). `bench/compare.mjs` runs the identical script against both packages, with JS host functions on both sides.

| workload | Run 2.1.7 | modebox (Node) | |
|---|---:|---:|---:|
| trivial run | 5.85 ms | **0.23 ms** | 25× |
| one host call | 5.97 ms | **0.40 ms** | 15× |
| 200 sequential host calls in one run | 57.4 ms | **15.1 ms** | 3.8× |
| 8 parallel runs, per run | 2.82 ms | **0.17 ms** | 16× |
| return a 1 MB string | 862 ms | **16.5 ms** | 52× |
| host returns 5,000 rows | 129 ms | **32.7 ms** | 4.0× |
| interrupt + resume | 31.3 ms | **3.8 ms** | 8.3× |
| token after 200 host calls | 1.13 MB | **16 KB** | 71× smaller |
| resume after 200 host calls | 67.2 ms | **0.87 ms** | 78× |
| CPU: sum 1..10M | 716 ms | 683 ms | ≈ (same interpreter) |

Each figure is the median of 5 runs per package, alternating between them, measured with modebox 0.1.3 on 2026-10-15 by the [bench workflow](../.github/workflows/bench.yml) on a GitHub-hosted `macos-15` runner (Apple M1, 3 vCPUs) with Node 24 ([this run](https://github.com/farming-labs/modebox/actions/runs/38764276177)). Anyone with access can re-run it from the Actions tab. A laptop is faster in absolute terms: on an M-series MacBook, modebox 0.1.0 did a trivial run in 0.12 ms.

From Rust directly (`cargo run --release -p modebox-core --example bench`, one run in the same workflow), a trivial run is 0.23 ms, interrupt + resume is 6.6 ms, and resume after 200 calls is 0.67 ms.

**Where it isn't faster:** CPU-bound JS. Both run the QuickJS bytecode interpreter compiled to wasm. Fuel metering costs about 15% here (734 ms metered, 633 ms unmetered, from Rust); turn it off with `EngineConfig { metering: false }`. If heavy compute matters, a V8-isolate tier is the next step (see the [roadmap](../README.md#limitations-and-roadmap)).
