benchmark:
    cargo build --release
    hyperfine "cargo run --release -- packs/simple"

flamegraph:
    cargo flamegraph --dev
    loupe flamegraph.svg