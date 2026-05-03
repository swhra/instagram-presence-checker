# instagram-presence-checker

A simple Rust program that reverse-engineers Instagram's DGW service _(Something Gateway, I suppose?)_ to poll for a specific user's current presence status, writing it to `/tmp/instagram.log` and stdout. This doubles up as a capable tool for maintaining one's own online status, since this is a necessary evil of obtaining other users' presence statuses. To do this we send the same 'foreground activity detected' and 'here are my additional contacts' pings that Instagram's web app sends, as well as the WebSocket-layer ping packet:

```rs
if elapsed > 5f32 {
    // IG pings
    socket.send(Message::Binary(additional_contacts_packet.clone().into()))?;
    socket.send(Message::Binary(foreground_packet.clone().into()))?;
    
    // WS ping
    socket.send(Message::Binary(vec![9].into()))?;

    // ...
}
```

## Setup

First, install Playwright. Off the top of my head I think this requires:

```
pip install playwright
playwright-cli install-browser firefox
```

If you don't have `pip`, just download Python from [here](https://www.python.org/downloads/macos/) and use the graphical installer, which will set up non-system Python and `pip`. (System Python on MacOS is Python 2, which is useless for most purposes, including ours.)

Run `python obtain_creds.py` once on your laptop to obtain your credentials. This will launch Firefox. If you don't use Firefox as your primary browser, it will take a moment to figure out that you aren't logged in to Instagram on Firefox (this requires launching Firefox once in headless mode, or more times if you have Firefox installed with several profiles), and then it will launch it in headed mode, letting you log in. Once you have logged in, it should close Firefox automatically. If it doesn't, leave it a while until it does, or else check for `credentials.json` in your current directory and try again if it is not there.

Once this is done, you can run the more efficient Rust code in the background indefinitely. To compile it, run `cargo build --release instagram`, then run `target/release/instagram`. That is all. If you want to set it up as a background service on MacOS (running silently whenever your laptop is switched on), run `make install`. This will use `launchctl` to set up a `launchd` service (MacOS's system for automatically managed background services). You can then just `tail -f /tmp/instagram.log` to watch the output and confirm that it's running ok.
