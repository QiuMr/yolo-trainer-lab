"""Click a point inside the running app window and screenshot the result.

Needed because nav clicks are the only way to reach a page in the packaged build: there is no
router query to set from outside, and the file dialog blocks automation.

Run:  python spikes/qa/click_and_shot.py <dx> <dy> <out.png>
      dx, dy are offsets from the window's top-left corner, in window pixels.
"""

import ctypes
import sys
import time
from ctypes import wintypes

from PIL import ImageGrab

user32 = ctypes.WinDLL("user32", use_last_error=True)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
MAIN_CLASS = "Tauri Window"


def main_window() -> int:
    found = []

    def callback(hwnd, _):
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        if cls.value == MAIN_CLASS:
            found.append(hwnd)
        return True

    user32.EnumWindows(EnumProc(callback), 0)
    if not found:
        raise SystemExit("no window found")
    return found[0]


def main() -> None:
    dx, dy = int(sys.argv[1]), int(sys.argv[2])
    out = sys.argv[3]

    user32.SetProcessDPIAware()
    hwnd = main_window()
    user32.ShowWindow(hwnd, 9)
    user32.SetForegroundWindow(hwnd)
    time.sleep(0.6)

    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    x, y = rect.left + dx, rect.top + dy
    print(f"window=({rect.left},{rect.top}) size={rect.right - rect.left}x{rect.bottom - rect.top}")
    print(f"clicking ({x},{y})")

    user32.SetCursorPos(x, y)
    time.sleep(0.2)
    user32.mouse_event(0x0002, 0, 0, 0, 0)
    user32.mouse_event(0x0004, 0, 0, 0, 0)
    time.sleep(1.4)

    ImageGrab.grab(
        bbox=(rect.left, rect.top, rect.right, rect.bottom), all_screens=True
    ).save(out)
    print(f"saved {out}")


if __name__ == "__main__":
    main()
