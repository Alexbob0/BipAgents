# Drama 3 voices (Fish Audio Drama 3 on audio.cpp)

The Bips' voices served by [audio.cpp](https://github.com/JarodMica/audio.cpp) (Apache 2.0), branch `drama-3-native`
at `27e92a31737382c9564f0f991666ab9169ca8438`, with the local patches in `patches/` (speaker turns across chunks,
pauses, near-real-time streaming, prefix and reference caches…). Model: `Jmica/Drama-3-GGUF`
(`drama-3-experts-q4_k-backbone-q8_0.gguf`), its « tower » requantized to Q8_0 (`tower-tensors.tsv`). The voices
themselves are reference clips with their transcripts in `voices/drama/`.

Model license: Fish Audio Community License — personal and small commercial use; no hosted service for third parties;
products built on it say « Built with Fish Audio ».

NVIDIA: `Containerfile.cuda` builds the same commit and patches on NVIDIA's CUDA images (amd64 and arm64); not
measured yet. Measured on a Strix Halo (Vulkan RADV): ~23 GiB of GPU memory, first audio 0.27 s once a voice is warm (~1 s the
first time), 1.2× real time.

BipAgents does not install it any more (since 2026-10-15): run it yourself, then connect it as your own voice model
(`install.sh voice`, port 8099 found on its own). The installer that set it up — build with these patches, model
download and check, tower requantization, rootless podman service on an internal network, voice prewarming — is in
the history (`installer/bipinstall/drama.py`, removed on 2026-10-15: `git log -- installer/bipinstall/drama.py` finds it). Its 14 voices (`voices/drama/`) are reference clips
for audio.cpp's `voice_presets`; the app shows them as « Voix 1… » with their bundled previews.
