# Arztsprech

## Compile

Standalone Usage

```bash
cargo run --release -p arztsprech-client --features standalone,cuda
```

Server Only

```bash
cargo run --release -p arztsprech-server  --features cuda
```

Client Only
 
```bash
cargo run --release -p arztsprech-client --features remote
```


## Trigger GitHub action

```bash
git tag v0.1.11
git push origin v0.1.11
```
