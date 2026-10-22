Standalone Usage

```bash
cargo run --release -p arztsprech-client --features cuda
```

Server Only

```bash
cargo run --release -p arztsprech-server --features cuda
```

Client Only

```bash
$env:ARZTSPRECH_SERVER="127.0.0.1:7878"
cargo run --release -p arztsprech-client --no-default-features --features remote
```