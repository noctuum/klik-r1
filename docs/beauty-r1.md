# Beauty-R1

Findings from a capture session on 2026-08-24, on Arch Linux with `evtest` 1.36. The remote is a no-name Bluetooth shutter that ships as a bonus with camera tripods. There is no printed text on the case. It has six buttons: four arrows, one unlabeled center button, and one button with a camera icon.

The short version: the buttons do not send key codes. The device pretends to be a digitizer and replays a recorded swipe across a virtual 4096×4096 surface. Which button was pressed is encoded in the axis, the direction, and the start coordinate of that swipe.

## Identity

The remote pairs as `Beauty-R1` and shows up as a single kernel input device:

```text
I: Bus=0005 Vendor=05ac Product=022c Version=011b
N: Name="Beauty-R1"
P: Phys=xx:xx:xx:xx:xx:xx
S: Sysfs=/devices/virtual/misc/uhid/0005:05AC:022C.0002/input/input27
U: Uniq=xx:xx:xx:xx:xx:xx
H: Handlers=kbd event22 mouse3
B: PROP=2
B: EV=1f
B: REL=1040
B: ABS=10100000003
B: MSC=10
```

The two Bluetooth addresses in it are masked.

`Bus=0005` is Bluetooth. `Vendor=05ac` is Apple's vendor ID, which this device is not entitled to: the firmware declares a foreign VID/PID. `PROP=2` is `INPUT_PROP_DIRECT`, the property a touchscreen sets. The kernel binds it to three handlers at once, `kbd`, `event22` and `mouse3`, because the HID descriptor declares keys, relative axes and absolute axes together.

The device node was `/dev/input/event22` in that session. The number is assigned at connect time and is not stable across reconnects, so any consumer has to match on `Uniq` or on the name instead.

Permissions are `crw-rw---- root input`, and the user is in the `input` group, so reading the device does not need root.

## Declared capabilities versus what is actually sent

`evtest` prints 157 key and button codes for this device, including `KEY_UP`, `KEY_DOWN`, `KEY_LEFT`, `KEY_RIGHT`, `KEY_ENTER`, `KEY_ESC`, `KEY_CAMERA`, `KEY_VOLUMEUP`, `KEY_VOLUMEDOWN` and `KEY_PLAYPAUSE`, plus `REL_HWHEEL`, `ABS_VOLUME` and `ABS_MISC`.

None of them ever fire. Across four capture runs the device emitted exactly three event codes: `MSC_SCAN`, `BTN_TOUCH` and `BTN_TOOL_PEN`, along with `ABS_X` and `ABS_Y`. The capability list is a reference descriptor copied wholesale from the chip vendor and has no relation to what the buttons do.

`MSC_SCAN` carries the value `0xd0042` on every press without exception. That is HID usage page `0x0D` (Digitizers), usage `0x42` (Tip Switch): the code for a pen or a finger touching the surface.

## Button signatures

Both axes report a range of 0 to 4095. Coordinates below are from the clean capture run, kept as `devices/beauty-r1/tests/fixtures/beauty-r1-clean-run.jsonl`, where all six buttons were pressed once in order with no double presses.

| Button | Axis | From | To | Delta | Duration |
| --- | --- | --- | --- | --- | --- |
| Up | `ABS_Y` | 2036 | 3500 | +1464 | 270 ms |
| Down | `ABS_Y` | 2176 | 200 | -1976 | 268 ms |
| Left | `ABS_X` | 512 | 3350 | +2838 | 300 ms |
| Right | `ABS_X` | 3328 | 700 | -2628 | 300 ms |
| Center | none | (1904, 1904) | tap, no movement | 0 | 30 ms |
| Camera | none | (2048, 3424) | tap, no movement | 0 | 120 ms |

Two things in that table are worth reading twice. `ABS_Y` grows when the Up button is pressed, which is inverted relative to screen coordinates where Y grows downward. And the Left button swipes to the right in coordinate terms, while Right swipes to the left. Neither direction matches what the label on the button suggests, so a mapping written from intuition will come out mirrored.

The two taps are told apart only by their coordinate. They are otherwise identical in shape.

## Packet shape

A press produces a burst that looks like this, trimmed to one button:

```text
Event: time 1787594306.654895, type 4 (EV_MSC), code 4 (MSC_SCAN), value d0042
Event: time 1787594306.654895, type 1 (EV_KEY), code 330 (BTN_TOUCH), value 1
Event: time 1787594306.654895, type 1 (EV_KEY), code 320 (BTN_TOOL_PEN), value 1
Event: time 1787594306.654895, type 3 (EV_ABS), code 1 (ABS_Y), value 2176
Event: time 1787594306.683259, type 3 (EV_ABS), code 1 (ABS_Y), value 2088
...
Event: time 1787594306.893266, type 3 (EV_ABS), code 1 (ABS_Y), value 500
Event: time 1787594306.923271, type 4 (EV_MSC), code 4 (MSC_SCAN), value d0042
Event: time 1787594306.923271, type 1 (EV_KEY), code 330 (BTN_TOUCH), value 0
Event: time 1787594306.923271, type 1 (EV_KEY), code 320 (BTN_TOOL_PEN), value 0
Event: time 1787594306.923283, type 3 (EV_ABS), code 1 (ABS_Y), value 200
```

The last coordinate arrives after `BTN_TOUCH` goes to 0, inside the same report. A reader that treats `BTN_TOUCH = 0` as the end of the gesture drops the final position and loses part of the delta. The gesture ends at the `SYN_REPORT` that follows the release, not at the release itself.

Intermediate positions arrive every 30 ms, in steps of 300 units for most of the travel. A swipe opens with several smaller steps before it settles into that rhythm: Up moves 88, 44, 44, 44, 44 and only then 300 per step. The Right button additionally starts with one step in the wrong direction, `3328 → 3400`, before it moves left.

## The gestures are replayed, not sampled

The same button produces the same numbers every time. Left starts at `ABS_X` 512 in every run, Right starts at 3328, Up starts at `ABS_Y` 2036, Down at 2176, and the intermediate steps land on the same values in the same order. Four capture runs spread over 17 minutes are identical on this.

This is not a digitizer reporting a real finger. The firmware holds a canned coordinate sequence per button and plays it back. That is useful: a detector can key on the start coordinate alone and does not need to integrate the whole swipe.

## Behavior on the iPhone

Reported, not measured: paired to an iPhone, the remote moves the volume in VLC without any setup, but erratically, drifting up and then down instead of stepping. The other buttons do nothing useful.

The erratic part is consistent with the signature found here. The device sends a swipe and not a discrete key, so a dropped BLE packet or a partial replay moves the target by an arbitrary amount rather than by one step. What the remote actually triggers on iOS was not established.

The test also ran while the remote was paired to the laptop, and whether it held a connection to the phone at the same time was not checked.

## The micro-USB port

Plugging a cable into the micro-USB port lights a red LED and adds no new device to `lsusb`. Nothing enumerates, so the port serves charging. Whether the data lines are physically absent or merely unused was not determined.

This was not cross-checked against the kernel log. `dmesg` is restricted to root on this machine (`kernel.dmesg_restrict`), and `sudo dmesg -T | grep -i usb` with the cable plugged in has not been run.

Reflashing was raised and dropped. The programming interface on this class of chip is SWD or a proprietary single wire debug line on bare pads inside the case, not USB, and which chip is in there cannot be known without opening the device.

## Open questions

- Whether `/dev/input/by-id/` carries a stable symlink for this device. It was listed during the session but the output was not kept, so the matching strategy for a reconnect is still unverified.
- Whether the second remote is a plain HID keyboard. It is a similar stick from another tripod, with only plus, minus and a camera button, and it has never been captured. If it sends `KEY_VOLUMEUP` and `KEY_VOLUMEDOWN` it needs no software at all, so it is worth capturing before anything gets written for the Beauty-R1.
- Whether the phantom first cluster repeats. One extra cluster appeared at the start of the first run after connecting, and it was not reproduced in the three later runs.
- Whether the remote can hold two hosts at the same time.
- What VLC on iOS does with this gesture, and which of its gestures the remote lands on.
