# Arztsprech

## Compile

Standalone Usage

```bash
cargo run --release -p arztsprech-client --features standalone,cuda
```

Server Only

```bash
cargo run --release -p arztsprech-server --features cuda
```

Client Only
 
```bash
$env:ARZTSPRECH_SERVER="127.0.0.1:7878"
cargo run --release -p arztsprech-client --features remote
```


## Trigger GitHub action

```bash
git tag v0.1.3
git push origin v0.1.3
```
