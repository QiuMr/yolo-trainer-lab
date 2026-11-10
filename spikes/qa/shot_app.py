"""Capture the YOLO Trainer window specifically.

`shot_window.py` matches on the window class "Tauri Window", which is also the class of the
WorkBuddy host window — so it happily captured the wrong app. This variant matches on the
window *title* instead, and optionally brings it to the front first (a background window can
be captured, but its DWM shadow is not composited into the client area the same way).

Run:  python spikes/qa/shot_app.py out.png
"""

import ctypes
import sys
from ctypes import wintypes

from PIL import ImageGrab

user32 = ctypes.WinDLL("user32", use_last_error=True)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

TITLE = "YOLO Trainer"


def find_window() -> int | None:
    found: list[int] = []

    def callback(hwnd, _):
        if not user32.IsWindowVisible(hwnd):
            return True
        buf = ctypes.create_unicode_buffer(512)
        user32.GetWindowTextW(hwnd, buf, 512)
        if buf.value.strip() == TITLE:
            found.append(hwnd)
            return False
        return True

    user32.EnumWindows(EnumProc(callback), 0)
    return found[0] if found else None


def rect_of(hwnd: int) -> tuple[int, int, int, int]:
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    return rect.left, rect.top, rect.right, rect.bottom


def main() -> None:
    out = sys.argv[1] if len(sys.argv) > 1 else "app.png"
    hwnd = find_window()
    if hwnd is None:
        raise SystemExit(f"window titled {TITLE!r} not found")

    user32.SetForegroundWindow(hwnd)
    user32.SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0040)  # NOSIZE|NOMOVE|SHOWWINDOW
    left, top, right, bottom = rect_of(hwnd)

    img = ImageGrab.grab(bbox=(left, top, right, bottom), all_screens=True)
    img.save(out)
    print(f"hwnd={hwnd} rect={(left, top, right, bottom)} saved {out} ({img.width}x{img.height})")


if __name__ == "__main__":
    main()
