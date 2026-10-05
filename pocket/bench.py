"""Time to first audio and speed of a running Pocket server (standard library only).

    python bench.py                       # http://127.0.0.1:8098, voice loutre
    python bench.py --voice en/loutre --text "Hi there! Three meetings today."
"""
import argparse
import http.client
import json
import time

SENTENCE = "Coucou ! J'ai regardé ta journée : trois rendez-vous, et un peu de temps pour marcher cet après-midi."


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8098)
    parser.add_argument("--voice", default="loutre")
    parser.add_argument("--text", default=SENTENCE)
    parser.add_argument("--runs", type=int, default=3)
    args = parser.parse_args()
    for run in range(1, args.runs + 1):
        conn = http.client.HTTPConnection(args.host, args.port, timeout=120)
        started = time.perf_counter()
        conn.request("POST", "/v1/audio/stream", json.dumps({"input": args.text, "voice": args.voice}),
                     {"Content-Type": "application/json"})
        response = conn.getresponse()
        if response.status != 200:
            raise SystemExit(f"HTTP {response.status}: {response.read().decode()[:200]}")
        first, size = None, 0
        while chunk := response.read1(65536):
            first = first if first is not None else time.perf_counter() - started
            size += len(chunk)
        total = time.perf_counter() - started
        audio = size / 48000  # PCM16 mono 24 kHz
        print(f"run {run}: first audio {first * 1000:.0f} ms, {audio:.1f} s of audio in {total:.2f} s "
              f"({audio / total:.1f}x real time)")
        conn.close()
    print("Good: first audio under ~300 ms and above 1.5x real time. Slower: try --quantize (x86) or dedicated vCPUs.")


if __name__ == "__main__":
    main()
