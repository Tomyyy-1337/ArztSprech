# Arztsprech

## Compile

Standalone Usage

```bash
cargo run --release -p arztsprech-client --features standalone
```

Server Only

```bash
cargo run --release -p arztsprech-server 
```

Client Only
 
```bash
cargo run --release -p arztsprech-client --features remote
```


## Trigger GitHub action

```bash
git tag v0.1.8
git push origin v0.1.8
```
