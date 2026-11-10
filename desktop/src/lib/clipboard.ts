/** Copy without assuming the Clipboard API works.
 *
 * `navigator.clipboard` needs a secure context. The packaged app serves its UI from
 * `http://tauri.localhost`, which WebView2 does not always treat as secure, so the call can reject
 * with a permissions error that looks exactly like "the button does nothing". The textarea fallback
 * is the only path that works in both the webview and a plain browser tab.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    /* fall through */
  }
  try {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-1000px";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  } catch {
    return false;
  }
}
