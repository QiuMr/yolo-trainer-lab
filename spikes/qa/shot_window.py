"""Screenshot just the app window (or the whole screen with --screen).

Written as a file rather than an inline -c snippet because the ctypes EnumWindows callback needs
real function types; a sloppy inline version silently returned a wrong rect and produced a capture
of the desktop instead of the window.

Run:  python spikes/qa/shot_window.py out.png [--screen]
"""

import ctypes
import sys
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
    out = sys.argv[1]
    user32.SetProcessDPIAware()

    if "--screen" in sys.argv:
        screen = ImageGrab.grab(all_screens=True)
        screen.save(out)
        print(f"saved {out} ({screen.size[0]}x{screen.size[1]})")
        return

    hwnd = main_window()
    rect = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(rect))
    box = (rect.left, rect.top, rect.right, rect.bottom)
    print(f"window hwnd={hwnd} rect={box}")
    ImageGrab.grab(bbox=box, all_screens=True).save(out)
    print(
        f"saved {out} ({rect.right - rect.left}x{rect.bottom - rect.top})"
    )


if __name__ == "__main__":
    main()
