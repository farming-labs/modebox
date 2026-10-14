# Benchmarks

Measured against Vercel Run, the closest package with the same model (QuickJS on wasm with host functions). Apple M-series, Node 24. `bench/compare.mjs` runs the identical script against both packages, with JS host functions on both sides.

| workload | Run 2.1.7 | modebox (Node) | |
|---|---:|---:|---:|
| trivial run | 3.57 ms | **0.12 ms** | 29× |
| one host call | 4.13 ms | **0.18 ms** | 23× |
| 200 sequential host calls in one run | 26.0 ms | **7.8 ms** | 3.3× |
| 8 parallel runs, per run | 1.48 ms | **0.06 ms** | 24× |
| return a 1 MB string | 531 ms | **11 ms** | 47× |
| host returns 5,000 rows | 62 ms | **23 ms** | 2.7× |
| interrupt + resume | 18.5 ms | **4.0 ms** | 4.7× |
| token after 200 host calls | 1.13 MB | **17 KB** | 66× smaller |
| resume after 200 host calls | 30.9 ms | **2.2 ms** | 14× |
| CPU: sum 1..10M | 488 ms | 511 ms | ≈ (same interpreter) |

These numbers are from modebox 0.1.0 (wasmtime 44), measured on 2026-10-10; 0.1.1 hasn't been re-measured yet. The two Node resume rows are too high: the benchmark's resume made the tool pause a second time, so they time two pauses instead of one. The Rust figures below time a single resume.

From Rust directly (`cargo run --release -p modebox-core --example bench`), a trivial run is 0.096 ms, interrupt + resume is 1.8 ms, and resume after 200 calls is 0.28 ms.

**Where it isn't faster:** CPU-bound JS. Both run the QuickJS bytecode interpreter compiled to wasm. Fuel metering costs about 10%; turn it off with `EngineConfig { metering: false }`. If heavy compute matters, a V8-isolate tier is the next step (see the [roadmap](../README.md#limitations-and-roadmap)).
