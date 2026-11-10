"""Scroll the app window with a real mouse wheel event.

`app_input.py` covers click/drag/key but not the wheel, and the pages below the fold (the
inference card on the monitor page, the parameter card on the train page) can only be reached by
scrolling. Arrow keys are not a substitute: the command palette listens for them.

Usage:
    python spikes/qa/wheel.py -600 [out.png]     # negative = scroll down
"""

import ctypes
import sys
from ctypes import wintypes

from PIL import ImageGrab

user32 = ctypes.WinDLL("user32", use_last_error=True)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
TITLE = "YOLO Trainer"
MOUSEEVENTF_WHEEL = 0x0800


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", wintypes.LONG),
        ("dy", wintypes.LONG),
        ("mouseData", wintypes.DWORD),
        ("dwFlags", wintypes.DWORD),
        ("time", wintypes.DWORD),
        ("dwExtraInfo", ctypes.POINTER(wintypes.ULONG)),
    ]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("mi", MOUSEINPUT)]


def find_window() -> int | None:
    found: list[int] = []

    def callback(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        buffer = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(hwnd, buffer, 512)
        if buffer.value == TITLE:
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(EnumProc(callback), 0)
    return found[0] if found else None


def main() -> None:
    amount = int(sys.argv[1]) if len(sys.argv) > 1 else -600
    out = sys.argv[2] if len(sys.argv) > 2 else None

    hwnd = find_window()
    if not hwnd:
        raise SystemExit("window not found")
    user32.SetForegroundWindow(hwnd)
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    centre_x = (rect.left + rect.right) // 2
    centre_y = (rect.top + rect.bottom) // 2
    user32.SetCursorPos(centre_x, centre_y)

    # Split into a few notches: one giant delta is clamped by some window procedures, and the
    # page's own smooth scrolling reads better as several events.
    for _ in range(6):
        event = INPUT(
            type=0,
            mi=MOUSEINPUT(0, 0, ctypes.c_ulong(amount // 6).value, MOUSEEVENTF_WHEEL, 0, None),
        )
        user32.SendInput(1, ctypes.byref(event), ctypes.sizeof(INPUT))
        ctypes.windll.kernel32.Sleep(40)

    ctypes.windll.kernel32.Sleep(500)
    if out:
        image = ImageGrab.grab(
            bbox=(rect.left, rect.top, rect.right, rect.bottom), all_screens=True
        )
        image.save(out)
        print(f"saved {out} ({image.width}x{image.height})")


if __name__ == "__main__":
    main()
