#!/usr/bin/env python3
"""Find and download a free photo from Wikimedia Commons (landscape, ≥ 1200 px), with its credit.
   photo.py "Cancale port" cancale.jpg   → prints {"path", "credit", "source"}"""
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

# Wikimedia's User-Agent policy (meta.wikimedia.org/wiki/User-Agent_policy): the application, a way to reach its
# makers, then the library.
UA = f"BipAgents/1.0 (https://bipagents.com) slides-styles/0.3 Python-urllib/{sys.version_info[0]}.{sys.version_info[1]}"
API = "https://commons.wikimedia.org/w/api.php"


def get(url, tries=4):
    for i in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": UA})
            return urllib.request.urlopen(req, timeout=30).read()
        except urllib.error.HTTPError as e:
            if e.code == 429 and i < tries - 1:
                time.sleep(3 * (i + 1))
                continue
            raise


def main():
    query, out = sys.argv[1], sys.argv[2]
    params = {"action": "query", "format": "json", "generator": "search", "gsrnamespace": 6,
              "gsrsearch": query + " filetype:bitmap", "gsrlimit": 12, "prop": "imageinfo",
              "iiprop": "url|size|extmetadata|mime", "iiurlwidth": 1600}
    data = json.loads(get(API + "?" + urllib.parse.urlencode(params)))
    pages = sorted((data.get("query") or {}).get("pages", {}).values(), key=lambda p: p.get("index", 99))
    for p in pages:
        ii = (p.get("imageinfo") or [{}])[0]
        if ii.get("mime") not in ("image/jpeg", "image/png") or ii.get("width", 0) < 1200:
            continue
        if ii["width"] < ii["height"]:   # landscape only
            continue
        meta = ii.get("extmetadata", {})
        cats = meta.get("Categories", {}).get("value", "").lower()
        date = re.sub("<[^>]+>", "", meta.get("DateTimeOriginal", {}).get("value", ""))
        year = re.search(r"(1[6-9]\d\d|20\d\d)", date)
        old = year and int(year.group(1)) < 1990
        if old or any(w in cats for w in ("painting", "postcard", "drawing", "engraving", "black and white", "historical")):
            continue   # old pictures, postcards and paintings look wrong in a modern deck
        open(out, "wb").write(get(ii.get("thumburl") or ii["url"]))
        artist = meta.get("Artist", {}).get("value", "")
        artist = re.sub("<[^>]+>", "", artist).strip()[:60]
        lic = meta.get("LicenseShortName", {}).get("value", "")
        print(json.dumps({"path": out, "credit": f"{artist} · {lic} · Wikimedia Commons".strip(" ·"),
                          "source": ii.get("descriptionurl")}, ensure_ascii=False))
        return 0
    print(json.dumps({"path": None, "error": "no landscape photo found, try other words"}))
    return 1


if __name__ == "__main__":
    sys.exit(main())
