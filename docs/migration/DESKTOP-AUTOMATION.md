# Desktop input diagnosis and project helpers

2026-11-05. This supersedes the earlier native evidence's input blocker.

The mouse is not confined to one screen. All three outputs use scale 1, with
logical rectangles DP-1 `0,0 2560x1440`, HDMI-A-1 `2560,0 3840x2160`, and DP-2
`6400,0 2560x1440`.

## Causes

1. The ydotool user service had been stopped after the previous verification.
   It was restarted and is now active; it was already enabled.
2. Flat acceleration does not remove sensitivity gain. Hyprland reports
   `input:accel_profile = flat`, `input:sensitivity = 0.35`. A direct ydotool
   absolute request for `(1000,600)` landed at `(1349,809)`: approximately 1.35
   times the requested position. The old move helper sent unadjusted coordinates
   and its 60 small corrections could not recover from the resulting large miss.
3. The installed ydotool key command documents raw numeric events. Symbolic
   values such as `tab` merely cause a delay. The old key helper forwarded those
   strings, so its successful exit did not mean a key had been pressed.

## Correction

Project-local helpers in `tools/computer-use/`:

- `pc-move.sh`: divide requested coordinates by the live sensitivity gain,
  require flat acceleration, reject coordinates outside the outputs, and verify
  the observed pointer position within one pixel. No click on a failed landing.
- `pc-click.sh`: use verified movement before sending the mouse button.
- `pc-key.sh`: translate key names/chords into numeric Linux keycode press and
  release events using the explicit installed input-event header.
- `pc-screenshot.sh`: force scale 1, include the cursor, and require captures
  to stay inside this checkout. Optional second argument is grim geometry.

No global skill file, display configuration, pointer sensitivity or acceleration
setting was changed. Existing typing helper remains usable. These are operational
automation tools, not a test suite. Their syntax was checked with `bash -n`.

## Observed results

| Screen | Requested pointer | Observed pointer | Screenshot |
|---|---|---|---|
| Left | 1280,720 | 1279,719 | artifacts/screenshots/pointer-left.png |
| Center | 4480,1080 | 4480,1079 | artifacts/screenshots/pointer-center.png |
| Right | 7680,720 | 7680,719 | artifacts/screenshots/pointer-right.png |

The Launcher was opened with the disposable `native-data` directory. A verified
click in its filter field followed by typing displayed `pointer-check`.
`Ctrl+A` and Backspace cleared it. Captures: `pointer-app-typed.png` and
`pointer-app-cleared.png`. A click on Add existing folder changed the UI to
Choosing folders, captured in `native-add-folder-click.png`; the native picker
itself did not appear. That separate picker/portal issue remains unverified.
The File menu and its Quit item were also clicked successfully; the Launcher
then exited with code 0. The ydotool service remains active for subsequent work.

No production project data was used, no Nuke/AE application was launched, and
no test suite/global tests ran. The helpers are verified on the current flat,
three-output setup; adaptive acceleration is deliberately refused.
