# `websocket` tool

> publishes events into a websocket as JSON

A peer-observer tool that sends out all events on a websocket. Can be used to
visualize the events in the browser. The `www/*.html` files implement a few
visualizations.

## WebSocket Picker Configuration

The HTML pages (e.g., `peers.html`, `blocks.html`) include a websocket picker that allows users to connect from one web page to multiple websocket tools. This picker loads available WebSocket servers from a `websockets.json` file and presents them as radio buttons for easy switching.

### websockets.json Format

The `websockets.json` file should contain a JSON object mapping display names to WebSocket URLs:

```json
{
  "display-name": "websocket-url"
}
```

### Examples

#### Development Setup

For local development, create a simple `websockets.json` file:

```json
{
  "dev": "ws://127.0.0.1:47482"
}
```

#### Multiple Nodes Setup

For monitoring multiple nodes, you can specify multiple endpoints:

```json
{
  "alice": "/websocket/alice/",
  "bob": "/websocket/bob/",
  "charlie": "/websocket/charlie/"
}
```

#### Production Multi-Node Setup

For monitoring multiple Bitcoin nodes, you can set up nginx as a reverse proxy to route different websocket endpoints:

```json
{
  "alice": "/websocket/alice/",
  "bob": "/websocket/bob/",
  "charlie": "/websocket/charlie/"
}
```

This configuration allows one web interface to switch between monitoring different nodes.

#### Single Production Node

For a single production deployment with TLS:

```json
{
  "prod": "wss://mycustompeer.observer/websocket"
}
```

### Serving websockets.json

The `websockets.json` file needs to be served from the same path as your HTML files. You can either:

#### Static File Approach
Place the file in your `www/` directory alongside the HTML files and serve it normally. This works for most deployments.

#### Nginx Dynamic Configuration
For production deployments where you want to manage the configuration through nginx (useful for multi-node setups):

```nginx
# WebSocket proxy routing (for multi-node setup)
location /websocket/alice/ {
    proxy_pass http://127.0.0.1:47481/;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
}

location /websocket/bob/ {
    proxy_pass http://127.0.0.1:47482/;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
}

# Serve websockets.json dynamically
location /websockets.json {
    add_header Content-Type application/json;
    add_header Access-Control-Allow-Origin *;
    return 200 '{
        "alice": "/websocket/alice/",
        "bob": "/websocket/bob/",
        "charlie": "/websocket/charlie/"
    }';
}
```

This approach is useful when:
- Running multiple websocket tool instances (one per Bitcoin node)
- Managing configuration through deployment scripts rather than static files
- Need nginx for TLS termination and load balancing

### Important Notes

- The websocket picker requires `websockets.json` to be accessible from the same origin as your HTML files
- If `websockets.json` is not found or returns an error, the picker will display an empty selection
- For development, make sure to add `websockets.json` to your `.gitignore` to avoid committing local configurations

## Example

For example, connect to a NATS server on 128.0.0.1:1234 and start the websocket server on 127.0.0.1:4848:

```
$ cargo run --bin websocket -- --nats-address 127.0.0.1:1234 --websocket-address 127.0.0.1:4848

or

$ ./target/[release/debug]/websocket -n 127.0.0.1:1234 --websocket-address 127.0.0.1:4848
```

## Usage

```
$ cargo run --bin websocket -- --help
A peer-observer tool that sends out all events on a websocket

Usage: websocket [OPTIONS]

Options:
  -n, --nats-address <NATS_ADDRESS>
          The NATS server address the tool should connect and subscribe to [default: 127.0.0.1:4222]
  -w, --websocket-address <WEBSOCKET_ADDRESS>
          The websocket address the tool listens on [default: 127.0.0.1:47482]
  -l, --log-level <LOG_LEVEL>
          The log level the took should run with. Valid log levels are "trace", "debug", "info", "warn", "error". See https://docs.rs/log/latest/log/enum.Level.html [default: DEBUG]
  -h, --help
          Print help
  -V, --version
          Print version
```
