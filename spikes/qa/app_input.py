"""Drive the running app with real mouse/keyboard input and capture the result.

Why this exists: gestures cannot be verified from a headless render — a synthetic DOM event
has no `pointerType`, no pointer capture and no OS-level button state. `mouse_event` /
`keybd_event` go through the same path a human does, so drag-resize, hover tooltips and
Ctrl+K behave exactly as they will for the user.

Window is found by title, not class: the host WorkBuddy window is also class "Tauri Window",
so class matching silently captures the wrong app.

Usage (coordinates are offsets from the window's top-left, in window pixels):
    python spikes/qa/app_input.py click 60 116 --shot out.png
    python spikes/qa/app_input.py move 32 200 --shot out.png
    python spikes/qa/app_input.py drag 231 40 300 40 --shot out.png
    python spikes/qa/app_input.py key ctrl+k --shot out.png
    python spikes/qa/app_input.py shot out.png
"""

import ctypes
import sys
import time
from ctypes import wintypes

from PIL import ImageGrab

user32 = ctypes.WinDLL("user32", use_last_error=True)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
TITLE = "YOLO Trainer"

MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
KEYEVENTF_KEYUP = 0x0002

VK = {
    "ctrl": 0x11,
    "shift": 0x10,
    "alt": 0x12,
    "esc": 0x1B,
    "enter": 0x0D,
    "up": 0x26,
    "down": 0x28,
    "left": 0x25,
    "right": 0x27,
}


def find_window() -> int:
    found: list[int] = []

    def callback(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        buf = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(hwnd, buf, 512)
        if buf.value.strip() == TITLE:
            found.append(hwnd)
        return True

    user32.EnumWindows(EnumProc(callback), 0)
    if not found:
        raise SystemExit(f"window {TITLE!r} not found")
    return found[0]


def activate(hwnd: int) -> tuple[int, int]:
    user32.SetProcessDPIAware()
    user32.ShowWindow(hwnd, 9)  # SW_RESTORE
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.45)
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    return rect.left, rect.top


def shot(hwnd: int, path: str) -> None:
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    img = ImageGrab.grab(
        bbox=(rect.left, rect.top, rect.right, rect.bottom), all_screens=True
    )
    img.save(path)
    print(f"saved {path} ({img.width}x{img.height})")


def move(x: int, y: int) -> None:
    user32.SetCursorPos(x, y)
    time.sleep(0.05)


def click(x: int, y: int) -> None:
    move(x, y)
    time.sleep(0.08)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    time.sleep(0.06)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)


def drag(x1: int, y1: int, x2: int, y2: int) -> None:
    move(x1, y1)
    user32.mouse_event(MOUSEEVENTF_LEFTDOWN, 0, 0, 0, 0)
    time.sleep(0.12)
    # Intermediate steps: a single jump produces one mousemove, and a pointer-capture drag
    # that only ever sees the final position cannot be distinguished from a click.
    steps = 18
    for i in range(1, steps + 1):
        move(x1 + (x2 - x1) * i // steps, y1 + (y2 - y1) * i // steps)
        time.sleep(0.012)
    time.sleep(0.1)
    user32.mouse_event(MOUSEEVENTF_LEFTUP, 0, 0, 0, 0)


def press_combo(combo: str) -> None:
    keys = [part.strip().lower() for part in combo.split("+")]
    codes = []
    for key in keys:
        code = VK.get(key)
        if code is None:
            code = ord(key.upper())
        codes.append(code)
    for code in codes:
        user32.keybd_event(code, 0, 0, 0)
        time.sleep(0.03)
    for code in reversed(codes):
        user32.keybd_event(code, 0, KEYEVENTF_KEYUP, 0)
        time.sleep(0.03)


def main() -> None:
    args = sys.argv[1:]
    out = None
    if "--shot" in args:
        i = args.index("--shot")
        out = args[i + 1]
        args = args[:i] + args[i + 2:]

    hwnd = find_window()
    ox, oy = activate(hwnd)
    command = args[0]

    if command == "shot":
        shot(hwnd, args[1] if len(args) > 1 else "shot.png")
        return
    if command == "click":
        click(ox + int(args[1]), oy + int(args[2]))
    elif command == "dblclick":
        x, y = ox + int(args[1]), oy + int(args[2])
        click(x, y)
        time.sleep(0.06)
        click(x, y)
    elif command == "move":
        move(ox + int(args[1]), oy + int(args[2]))
    elif command == "drag":
        drag(
            ox + int(args[1]),
            oy + int(args[2]),
            ox + int(args[3]),
            oy + int(args[4]),
        )
    elif command == "key":
        press_combo(args[1])
    else:
        raise SystemExit(f"unknown command {command!r}")

    time.sleep(0.75)
    if out:
        shot(hwnd, out)


if __name__ == "__main__":
    main()
