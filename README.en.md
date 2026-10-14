# Argos (English summary)

A personal assistant on Telegram, written in Rust. It handles a Google Calendar, nags you about tasks until you mark them done, keeps a long-term memory and tracks vehicle maintenance. It also logs fuel-ups from a photo of the pump and the odometer. It runs in **~4–6 MB of RAM** on a tiny VM. The language model is Anthropic's Claude, used through your own API key.

**The bot, its prompts and its docs are in Brazilian Portuguese**, and some optional features rely on Brazilian public data: INMET weather alerts and the fuel prices from ANP.

Quick start:

```bash
cargo build --release
mkdir ~/argos-me && cd ~/argos-me
ARGOS_DIR=$PWD argos --init      # asks your name and the two keys (BotFather token, Anthropic API key)
ARGOS_DIR=$PWD argos             # prints a 6-digit code: send it to your bot in a private chat to pair
```

Optional pieces:
- `argos --veiculo moto|carro "Name" year` adds a vehicle.
- `argos --google-login` connects Google Calendar. Use your own OAuth "Desktop app" client, and publish the app to production so the refresh token doesn't expire after 7 days.
- Every other setting is optional; see `exemplos/configs.exemplo.json`. `argos --checar-config` validates the file.

License: [FSL-1.1-ALv2](LICENSE.md). You may use, modify and self-host it, but not offer it as a competing service. Each version becomes Apache-2.0 two years after release. Issues are welcome; pull requests only with a prior copyright assignment.
