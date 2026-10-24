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

## Setup

Create a user list `server/certs/users.txt` (one `username:password` per line, `#` starts a comment):

```text
arzt:geheimesPasswort
assistent:anderesPasswort
```

Certificates get created automatically when starting the server for the first time if they do not already exist.

## Environment Variables
- `ARZTSPRECH_LISTEN` (default `0.0.0.0:7878`)