"""Probe the public update feed without starting the app.

Why this exists: a Release can look perfectly fine on the GitHub page while the updater still
fails. The two silent killers we actually hit were (1) `gh` renaming an asset with a space, so
the URL in latest.json 404s, and (2) a signature whose `version:` field does not match the
announced version, which `requireSignedVersion` rejects at download time. Both are cheap to
check over plain HTTP.

    python spikes/qa/probe_update.py                  # latest Release
    python spikes/qa/probe_update.py --tag v0.1.1    # a specific Release
    python spikes/qa/probe_update.py --sig path/to/x.exe.sig   # also compare with a local build
"""

import argparse
import base64
import json
import re
import sys
import urllib.request

SLUG = "QiuMr/yolo-trainer-lab"
PLATFORM = "windows-x86_64"


def request(url: str, method: str = "GET") -> tuple[int, bytes, str]:
    req = urllib.request.Request(
        url, method=method, headers={"User-Agent": "yolo-trainer-update-probe"}
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        return response.status, response.read(), response.geturl()


checks: list[tuple[str, bool, str]] = []


def check(name: str, ok: bool, detail: str = "") -> None:
    checks.append((name, bool(ok), detail))
    print(f"{'PASS' if ok else 'FAIL'}  {name}" + (f"  — {detail}" if detail else ""))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="probe this tag instead of `releases/latest`")
    parser.add_argument("--sig", help="local .sig file to compare against the feed")
    args = parser.parse_args()

    base = (
        f"https://github.com/{SLUG}/releases/download/{args.tag}"
        if args.tag
        else f"https://github.com/{SLUG}/releases/latest/download"
    )
    feed_url = f"{base}/latest.json"

    status, body, final = request(feed_url)
    check("feed 匿名可读", status == 200, f"HTTP {status}")
    if status != 200:
        return 1
    check(
        "feed 无 BOM",
        not body.startswith(b"\xef\xbb\xbf"),
        "serde_json 遇到 BOM 会直接解析失败",
    )

    try:
        manifest = json.loads(body.decode("utf-8-sig"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        check("feed 是有效 JSON", False, str(error))
        return 1
    check("feed 是有效 JSON", True)

    version = manifest.get("version", "")
    check("version 是三段数字", bool(re.fullmatch(r"\d+\.\d+\.\d+", version)), repr(version))
    check("有 notes / pub_date", bool(manifest.get("notes")) and bool(manifest.get("pub_date")))
    check("pub_date 是 RFC3339 UTC", bool(re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", manifest.get("pub_date", ""))))

    platforms = manifest.get("platforms") or {}
    check(f"platforms 含 {PLATFORM}", PLATFORM in platforms, str(list(platforms)))
    platform = platforms.get(PLATFORM) or {}
    url = platform.get("url", "")
    signature = platform.get("signature", "")

    check(
        "资产 URL 指向本仓库且无空格",
        url.startswith(f"https://github.com/{SLUG}/releases/download/") and " " not in url and "%20" not in url,
        url,
    )

    # The signature string is base64(minisign signature file). Decode it fully: the version
    # field sits at the END of the trusted comment, so reading only a prefix gives a false
    # "no version" conclusion (that exact mistake is documented in DECISIONS D023).
    signed_version = None
    try:
        decoded = base64.b64decode(signature, validate=True).decode("utf-8", "replace")
        check("signature 可 base64 解码", True, f"{len(decoded)} 字符")
        match = re.search(r"version:([^\s]+)", decoded)
        signed_version = match.group(1) if match else None
    except Exception as error:  # noqa: BLE001 - a probe should report, not crash
        check("signature 可 base64 解码", False, str(error))
    check(
        "签名里的版本与 feed 一致（requireSignedVersion）",
        signed_version == version,
        f"signed={signed_version!r} announced={version!r}",
    )

    if args.sig:
        with open(args.sig, "rb") as handle:
            local = handle.read().decode("utf-8").strip()
        check("与本地 .sig 完全一致", signature == local, args.sig)

    if url:
        try:
            asset_status, _, final_asset = request(url, "HEAD")
            check("安装包资产可下载", asset_status == 200, f"HTTP {asset_status} → {final_asset.split('/')[2]}")
        except Exception as error:  # noqa: BLE001
            check("安装包资产可下载", False, str(error))

    failed = [name for name, ok, _ in checks if not ok]
    print(f"\n{len(checks) - len(failed)}/{len(checks)} 通过")
    if failed:
        print("失败项：" + "、".join(failed))
        return 1
    print(f"更新源可用：{feed_url}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
