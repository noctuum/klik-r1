# Klik-R1

Klik-R1 turns a Bluetooth remote into buttons for the Linux desktop. Its daemon, `klik`, reads the remote, tells a tap from a hold, and sends the key, the wheel movement or the click that the press is bound to.

It runs on Linux only, since it reads the remote through evdev and sends through uinput. Rust, edition 2024, rustc 1.98 or later.

## State

On 2026-10-22 the daemon knows one remote, the Beauty-R1, and one module of actions, `input`: a key or a chord of keys, the wheel, and a click of the left button. A configuration holds one profile of bindings, and the daemon refuses a second one. The profile has no calibration step. It uses the values measured on one unit, and the device section of the configuration can override each of them.

The daemon of this repository has not been run against the remote. What was measured on the remote itself, a Beauty-R1 described in [docs/beauty-r1.md](docs/beauty-r1.md):

| Date | Measured |
| --- | --- |
| 2026-08-24 | The signature of each of the six buttons, from a clean capture run |
| 2026-09-16 | The hold of each button: the arrows keep streaming, the camera holds `BTN_TOUCH` down, the center has no hold |
| 2026-09-17 | A second press of a motionless button, which carries no coordinates. Reconnects, with udev 15 to 51 ms behind the kernel. |
| 2026-09-24 | A bounce and a deliberate repeat, both 30 ms after a release |
| 2026-10-02 | The quickest repeat on record, 59 ms after a release |

The quiet window of 45 ms lies between the bounce and that quickest repeat. The rate of the wheel and its rise during a hold come from 2026-09-16 and 2026-09-24. Four of the captures are fixtures in `devices/beauty-r1/tests/fixtures`, and the tests replay them through the profile. `cargo test` needs no remote.

## Build and install

The build links `libudev` and finds it with `pkg-config`. On Arch Linux the library comes with `systemd-libs`.

The daemon opens the node of the remote under `/dev/input` and opens `/dev/uinput`, so the user who runs it needs access to both. On the machine it was written on, both belong to the group `input` and the user is a member. The group of `/dev/uinput` there comes from the udev rule of the `dotool` package.

Copy the example configuration, then put the Bluetooth address of the remote into `uniq`. `bluetoothctl devices` lists it.

```sh
mkdir -p ~/.config/klik
cp packaging/klik.example.toml ~/.config/klik/klik.toml
```

Link and enable the user service, then build and install:

```sh
systemctl --user enable "$PWD/packaging/klik.service"
packaging/update.sh
```

`systemctl --user enable` with a path links the unit into the unit directory of the user and enables it into `graphical-session.target`. `packaging/update.sh` builds the release binary, installs it as `~/.local/bin/klik`, reloads the units and restarts `klik.service`. `~/.local/bin` has to exist. The same script is the update after a pull.

Last, the udev rule:

```sh
~/.local/bin/klik install-udev
```

It prints a rule for each device of the configuration and the two commands that install it. It writes nothing itself. The rule matches the address and sets `LIBINPUT_IGNORE_DEVICE`, so the desktop leaves the remote alone even while no daemon holds it. Without the rule the desktop takes the remote for a touchscreen whenever the daemon does not hold it.

The service logs to the journal, read with `journalctl --user -u klik`. It always holds the start and the stop, the remote coming and going, and every failed action. With `[debug] enabled = true` in the configuration it also holds a line for every press.

## Configuration

`packaging/klik.example.toml` is a complete configuration with a comment on each part, and a test holds it to what the daemon accepts. A binding is keyed `"<device>.<button>:<kind>"`. The device is the name of its section under `[devices]`. The buttons of the Beauty-R1 are `up`, `down`, `left`, `right`, `center` and `camera`, and the kind is `tap` or `hold`. A hold exists for Left, Right and the camera. A held Up or Down is its tap followed by ticks, so a wheel bound to `up:tap` turns for as long as Up is held. A binding for `up:hold`, `down:hold` or `center:hold` is refused.

## Command line

| Command | What it does |
| --- | --- |
| `klik daemon [--config PATH]` | Reads the devices of the configuration and runs their bindings until `SIGINT` or `SIGTERM`. |
| `klik status` | Asks the running daemon for the active profile and the state of each configured device. |
| `klik events` | Prints a line for every press the running daemon handles, until stopped. |
| `klik replay FILE` | Feeds a recorded capture through the Beauty-R1 profile and prints what it resolves. |
| `klik record --uniq ADDRESS [--output FILE] [--no-grab]` | Writes the raw events of one remote as JSON Lines until Ctrl-C, to standard output without `--output`. |
| `klik install-udev [--config PATH]` | Prints the udev rule for the devices of the configuration and the commands that install it. |

`--config` defaults to `$XDG_CONFIG_HOME/klik/klik.toml`, and to `~/.config/klik/klik.toml` when the variable is not set.

`status` and `events` reach the daemon through the socket `$XDG_RUNTIME_DIR/klik/control`, which only its owner can open. A second daemon refuses to start while one answers on that socket. A line of `events` reads `<time> <device> <button> <kind>: came to <module>.<action>, <n> ms`, with the time in UTC. A press bound to nothing came to `nothing`. A press the quiet window refuses and a gesture that matches no button get a line as well.

`replay` needs no device. Over the clean capture run it prints:

```console
$ klik replay devices/beauty-r1/tests/fixtures/beauty-r1-clean-run.jsonl
press up tap
end up
press down tap
end down
press left tap
end left
press right tap
end right
press center tap
end center
press camera tap
```

`record` holds the remote while it records, so a held Left or Right does not change the volume of the system meanwhile. `--no-grab` leaves the remote to the desktop. At each connection it writes the position of the remote first, which a motionless first press needs in order to be identified on replay. It warns when the remote reconnects during a recording, since such a file can replay a press the remote never sent.

## License

MIT, in [LICENSE](LICENSE).
