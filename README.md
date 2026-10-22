# Klik-R1

Klik-R1 turns a Bluetooth remote into buttons for the Linux desktop. Its daemon, `klik`, reads the remote, tells a tap from a hold, and runs what the press is bound to: a key, the wheel, a click, a switch of virtual desktop, a command.

It runs on Linux only, since it reads the remote through evdev and sends through uinput. Rust, edition 2024, rustc 1.98 or later.

## State

On 2026-10-22 the daemon knows one remote, the Beauty-R1, and four modules of actions: `input` for keys, the wheel and a click; `kwin` for virtual desktops and windows on KDE Plasma; `script` for commands; `profile` for switching profiles by hand. A configuration holds any number of profiles of bindings, and rules over the window in front choose between them. Three presets come with the binary. The profile of the remote has no calibration step. It uses the values measured on one unit, and the device section of the configuration can override each of them.

The daemon of this repository has not been run against the remote. The `kwin` module was tried against the desktop with read-only questions alone: the window in front, the screen, the list of desktops. What was measured on the remote itself, a Beauty-R1 described in [docs/beauty-r1.md](docs/beauty-r1.md):

| Date | Measured |
| --- | --- |
| 2026-08-24 | The signature of each of the six buttons, from a clean capture run |
| 2026-09-16 | The hold of each button: the arrows keep streaming, the camera holds `BTN_TOUCH` down, the center has no hold |
| 2026-09-17 | A second press of a motionless button, which carries no coordinates. Reconnects, with udev 15 to 51 ms behind the kernel. |
| 2026-09-24 | A bounce and a deliberate repeat, both 30 ms after a release |
| 2026-10-02 | The quickest repeat on record, 59 ms after a release |

The quiet window of 45 ms lies between the bounce and that quickest repeat. The rate of the wheel and its rise during a hold come from 2026-09-16 and 2026-09-24. Four of the captures are fixtures in `devices/beauty-r1/tests/fixtures`, and the tests replay them through the profile. `cargo test` needs no remote.

## Build and install

The build links `libudev` and finds it with `pkg-config`. On Arch Linux the library comes with `systemd-libs`. The modules `kwin` and `script` are Cargo features, both on by default; `cargo build --release --no-default-features` leaves them out. The `kwin` module talks to KWin over the session bus and runs `kdotool` and `kscreen-doctor`, two seconds at most each.

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

The service logs to the journal, read with `journalctl --user -u klik`. It always holds the start and the stop, the remote coming and going, every switch of the profile, every failed action, and the `kwin` module off when KWin is out of reach at startup. With `[debug] enabled = true` in the configuration it also holds a line for every press.

## Configuration

`packaging/klik.example.toml` is a complete configuration with a comment on each part, and tests hold it to what the daemon accepts, the rules it shows in comments included. A binding is keyed `"<device>.<button>:<kind>"`. The device is the name of its section under `[devices]`. The buttons of the Beauty-R1 are `up`, `down`, `left`, `right`, `center` and `camera`, and the kind is `tap` or `hold`. A hold exists for Left, Right and the camera. A held Up or Down is its tap followed by ticks, so a wheel bound to `up:tap` turns for as long as Up is held. A binding for `up:hold`, `down:hold` or `center:hold` is refused.

### Profiles and rules

Several profiles need `[[rules]]` to choose between them:

```toml
[[rules]]
when = { window.class = "codium" }
use = "reading"

[[rules]]
use = "media"
```

The first rule whose conditions all hold names the active profile, and the last rule has no condition and names the default. A condition pairs a fact with a value. A value with `*` or `?` is a pattern, `*` for any run of characters and `?` for one, and case counts. A fact with no value, such as the class of a window when none is in front, matches nothing. The `kwin` module gives three facts: `window.class`, `window.title` and `desktop.current`, the name of the desktop in view. They are asked at the press of a button, and only while the rules decide. A profile can extend another with `extends = "<profile>"`, and a press it does not bind is looked up in its parent.

Four actions switch the profile by hand. `profile.set` keeps a profile until the next switch, `profile.next` takes the next one in the order of the file, and `profile.auto` hands the choice back to the rules. `profile.hold` makes a profile active while its button is held, and binds a hold only; it ends when the remote goes away as well. A held profile wins over one set by hand, and both win over the rules. A press keeps the profile it was looked up in until it ends, so its ticks and its end stay with the bindings of its press.

### Actions

| Action | Parameters | What it does |
| --- | --- | --- |
| `input.key` | `key` | Taps a key, such as `KEY_1`, or a chord such as `KEY_LEFTCTRL+KEY_A`. A media key is a key, `KEY_PLAYPAUSE`. |
| `input.wheel` | `direction`, `aim` | Turns the wheel `up` or `down`, faster the longer the button is held. With the `kwin` module the cursor is first put on the window in front, unless `aim = false`. |
| `input.click` | | Clicks the left button. |
| `kwin.desktop.next`, `kwin.desktop.prev` | | Switches to the next or the previous virtual desktop. |
| `kwin.window.activate` | `class` or `title` | Activates the first window whose class or caption is exactly that, on whichever desktop, and fails when it does not take the focus. |
| `script.run` | `command`, `wait`, `limit_ms` | Runs `sh -c <command>` at the press. |
| `profile.set`, `profile.hold` | `name` | See above. |
| `profile.next`, `profile.auto` | | See above. |

`script.run` gives the command `KLIK_DEVICE`, `KLIK_BUTTON`, `KLIK_KIND` (`tap` or `hold`) and `KLIK_PROFILE` in its environment. It does not wait for the command unless `wait = true`. A command still running after `limit_ms`, 10000 by default, is killed and named in the journal; a program the command starts in the background, with `&` or `setsid -f`, is left running. What the command prints goes to the journal.

### Presets

`klik preset list` names the three presets: `reading` scrolls and switches desktops, `media` handles the volume, play and tracks, `presentation` sends Page Up and Page Down. `klik preset apply <name>` adds the profile of a preset to the end of the configuration, for editing, and refuses a profile that exists. The presets bind a device named `beauty-r1`.

## Command line

| Command | What it does |
| --- | --- |
| `klik daemon [--config PATH]` | Reads the devices of the configuration and runs their bindings until `SIGINT` or `SIGTERM`. |
| `klik status` | Asks the running daemon for the active profile, why it is active, and the state of each configured device. |
| `klik events` | Prints a line for every press the running daemon handles, until stopped. |
| `klik profile set NAME`, `klik profile next`, `klik profile auto` | Switches the profile of the running daemon, as the profile actions do. |
| `klik profiles` | Lists the profiles in the order of the file, the active one marked with `*`, and why it is active. |
| `klik facts [--wait SECONDS]` | Prints the facts the modules give now, after waiting that many seconds. |
| `klik preset list`, `klik preset apply NAME [--config PATH]` | Lists the presets, or adds one to the configuration. |
| `klik replay FILE` | Feeds a recorded capture through the Beauty-R1 profile and prints what it resolves. |
| `klik record --uniq ADDRESS [--output FILE] [--no-grab]` | Writes the raw events of one remote as JSON Lines until Ctrl-C, to standard output without `--output`. |
| `klik install-udev [--config PATH]` | Prints the udev rule for the devices of the configuration and the commands that install it. |

`--config` defaults to `$XDG_CONFIG_HOME/klik/klik.toml`, and to `~/.config/klik/klik.toml` when the variable is not set.

`status`, `events`, `profile`, `profiles` and `facts` reach the daemon through the socket `$XDG_RUNTIME_DIR/klik/control`, which only its owner can open. A second daemon refuses to start while one answers on that socket. A line of `events` reads `<time> <device> <button> <kind>: came to <module>.<action> in <profile>, <n> ms`, with the time in UTC. A press bound to nothing came to `nothing`. A press the quiet window refuses and a gesture that matches no button get a line as well.

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
