"""Drive the native folder picker with the keyboard.

The dataset page's only entry point is a Windows common file dialog, which automation cannot click
through. It can, however, be typed into: the dialog focuses its "folder" field, so sending the path
as unicode keystrokes is enough to select a directory.

Two things cost a debugging round and are worth keeping in mind:
  * `sizeof(INPUT)` must be 40 on x64. A union that only contains `KEYBDINPUT` makes it 32, and
    `SendInput` then rejects every event with a zero return value — silently typing nothing.
  * The caller should verify the return value. `SendInput` returning less than the number of events
    is the only signal that the injection failed.

Run:  python spikes/qa/type_path.py "E:\\path\\to\\folder"
"""

import ctypes
import sys

from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)

KEYEVENTF_UNICODE = 0x0004
KEYEVENTF_KEYUP = 0x0002


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", wintypes.LONG),
        ("dy", wintypes.LONG),
        ("mouseData", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong)),
    ]


class KEYBDINPUT(ctypes.Structure):
    _fields_ = [
        ("wVk", wintypes.WORD),
        ("wScan", wintypes.WORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong)),
    ]


class HARDWAREINPUT(ctypes.Structure):
    _fields_ = [
        ("uMsg", wintypes.DWORD),
        ("wParamL", wintypes.WORD),
        ("wParamH", wintypes.WORD),
    ]


class INPUT(ctypes.Structure):
    class _Union(ctypes.Union):
        _fields_ = [("mi", MOUSEINPUT), ("ki", KEYBDINPUT), ("hi", HARDWAREINPUT)]

    _anonymous_ = ("u",)
    _fields_ = [("type", wintypes.DWORD), ("u", _Union)]


def send_unicode(text: str) -> int:
    events = []
    for char in text:
        for flags in (KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP):
            event = INPUT()
            event.type = 1  # INPUT_KEYBOARD
            event.ki = KEYBDINPUT(0, ord(char), flags, 0, None)
            events.append(event)
    array = (INPUT * len(events))(*events)
    sent = user32.SendInput(len(events), array, ctypes.sizeof(INPUT))
    if sent != len(events):
        raise SystemExit(
            f"SendInput injected {sent}/{len(events)} events "
            f"(sizeof(INPUT)={ctypes.sizeof(INPUT)}, last error={ctypes.get_last_error()})"
        )
    return sent


def main() -> None:
    text = sys.argv[1]
    events = send_unicode(text)
    print(f"typed {len(text)} chars as {events} key events")


if __name__ == "__main__":
    main()
