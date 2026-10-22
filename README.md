Standalone Usage

```bash
cargo run --release --features cuda
```

Server Only

```bash
cargo run --release --bin arztsprech-server --features cuda
```

Client Only

```bash
$env:ARZTSPRECH_SERVER="127.0.0.1:7878"
cargo run --release --no-default-features --features remote
```