# f-xoss: Free your Xoss device

A cloud-free reimplementation of Xoss companion app

## Support

For now, this was only tested with Xoss Nav bike computer. Other Xoss bike computers seem to use the same protocol, however I haven't yet looked into supporting them.

## Features

This app allows you to:

- Download workout data from your device (in Garmin FIT format)
- Get Satellite GPS assistance data (using u-blox AssistNow Offline API)

Not yet implemented:

- Upload map data to device
- Upload tracks to device
- Editing device settings
- Customizing panel layouts

## Usage

#### 1. Install the app

You can use a bash installer on linux or macOS:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/DCNick3/f-xoss/releases/latest/download/f-xoss-util-installer.sh | sh
```

And a powershell installer on Windows:

```powershell
irm https://github.com/DCNick3/f-xoss.git/releases/download/f-xoss-util-installer.ps1 | iex
```

Alternatively, `cargo binstall` can be used:

```bash
cargo binstall f-xoss-util
```

Finally, you can build it from source with `cargo install`:

```bash
cargo install f-xoss-util
```

#### 1.1. (Optional) You can also install shell completion with

bash:

```bash
f-xoss-util completion bash > ~/.local/share/bash-completion/completions/f-xoss-util
```

fish:

```bash
f-xoss-util completion fish > ~/.config/fish/completions/f-xoss-util.fish
```

zsh:

```bash
f-xoss-util completion zsh > ~/.zsh/completions/_f-xoss-util
```

#### 2.0. (Linux-only) Adjust BLE connection supervision timeout 

If you are using Linux, you will need to increase BLE connection supervision timeout before pairing, as the default one is too low for the device and leads to timeouts during file transfers.

To change the timeout edit `/etc/bluetooth/main.conf` and add/uncomment the following line in the `[LE]` section:

```
[LE]
ConnectionSupervisionTimeout=400
```

Note that bluetoothd restart is NOT sufficient to apply the changes, you will need to reboot your system. Unloading all bluetooth-related kernel modules would work too, but there are a lot of dependent ones, so it's easier to just reboot.

#### 2.1. Pair with your device

You would use standard OS tools for this. To switch the Xoss Nav to the pairing mode you need to go to menu and select "Connect XOSS" menu item.

If you have paired with some other device it will show "XOSS#1234" instead. You would need to remove that pairing first by selecting the "Remove" option in the submenu.

#### 3. Generate a config file

Next, you should use `f-xoss-util setup` command to generate a config file.

First, it will ask you to select the XOSS device from all available bluetooth devices.

Then it will ask you for an u-blox AssistNow token used for updating satellite data. You can get one for free [here](https://www.u-blox.com/en/assistnow-service-registration-form). You can also just press enter to skip this step, but the satellite data will not be updated.

#### 4. Sync!

Now you can freely use `f-xoss-util dev sync` to regularly sync your device.

This will ensure the time is set correctly, the satellite data is updated and the workouts are downloaded.

The workouts will be saved in the data directory in Garmin FIT format.

You can use `f-xoss-util paths` to get the path to the data directory. 
