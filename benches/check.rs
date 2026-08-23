//! End-to-end benchmarks of the check pipeline: N tree/test file pairs run
//! through the same `runner::check_all` the CLI uses, across thread counts
//! and grammar backends (native vs sandboxed wasm).
//!
//! Run with:
//! ```console
//! cargo bench                    # native grammars
//! cargo bench --features wasm    # adds the sandboxed-grammar group
//!                                # (needs scripts/fetch-wasm-grammars.sh)
//! ```
//! Criterion stores baselines under target/criterion — use
//! `cargo bench -- --save-baseline <name>` / `--baseline <name>` to compare
//! runs across changes.

use btt::config::CheckConfig;
use btt::{pack, runner};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::path::{Path, PathBuf};
use std::time::Duration;

const FILES: usize = 40;
const JOBS: [usize; 3] = [1, 4, 8];

const TREE: &str = "\
HashMap
├── when the key is present
│   └── it returns the value
└── when the key is absent
    └── it returns none
";

const TEST_TS: &str = r#"
import { describe, it } from "vitest";
describe("HashMap", () => {
  describe("when the key is present", () => {
    it("returns the value", () => {});
  });
  describe("when the key is absent", () => {
    it("returns none", () => {});
  });
});
"#;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// Write N identical tree/test pairs under target/ and return the tree paths.
fn setup_fixtures() -> Vec<PathBuf> {
    let dir = repo_root().join("target/bench-fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    (0..FILES)
        .map(|i| {
            let tree = dir.join(format!("f{i}.tree"));
            std::fs::write(&tree, TREE).unwrap();
            std::fs::write(dir.join(format!("f{i}.test.ts")), TEST_TS).unwrap();
            tree
        })
        .collect()
}

fn bench_pack(c: &mut Criterion, group_name: &str, pack: pack::Pack, tree_files: &[PathBuf]) {
    let packs = vec![pack];

    // Steady state: the pool (and with it, per-thread grammar caches)
    // persists across iterations, so wasm grammar compilation is excluded.
    let mut group = c.benchmark_group(group_name);
    group
        .sample_size(20)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(3));
    for jobs in JOBS {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(jobs)
            .build()
            .unwrap();
        group.bench_with_input(BenchmarkId::new("jobs", jobs), &jobs, |b, _| {
            b.iter(|| {
                pool.install(|| runner::check_all(&packs, tree_files, CheckConfig::default()))
            });
        });
    }
    group.finish();

    // Cold start: a fresh pool per iteration kills the thread-local caches
    // with their threads, so wasm pays its per-thread Cranelift compile
    // every time — the cost the steady-state group deliberately excludes.
    let mut cold = c.benchmark_group(format!("{group_name}-cold"));
    cold.sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(5));
    for jobs in [1, 4] {
        cold.bench_with_input(BenchmarkId::new("jobs", jobs), &jobs, |b, &jobs| {
            b.iter(|| {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(jobs)
                    .build()
                    .unwrap();
                pool.install(|| runner::check_all(&packs, tree_files, CheckConfig::default()))
            });
        });
    }
    cold.finish();
}

fn benches(c: &mut Criterion) {
    let tree_files = setup_fixtures();

    bench_pack(
        c,
        "check/typescript-native",
        pack::load("typescript", repo_root()).unwrap(),
        &tree_files,
    );

    #[cfg(feature = "wasm")]
    {
        let dir = repo_root().join("packs-wasm/typescript");
        if dir.join("grammar.wasm").is_file() {
            bench_pack(
                c,
                "check/typescript-wasm",
                pack::load_dir(&dir).unwrap(),
                &tree_files,
            );
        } else {
            eprintln!("skipping check/typescript-wasm: run scripts/fetch-wasm-grammars.sh first");
        }
    }
}

criterion_group!(check, benches);
criterion_main!(check);
