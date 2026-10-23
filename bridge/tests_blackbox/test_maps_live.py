"""The live maps in the agents' pages (map_tiles.rs, 2026-10-23): the bridge fetches and keeps what MapLibre needs
(`/v1/maps/…`: the style rewritten to `bipmap://map/…`, vector tiles, glyphs, the sprite) from bbkit's FakeMaps under
`/vector`, never more than the style allows; map_image's card carries the places for the live map and keeps its
picture inline for everywhere else; the pages' policy opens only the maps' door."""
import gzip
import json
import re

from bbkit.common import AUTH
from bbkit.fakes import VECTOR_TILE
from bbkit.mcp import call, give_token, text_of

TILE = "/v1/maps/tiles/openmaptiles/14/8418/5843.pbf"


def vector_seen(fakes):
    return [r.path for r in fakes.maps.seen("/vector/")]


def test_the_style_points_only_at_the_apps_door(bridge, fakes):
    assert bridge.get("/v1/maps/style.json").status_code == 401
    got = bridge.get("/v1/maps/style.json", headers=AUTH)
    assert got.status_code == 200 and got.headers["content-type"] == "application/json"
    assert got.headers["cache-control"] == "private, max-age=3600"
    style = got.json()
    text = json.dumps(style)
    assert "127.0.0.1" not in text and "http" not in text   # nothing but bipmap:// addresses
    # only the provider's vector source (the raster one and the one on another host are left out, with their layers)
    assert list(style["sources"]) == ["openmaptiles"]
    source = style["sources"]["openmaptiles"]
    assert source["tiles"] == ["bipmap://map/tiles/openmaptiles/{z}/{x}/{y}.pbf"] and source["maxzoom"] == 14
    assert "OpenStreetMap contributors" in source["attribution"] and "<a" not in source["attribution"]
    assert [layer["id"] for layer in style["layers"]] == ["background", "water", "label"]
    assert style["glyphs"] == "bipmap://map/fonts/{fontstack}/{range}.pbf"
    assert style["sprite"] == "bipmap://map/sprite/sprite"
    # the style and its TileJSON asked once (kept a day), with the app's name; the other host never asked
    assert bridge.get("/v1/maps/style.json", headers=AUTH).json() == style
    assert vector_seen(fakes) == ["/vector/styles/positron", "/vector/planet"]
    assert all(r.header("user-agent").startswith("BipAgents/") for r in fakes.maps.seen("/vector/"))


def test_tiles_are_checked_kept_and_served_offline(bridge, fakes):
    got = bridge.get(TILE, headers=AUTH)
    assert got.status_code == 200 and got.content == VECTOR_TILE   # gzip kept as it came, decoded by the client
    assert got.headers["content-encoding"] == "gzip" and got.headers["content-type"] == "application/x-protobuf"
    assert got.headers["cache-control"] == "private, max-age=604800"
    kept = list((bridge.state_dir / "map-vector" / "tiles").rglob("5843.pbf"))
    assert len(kept) == 1 and gzip.decompress(kept[0].read_bytes()) == VECTOR_TILE
    assert bridge.get(TILE, headers=AUTH).content == VECTOR_TILE
    assert vector_seen(fakes).count("/vector/planet/20261018/14/8418/5843.pbf") == 1   # the second from the disk
    # only tiles of the world, at the zooms the source has, of a source the style has: nothing asked upstream
    before = len(fakes.maps.requests)
    for path in ["/v1/maps/tiles/openmaptiles/15/0/0.pbf", "/v1/maps/tiles/openmaptiles/2/4/0.pbf",
                 "/v1/maps/tiles/openmaptiles/2/0/4.pbf", "/v1/maps/tiles/ne2_shaded/2/0/0.pbf",
                 "/v1/maps/tiles/elsewhere/2/0/0.pbf", "/v1/maps/tiles/openmaptiles/2/0/0.png",
                 "/v1/maps/tiles/openmaptiles/2/0/..%2F..%2Fx.pbf"]:
        assert bridge.get(path, headers=AUTH).status_code in (400, 404), path
    assert len(fakes.maps.requests) == before
    # the provider down: what was kept still opens, the rest is a 502
    fakes.maps.vector_down = True
    assert bridge.get(TILE, headers=AUTH).content == VECTOR_TILE
    assert bridge.get("/v1/maps/tiles/openmaptiles/14/8418/5844.pbf", headers=AUTH).status_code == 502


def test_glyphs_and_sprite_only_what_the_style_uses(bridge, fakes):
    got = bridge.get("/v1/maps/fonts/Noto Sans Regular/256-511.pbf", headers=AUTH)
    assert got.status_code == 200 and got.content == b"glyphs:/fonts/Noto Sans Regular/256-511.pbf"
    before = len(fakes.maps.requests)
    for path in ["/v1/maps/fonts/Comic Sans/0-255.pbf", "/v1/maps/fonts/Noto Sans Regular/0-300.pbf",
                 "/v1/maps/fonts/Noto Sans Regular/65536-65791.pbf", "/v1/maps/fonts/Noto Sans Regular/0-255.png",
                 "/v1/maps/sprite/other.png", "/v1/maps/sprite/sprite@3x.png", "/v1/maps/elsewhere"]:
        assert bridge.get(path, headers=AUTH).status_code == 404, path
    assert len(fakes.maps.requests) == before
    png = bridge.get("/v1/maps/sprite/sprite@2x.png", headers=AUTH)
    assert png.status_code == 200 and png.headers["content-type"] == "image/png" and png.content.startswith(b"\x89PNG")
    assert bridge.get("/v1/maps/sprite/sprite.json", headers=AUTH).json()["park"]["width"] == 16
    assert "/vector/sprites/ofm_f384/ofm@2x.png" in vector_seen(fakes)


def test_misses_have_a_budget(bridge, fakes):
    """A page cannot make the bridge ask the provider for tiles without end: past the budget, 429 and nothing asked."""
    statuses = [bridge.get(f"/v1/maps/tiles/openmaptiles/14/{x}/7000.pbf", headers=AUTH).status_code
                for x in range(420)]
    assert statuses.count(200) >= 380 and statuses[-1] == 429, statuses.count(200)
    tiles = [p for p in vector_seen(fakes) if p.startswith("/vector/planet/20261018/")]
    assert len(tiles) == statuses.count(200)


def test_the_switch_turns_the_door_off(bridges, fakes):
    bridge = bridges.new("off", lambda c: c["maps"].update({"vector": "off"})).start()
    assert bridge.get("/v1/maps/style.json", headers=AUTH).status_code == 404
    assert bridge.get(TILE, headers=AUTH).status_code == 404
    assert vector_seen(fakes) == []


def test_map_image_gives_a_card_for_the_live_map_with_its_picture(bridge, fakes):
    give_token(bridge)
    made = call(bridge, "map_image", {"markers": [{"place": "45.7718, 4.8527", "label": "Porte <1>"},
                                                  {"place": "45.7802, 4.8521", "label": "Lac", "color": "blue"}],
                                      "line": True, "directions": "walking"})
    text = text_of(made)
    assert made["isError"] is False, text
    card = re.search(r'<figure class="map-card".*?</figure>', text).group(0)
    assert card.startswith('<figure class="map-card" data-line="1" data-directions="walking"><img src="map:m-')
    assert '<li data-x="' in card and 'data-lat="45.771800" data-lon="4.852700" data-color="#e5484d">Porte &lt;1&gt;</li>' in card
    assert 'data-lat="45.780200" data-lon="4.852100" data-color="#4f7cff">Lac</li>' in card
    assert len(fakes.maps.seen("/osm/")) > 0 and not fakes.maps.seen("/search")   # coordinates: no geocoder
    # in a page: the picture inline (the fallback anywhere the live map cannot open), the places kept for the kit
    page = call(bridge, "create_page", {"html": "<h1>Balade</h1>" + card, "title": "Balade"})
    assert page["isError"] is False, text_of(page)
    kept = (bridge.state_dir / "agent-pages" / "wellness" / "balade.html").read_text()
    assert '<figure class="map-card" data-line="1" data-directions="walking"><img src="data:image/png;base64,' in kept
    assert '<img src="map:m-' not in kept
    assert 'data-directions="walking"' in kept and 'data-lat="45.780200"' in kept
    policy = re.search(r'Content-Security-Policy" content="([^"]+)"', kept).group(1)
    assert "connect-src bipmap: https://bipmap.invalid;" in policy and "script-src 'unsafe-inline' bipmap: https://bipmap.invalid;" in policy
    assert "worker-src blob:" in policy and "unsafe-eval" not in policy and "default-src 'none'" in policy
    # the kit loads the library only through that door, and links places only to the app (bipmaps:)
    assert 'MAP_BASE = ANDROID ? "https://bipmap.invalid/" : "bipmap://map/"' in kept and '"bipmaps:place?lat="' in kept
    served = bridge.get("/v1/media", params={"path": "/bipagents/pages/wellness/balade.html"}, headers=AUTH)
    assert served.status_code == 200 and served.headers["content-security-policy"].startswith("sandbox allow-scripts; ")
    assert "connect-src bipmap: https://bipmap.invalid;" in served.headers["content-security-policy"]
