# f-xoss: Free your Xoss device

A cloud-free reimplementation of Xoss companion app

## Support

For now, this was only tested with Xoss Nav bike computer. Other Xoss devices seem to use the same protocol, however I haven't yet looked into supporting them.

## Features

This app allows you to:

- Download workout data from your device
- Get Satellite GPS assistance data (using u-blox AssistNow Offline API)

TBD:

- Upload map data
- Upload tracks to device

## Usage

```
cargo install --git https://github.com/DCNick3/f-xoss.git
f-xoss-util dev sync
```

You can also install shell completion with

bash:

```
f-xoss-util completion bash > ~/.local/share/bash-completion/completions/f-xoss-util
```

fish:

```
f-xoss-util completion fish > ~/.config/fish/completions/f-xoss-util.fish
```
