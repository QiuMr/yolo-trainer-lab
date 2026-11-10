"""Inspect and drive the running desktop app window from outside.

Used to verify what a UI screenshot cannot show: whether the main window took the geometry
stored in the window-state file, whether it is still visible, and what really happens to the
process when the close button is pressed.

The window is located by class name, not by title: the single-instance plugin also creates a
top-level window titled after the app, so a title lookup can grab that 16x16 helper instead
of the real frame (this happened, and produced a bogus 136x39 reading).

Run:  python spikes/qa/window_probe.py rect
      python spikes/qa/window_probe.py close
      python spikes/qa/window_probe.py process
"""

import ctypes
import sys
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
EnumProc = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

MAIN_CLASS = "Tauri Window"


def find_main() -> tuple[int, int]:
    """Return (hwnd, pid) of the real application window."""
    found: list[tuple[int, int]] = []

    def callback(hwnd, _):
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        if cls.value == MAIN_CLASS:
            pid = wintypes.DWORD()
            user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            found.append((hwnd, pid.value))
        return True

    user32.EnumWindows(EnumProc(callback), 0)
    if not found:
        raise SystemExit(f"no window of class {MAIN_CLASS!r}")
    return found[0]


def rect() -> str:
    hwnd, pid = find_main()
    r = wintypes.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    return (
        f"hwnd={hwnd} pid={pid} visible={bool(user32.IsWindowVisible(hwnd))} "
        f"pos=({r.left},{r.top}) size={r.right - r.left}x{r.bottom - r.top}"
    )


def close() -> str:
    hwnd, _ = find_main()
    # WM_CLOSE: exactly what the window's X button sends.
    user32.PostMessageW(hwnd, 0x0010, 0, 0)
    return f"posted WM_CLOSE to hwnd={hwnd}"


def process() -> str:
    _, pid = find_main()
    # Alive check: a handle we can open means the process object still exists.
    handle = kernel32.OpenProcess(0x1000, False, pid)
    if not handle:
        return f"pid={pid} gone"
    code = wintypes.DWORD()
    kernel32.GetExitCodeProcess(handle, ctypes.byref(code))
    kernel32.CloseHandle(handle)
    state = "alive (STILL_ACTIVE)" if code.value == 259 else f"exited code={code.value}"
    return f"pid={pid} {state}"


if __name__ == "__main__":
    action = sys.argv[1] if len(sys.argv) > 1 else "rect"
    print({"rect": rect, "close": close, "process": process}[action]())
